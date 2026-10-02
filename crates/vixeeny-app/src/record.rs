// SPDX-License-Identifier: GPL-3.0-or-later
//! Screen recording (plan 5.9): the capture stream feeds the recorder, on a thread of its own so
//! the action loop keeps answering the daemon. This is the CPU path; the target is the monitor
//! under the cursor.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context;
use vixeeny_common::config::{Config, Profile};
use vixeeny_common::ipc::RecState;
use vixeeny_encode::clock::Fps;
use vixeeny_encode::probe::ProbeResult;
use vixeeny_encode::recorder::{OutputContainer, RecordConfig, Recorder, Split, VideoFrame};
use vixeeny_encode::registry::{Chroma, Encoder, Platform, PresetName, Registry};
use vixeeny_encode::validate::{self, Context as ValidateContext, Severity};

/// How often a still screen gets its frame repeated.
const TICK: Duration = Duration::from_millis(100);

enum Ctl {
    TogglePause,
    Stop,
}

const IDLE: u8 = 0;
const RECORDING: u8 = 1;
const PAUSED: u8 = 2;

/// A running recording.
pub struct Handle {
    ctl: Sender<Ctl>,
    state: Arc<AtomicU8>,
    thread: Option<JoinHandle<()>>,
    reported: RecState,
}

impl Handle {
    pub fn pause_toggle(&self) {
        let _ = self.ctl.send(Ctl::TogglePause);
    }

    pub fn stop(&self) {
        let _ = self.ctl.send(Ctl::Stop);
    }

    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// The state if it changed since the last call.
    pub fn changed(&mut self) -> Option<RecState> {
        let now = match self.state.load(Ordering::Acquire) {
            RECORDING => RecState::Recording,
            PAUSED => RecState::Paused,
            _ => RecState::Idle,
        };
        (now != self.reported).then(|| {
            self.reported = now;
            now
        })
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.ctl.send(Ctl::Stop);
            let _ = thread.join();
        }
    }
}

/// The encoder a profile asks for: its own, or the best detected one for `auto`.
fn choose_encoder<'a>(
    registry: &'a Registry,
    profile: &Profile,
    probe: Option<&ProbeResult>,
) -> anyhow::Result<&'a Encoder> {
    if profile.encoder == "auto" {
        return validate::pick_auto(registry, Platform::current(), probe)
            .context("no encoder available");
    }
    registry
        .get(&profile.encoder)
        .with_context(|| format!("unknown encoder `{}`", profile.encoder))
}

fn parse_split(mode: &str) -> Split {
    // `off`, `size:<MB>` or `duration:<minutes>`.
    match mode.split_once(':') {
        Some(("size", mb)) => mb
            .parse::<u64>()
            .map_or(Split::Off, |mb| Split::Bytes(mb * 1024 * 1024)),
        Some(("duration", min)) => min
            .parse::<u64>()
            .map_or(Split::Off, |m| Split::Duration(Duration::from_secs(m * 60))),
        _ => Split::Off,
    }
}

/// Everything decided before the capture starts.
struct Plan {
    config: RecordConfig,
    monitor: vixeeny_platform::MonitorInfo,
    cursor: bool,
    namer: Box<dyn FnMut(u32) -> std::path::PathBuf + Send>,
}

fn plan(config: &Config) -> anyhow::Result<Plan> {
    vixeeny_platform::ensure_dpi_aware();
    let monitors = vixeeny_platform::monitors()?;
    let cursor = vixeeny_platform::cursor_position()?;
    let monitor = vixeeny_platform::monitor_at(&monitors, cursor.0, cursor.1)
        .or_else(|| monitors.first())
        .context("no monitor")?
        .clone();
    let source = (monitor.rect.width, monitor.rect.height);

    let profile = config
        .profiles
        .get(&config.video.profile)
        .cloned()
        .unwrap_or_default();
    let registry = Registry::builtin();
    let probe = crate::probe::current(false).ok();
    let ctx = ValidateContext {
        registry,
        platform: Platform::current(),
        source,
        probe: probe.as_ref(),
    };
    let issues = validate::validate(&profile, &ctx);
    if let Some(issue) = issues.iter().find(|i| i.severity == Severity::Error) {
        anyhow::bail!("the video profile is not valid: {issue:?}");
    }
    let encoder = choose_encoder(registry, &profile, probe.as_ref())?.clone();

    let container = OutputContainer::from_setting(&profile.container)
        .with_context(|| format!("unknown container `{}`", profile.container))?;
    let (w, h) = validate::output_size(&profile.resolution, source).context("bad resolution")?;
    let output_size = (w.max(2) & !1, h.max(2) & !1);
    let chroma = match profile.chroma.as_str() {
        "444" => Chroma::C444,
        "422" => Chroma::C422,
        _ => Chroma::C420,
    };
    let preset = PresetName::from_setting(&profile.preset).unwrap_or(PresetName::Balanced);
    let options = encoder
        .presets
        .get(preset)
        .iter()
        .map(|(k, v)| (k.clone(), v.to_ffmpeg()))
        .collect();
    let record = RecordConfig {
        options,
        container,
        output_size,
        fps: Fps::whole(profile.fps.clamp(1, 240)),
        depth: profile.depth,
        chroma,
        hdr: matches!(profile.hdr.as_str(), "keep_hdr" | "hdr"),
        split: parse_split(&profile.split.mode),
        keyframe_seconds: 2.0,
        queue: 8,
        encoder,
    };

    let dir = vixeeny_common::paths::expand_user_dir(&config.paths.videos)
        .context("cannot locate the videos folder")?;
    let now = vixeeny_platform::local_time();
    let snapshot = crate::still::Snapshot {
        monitors: monitors.clone(),
        cursor,
        foreground: vixeeny_platform::foreground_window()?,
    };
    let destination = crate::still::Destination {
        dir: &dir,
        template: &config.paths.filename_template,
        per_app_subfolder: config.paths.per_app_subfolder.videos,
        use_foreground_app: config.paths.use_foreground_app,
        app_names: &config.paths.app_names,
        now: &now,
        after_save: None,
    };
    let vars = vixeeny_common::naming::Vars {
        app: crate::still::app_name(
            vixeeny_common::ipc::ActionId::RecordToggle,
            &snapshot,
            &destination,
            &vixeeny_platform::exe_metadata,
        ),
        title: String::new(),
        date: now.date(),
        time: now.time(),
        millis: now.millisecond,
        width: output_size.0,
        height: output_size.1,
        monitor: monitors
            .iter()
            .position(|m| m.id == monitor.id)
            .map_or_else(String::new, |i| (i + 1).to_string()),
    };
    let extension = container.extension();
    let first = vixeeny_common::naming::output_path(
        destination.dir,
        destination.per_app_subfolder,
        destination.template,
        &vars,
        extension,
        |p| p.exists(),
    );
    let namer = Box::new(move |part: u32| {
        if part == 0 {
            return first.clone();
        }
        let stem = first
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("video");
        first.with_file_name(format!("{stem}_part{}.{extension}", part + 1))
    });
    Ok(Plan {
        config: record,
        monitor,
        cursor: profile.show_cursor,
        namer,
    })
}

/// Starts a recording on its own thread; fails early (bad profile, no encoder, no capture) so
/// the caller can tell the user.
pub fn start(config: &Config) -> anyhow::Result<Handle> {
    let plan = plan(config)?;
    let stream = vixeeny_capture::VideoStream::start(
        &vixeeny_capture::StreamTarget::Monitor(plan.monitor),
        plan.cursor,
    )?;
    let recorder = Recorder::start(plan.config, plan.namer)?;
    let state = Arc::new(AtomicU8::new(RECORDING));
    let (ctl, rx) = channel();
    let thread_state = Arc::clone(&state);
    let thread = std::thread::Builder::new()
        .name("recording".into())
        .spawn(move || {
            let result = record_loop(&stream, recorder, &rx, &thread_state);
            thread_state.store(IDLE, Ordering::Release);
            match result {
                Ok(summary) => tracing::info!("recording finished: {summary:?}"),
                Err(e) => tracing::error!("recording failed: {e:#}"),
            }
        })?;
    Ok(Handle {
        ctl,
        state,
        thread: Some(thread),
        reported: RecState::Recording,
    })
}

fn record_loop(
    stream: &vixeeny_capture::VideoStream,
    recorder: Recorder,
    ctl: &Receiver<Ctl>,
    state: &AtomicU8,
) -> anyhow::Result<vixeeny_encode::recorder::Summary> {
    let origin = vixeeny_platform::monotonic_ns();
    let now = || vixeeny_platform::monotonic_ns() - origin;
    let mut paused = false;
    let mut failure = None;
    loop {
        match ctl.try_recv() {
            Ok(Ctl::TogglePause) => {
                paused = !paused;
                if paused {
                    recorder.pause(now());
                    state.store(PAUSED, Ordering::Release);
                } else {
                    recorder.resume(now());
                    state.store(RECORDING, Ordering::Release);
                }
            }
            Ok(Ctl::Stop) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        match stream.recv(TICK) {
            Ok(Some(captured)) => {
                if !paused {
                    let f = captured.frame;
                    let t = (captured.time_ns - origin).max(0);
                    recorder.push_frame(
                        t,
                        VideoFrame {
                            width: f.width,
                            height: f.height,
                            stride: f.stride,
                            bgra: f.data,
                        },
                    );
                }
            }
            Ok(None) => {
                if !paused {
                    recorder.tick(now());
                }
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    let summary = recorder.stop(now())?;
    match failure {
        Some(e) => Err(anyhow::anyhow!("capture stopped: {e}")).context(format!("{summary:?}")),
        None => Ok(summary),
    }
}
