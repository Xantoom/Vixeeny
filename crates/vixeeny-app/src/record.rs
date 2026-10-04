// SPDX-License-Identifier: GPL-3.0-or-later
//! Screen recording (plan 5.9): the capture stream feeds the recorder, on a thread of its own so
//! the action loop keeps answering the daemon. This is the CPU path; the target is the monitor
//! under the cursor.

use crate::toast::{Failed, Saved, Toast, failure_text};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::audio_rig::Rig;
use crate::widget_math::FromWidget;
use anyhow::Context;
use vixeeny_common::config::{Config, Profile};
use vixeeny_common::ipc::RecState;
use vixeeny_encode::audio::{AudioCodec, AudioTrackConfig};
use vixeeny_encode::clock::Fps;
use vixeeny_encode::d3d_convert as d3d;
use vixeeny_encode::gpu::{GpuPipeline, HwFrame, Source as GpuSource};
use vixeeny_encode::probe::ProbeResult;
use vixeeny_encode::recorder::{
    FrameFormat, OutputContainer, RecordConfig, Recorder, Split, VideoFrame,
};
use vixeeny_encode::registry::Vendor;
use vixeeny_encode::registry::{Chroma, Encoder, PresetName, Registry};
use vixeeny_encode::replay;
use vixeeny_encode::validate::{self, Context as ValidateContext, Severity};

/// The capture stream, and what it hands over.
use vixeeny_capture::VideoStream as Video;

/// How often a still screen gets its frame repeated.
const TICK: Duration = Duration::from_millis(100);

enum Ctl {
    Save,
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

    /// Replay only: writes the buffer's last seconds to a new file.
    pub fn save(&self) {
        let _ = self.ctl.send(Ctl::Save);
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
        // The kind the user chose: software never picks a GPU encoder.
        if profile.encoder_kind == "software" {
            return registry.get("libx264").context("no encoder available");
        }
        return validate::pick_auto(registry, probe).context("no encoder available");
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

type Namer = Box<dyn FnMut(u32) -> PathBuf + Send>;
type SaveNamer = Box<dyn FnMut() -> anyhow::Result<PathBuf> + Send>;

/// Everything decided before the capture starts.
struct Plan {
    config: RecordConfig,
    monitor: vixeeny_platform::MonitorInfo,
    cursor: bool,
    /// The monitor shows HDR and the profile keeps it: frames are scRGB half floats.
    hdr: bool,
    namer: Namer,
    /// Replay only: the path of the next save.
    save_as: Option<SaveNamer>,
    /// Frames are converted on the GPU on this device (see `try_gpu`).
    gpu: Option<GpuParts>,
    audio: Vec<vixeeny_audio::TrackPlan>,
    /// Filter the microphone sources (`[audio] mic_noise_reduction`).
    mic_denoise: bool,
    /// The widget to show, and the language of its labels.
    widget: Option<(vixeeny_common::config::RecordingWidget, String)>,
    /// The settings, for the notifications that end a session.
    notice: Config,
}

struct GpuParts {
    pipeline: Arc<GpuPipeline>,
    device: d3d::Device,
    context: d3d::DeviceContext,
}

/// Keeps the D3D11 device alive for the length of the recording.
struct KeepAlive(#[allow(dead_code)] Option<GpuParts>);

// SAFETY: the device is free-threaded and multithread protected; the value is only held, never
// used, by the recording thread.
unsafe impl Send for KeepAlive {}

/// The GPU path: NVENC / AMF read D3D11 textures converted by the video processor. `None` (with
/// the reason logged) when the encoder or the machine cannot do it: the CPU path then records.
fn try_gpu(
    encoder: &Encoder,
    config: &RecordConfig,
    source: (u32, u32),
    source_hdr: bool,
) -> Option<GpuParts> {
    if std::env::var_os("VIXEENY_NO_GPU").is_some()
        || !vixeeny_encode::gpu::supports(encoder)
        || config.chroma != Chroma::C420
        || !matches!(config.depth, 8 | 10)
    {
        return None;
    }
    let vendor = match encoder.vendor {
        Vendor::Nvidia => Some(0x10DE),
        Vendor::Amd => Some(0x1002),
        _ => None,
    };
    let built = d3d::create_device(vendor).and_then(|(device, context)| {
        GpuPipeline::new(
            &device,
            &context,
            GpuSource {
                size: source,
                hdr: source_hdr,
            },
            config.output_size,
            config.fps,
            config.hdr,
            config.depth == 10 || config.hdr,
        )
        .map(|pipeline| GpuParts {
            pipeline: Arc::new(pipeline),
            device,
            context,
        })
        .map_err(|e| e.to_string())
    });
    match built {
        Ok(parts) => Some(parts),
        Err(e) => {
            tracing::warn!("GPU recording path unavailable, using the CPU path: {e}");
            None
        }
    }
}

fn plan(config: &Config, allow_gpu: bool, replay: bool) -> anyhow::Result<Plan> {
    vixeeny_platform::ensure_dpi_aware();
    let monitors = vixeeny_platform::monitors()?;
    let cursor = vixeeny_platform::cursor_position()?;
    let monitor = vixeeny_platform::monitor_at(&monitors, cursor.0, cursor.1)
        .or_else(|| monitors.first())
        .context("no monitor")?
        .clone();
    let source = (monitor.rect.width, monitor.rect.height);

    // The replay has its own profile, by default the recording's.
    let profile_name = if replay && !config.replay.profile.is_empty() {
        &config.replay.profile
    } else {
        &config.video.profile
    };
    let profile = config
        .profiles
        .get(profile_name)
        .cloned()
        .unwrap_or_default();
    let registry = Registry::builtin().context("codec registry")?;
    let probe = crate::probe::current(false).ok();
    let ctx = ValidateContext {
        registry: &registry,
        source,
        probe: probe.as_ref(),
    };
    let issues = validate::validate(&profile, &ctx);
    if let Some(issue) = issues.iter().find(|i| i.severity == Severity::Error) {
        anyhow::bail!("the video profile is not valid: {issue:?}");
    }
    let encoder = choose_encoder(&registry, &profile, probe.as_ref())?.clone();

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
    let options = if profile.preset == "custom" {
        encoder
            .custom_options(&profile.params)
            .into_iter()
            .collect()
    } else {
        encoder
            .presets
            .get(preset)
            .iter()
            .map(|(k, v)| (k.clone(), v.to_ffmpeg()))
            .collect()
    };
    let (mut audio, unknown) = vixeeny_audio::plan_tracks(&profile.audio);
    for name in unknown {
        tracing::warn!("unknown audio source `{name}` ignored");
    }
    let audio_codec = AudioCodec::from_setting(&profile.audio.codec, container)
        .with_context(|| format!("unknown audio codec `{}`", profile.audio.codec))?;
    if profile.audio.surround && audio_codec.surround_in(container) {
        vixeeny_audio::assign_channels(
            &mut audio,
            vixeeny_audio::MAX_CHANNELS,
            vixeeny_audio::source_channels,
        );
    }
    let audio_configs = audio
        .iter()
        .map(|t| AudioTrackConfig {
            title: t.title.clone(),
            codec: audio_codec,
            bitrate_kbps: profile.audio.bitrate_kbps,
            vbr: profile.audio.vbr,
            channels: t.channels,
        })
        .collect();
    // An SDR monitor has nothing to preserve: such a recording stays SDR.
    let hdr = monitor.hdr.is_some() && matches!(profile.hdr.as_str(), "keep_hdr" | "hdr");
    #[allow(unused_mut)]
    let mut record = RecordConfig {
        options,
        container,
        output_size,
        fps: Fps::whole(profile.fps.clamp(1, 240)),
        depth: profile.depth,
        chroma,
        hdr,
        split: if replay {
            Split::Off
        } else {
            parse_split(&profile.split.mode)
        },
        vfr: profile.vfr,
        // A replay starts on a key frame: one every second keeps the cut within a second.
        keyframe_seconds: if replay { 1.0 } else { 2.0 },
        queue: 8,
        audio: audio_configs,
        gpu: None,
        encoder,
        replay_seconds: replay.then(|| replay::clamp_seconds(config.replay.duration_seconds)),
        replay_storage: replay_storage(config),
        files: !replay,
    };
    let gpu = if allow_gpu {
        try_gpu(&record.encoder, &record, source, hdr)
    } else {
        None
    };
    {
        record.gpu = gpu.as_ref().map(|g| Arc::clone(&g.pipeline));
    }

    let extension = container.extension();
    let (namer, save_as): (Namer, Option<SaveNamer>) = if replay {
        let config = config.clone();
        let monitor = monitor.clone();
        let save: SaveNamer =
            Box::new(move || output_file(&config, true, &monitor, output_size, extension));
        (Box::new(|_| PathBuf::new()), Some(save))
    } else {
        let first = output_file(config, false, &monitor, output_size, extension)?;
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
        (namer, None)
    };
    Ok(Plan {
        config: record,
        monitor,
        cursor: profile.show_cursor,
        hdr,
        namer,
        save_as,
        gpu,
        audio,
        mic_denoise: profile.audio.mic_noise_reduction,
        widget: (config.recording_widget.enabled && !replay).then(|| {
            (
                config.recording_widget.clone(),
                config.general.language.clone(),
            )
        }),
        notice: config.clone(),
    })
}

/// A new file in the videos (or replays) folder, named by the user's template.
fn output_file(
    config: &Config,
    replay: bool,
    monitor: &vixeeny_platform::MonitorInfo,
    size: (u32, u32),
    extension: &str,
) -> anyhow::Result<PathBuf> {
    let (folder, per_app, action) = if replay {
        (
            &config.paths.replays,
            config.paths.per_app_subfolder.replays,
            vixeeny_common::ipc::ActionId::ReplaySave,
        )
    } else {
        (
            &config.paths.videos,
            config.paths.per_app_subfolder.videos,
            vixeeny_common::ipc::ActionId::RecordToggle,
        )
    };
    let dir = vixeeny_common::paths::expand_user_dir(folder)
        .context("cannot locate the videos folder")?;
    let monitors = vixeeny_platform::monitors()?;
    let now = vixeeny_platform::local_time();
    let snapshot = crate::still::Snapshot {
        monitors: monitors.clone(),
        cursor: vixeeny_platform::cursor_position()?,
        foreground: vixeeny_platform::foreground_window()?,
    };
    let destination = crate::still::Destination {
        dir: &dir,
        template: &config.paths.filename_template,
        per_app_subfolder: per_app,
        use_foreground_app: config.paths.use_foreground_app,
        app_names: &config.paths.app_names,
        now: &now,
        after_save: None,
    };
    let vars = vixeeny_common::naming::Vars {
        app: crate::still::app_name(
            action,
            &snapshot,
            &destination,
            &vixeeny_platform::exe_metadata,
        ),
        title: String::new(),
        date: now.date(),
        time: now.time(),
        millis: now.millisecond,
        width: size.0,
        height: size.1,
        monitor: monitors
            .iter()
            .position(|m| m.id == monitor.id)
            .map_or_else(String::new, |i| (i + 1).to_string()),
    };
    Ok(vixeeny_common::naming::output_path(
        destination.dir,
        destination.per_app_subfolder,
        destination.template,
        &vars,
        extension,
        |p| p.exists(),
    ))
}

/// Starts a recording on its own thread; fails early (bad profile, no encoder, no capture) so
/// the caller can tell the user. The GPU path is tried first; if it cannot start, the same
/// recording starts on the CPU path.
pub fn start(config: &Config) -> anyhow::Result<Handle> {
    start_session(config, false)
}

/// Starts the replay buffer: the same capture and encoder, nothing written until a save.
pub fn start_replay(config: &Config) -> anyhow::Result<Handle> {
    start_session(config, true)
}

fn start_session(config: &Config, replay: bool) -> anyhow::Result<Handle> {
    let first = plan(config, true, replay)?;
    if first.gpu.is_none() {
        return launch(first);
    }
    match launch(first) {
        Ok(handle) => Ok(handle),
        Err(e) => {
            tracing::warn!("GPU recording failed to start ({e:#}); using the CPU path");
            launch(plan(config, false, replay)?)
        }
    }
}

/// Frames the GPU sink may queue for the recording thread.
const GPU_BACKLOG: usize = 4;

fn launch(plan: Plan) -> anyhow::Result<Handle> {
    // The recording's time zero: video and audio timestamps are relative to it.
    let origin = vixeeny_platform::monotonic_ns();
    let (stream, gpu_frames) = {
        let target = vixeeny_capture::StreamTarget::Monitor(plan.monitor.clone());
        let (stream, gpu_frames) = match &plan.gpu {
            None => (
                vixeeny_capture::VideoStream::start(&target, plan.cursor, plan.hdr)?,
                None,
            ),
            Some(gpu) => {
                let (tx, rx) = std::sync::mpsc::sync_channel::<(i64, HwFrame)>(GPU_BACKLOG);
                let pipeline = Arc::clone(&gpu.pipeline);
                let sink: vixeeny_capture::TextureSink =
                    Box::new(move |texture, content, time_ns| {
                        let rect = d3d::Rect {
                            left: 0,
                            top: 0,
                            right: content.0 as i32,
                            bottom: content.1 as i32,
                        };
                        // A refused conversion (pool exhausted, GPU busy) is a dropped frame.
                        if let Ok(frame) = pipeline.convert(texture, rect) {
                            let _ = tx.try_send((time_ns, frame));
                        }
                    });
                let capture = vixeeny_capture::GpuCapture {
                    device: gpu.device.clone(),
                    context: gpu.context.clone(),
                    sink,
                };
                (
                    vixeeny_capture::VideoStream::start_with(
                        &target,
                        plan.cursor,
                        plan.hdr,
                        Some(capture),
                    )?,
                    Some(rx),
                )
            }
        };
        (stream, gpu_frames)
    };
    let recorder = Recorder::start(plan.config, plan.namer)?;
    let (ctl, rx) = channel();
    // The widget is a nicety: without it the recording goes on (hotkeys still work).
    let widget = plan.widget.as_ref().and_then(|(settings, language)| {
        let presses = ctl.clone();
        crate::widget::Widget::spawn(settings, language, &plan.monitor, move |press| {
            let _ = presses.send(match press {
                FromWidget::TogglePause => Ctl::TogglePause,
                FromWidget::Stop => Ctl::Stop,
            });
        })
        .inspect_err(|e| tracing::warn!("recording widget: {e:#}"))
        .ok()
    });
    let rig = (!plan.audio.is_empty())
        .then(|| Rig::start(&plan.audio, origin, plan.notice.clone(), plan.mic_denoise));
    let state = Arc::new(AtomicU8::new(RECORDING));
    let thread_state = Arc::clone(&state);
    // The GPU parts must outlive the recording (the device the textures live on).
    let save_as = plan.save_as;
    let notice = plan.notice;
    let keep_alive = KeepAlive(plan.gpu);
    let thread = std::thread::Builder::new()
        .name("recording".into())
        .spawn(move || {
            let result = record_loop(
                &stream,
                gpu_frames.as_ref(),
                recorder,
                rig,
                widget,
                save_as,
                origin,
                &rx,
                &thread_state,
                &notice,
            );
            drop(stream);
            drop(keep_alive);
            thread_state.store(IDLE, Ordering::Release);
            match result {
                Ok(summary) => {
                    tracing::info!("recording finished: {summary:?}");
                    if let Some(file) = summary.files.first() {
                        let saved = Toast::Saved(Saved::Recording, file.clone());
                        crate::toast::notify(&notice, &saved);
                    }
                }
                Err(e) => {
                    tracing::error!("recording failed: {e:#}");
                    let text = failure_text(&e, crate::lang(&notice.general.language));
                    crate::toast::notify(&notice, &Toast::Failed(Failed::Recording, text));
                }
            }
        })?;
    Ok(Handle {
        ctl,
        state,
        thread: Some(thread),
        reported: RecState::Recording,
    })
}

/// Starts writing the replay; the file is finished (and reported) by a thread of its own, so the
/// recording goes on, and a second save right after does not wait for the first.
fn save_replay(recorder: &Recorder, next_path: &mut SaveNamer, notice: &Config) {
    let started = std::time::Instant::now();
    let save = next_path().and_then(|path| Ok(recorder.save_replay(path)?));
    match save {
        Ok(save) => {
            let seconds = save.seconds;
            let notice = notice.clone();
            std::thread::spawn(move || match save.wait() {
                Ok(path) => {
                    tracing::info!(
                        "replay saved: {} ({seconds:.1} s, {:.2} s to write)",
                        path.display(),
                        started.elapsed().as_secs_f64()
                    );
                    crate::toast::notify(&notice, &Toast::Saved(Saved::Replay, path));
                }
                Err(e) => {
                    tracing::error!("cannot save the replay: {e}");
                    let text = failure_text(
                        &anyhow::anyhow!("{e}"),
                        crate::lang(&notice.general.language),
                    );
                    crate::toast::notify(&notice, &Toast::Failed(Failed::Replay, text));
                }
            });
        }
        Err(e) => {
            tracing::error!("cannot save the replay: {e:#}");
            let text = failure_text(&e, crate::lang(&notice.general.language));
            crate::toast::notify(notice, &Toast::Failed(Failed::Replay, text));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn record_loop(
    stream: &Video,
    #[cfg(windows)] gpu_frames: Option<&std::sync::mpsc::Receiver<(i64, HwFrame)>>,
    recorder: Recorder,
    mut rig: Option<Rig>,
    mut widget: Option<crate::widget::Widget>,
    mut save_as: Option<SaveNamer>,
    origin: i64,
    ctl: &Receiver<Ctl>,
    state: &AtomicU8,
    notice: &Config,
) -> anyhow::Result<vixeeny_encode::recorder::Summary> {
    let now = || vixeeny_platform::monotonic_ns() - origin;
    let mut paused = false;
    let mut failure = None;
    // The widget shows the recorded time: pauses do not count.
    let mut paused_ns = 0_i64;
    let mut paused_since = 0_i64;
    let elapsed = |paused: bool, at: i64, paused_ns: i64, paused_since: i64| {
        Duration::from_nanos(
            (at - paused_ns - if paused { at - paused_since } else { 0 }).max(0) as u64,
        )
    };
    let mut last_shown = Duration::ZERO;
    if let Some(w) = &mut widget {
        w.state(false, Duration::ZERO);
    }
    loop {
        match ctl.try_recv() {
            Ok(Ctl::Save) => {
                if let Some(next_path) = &mut save_as {
                    save_replay(&recorder, next_path, notice);
                }
            }
            // A replay has no pause: it always keeps the last seconds.
            Ok(Ctl::TogglePause) if save_as.is_some() => {}
            Ok(Ctl::TogglePause) => {
                paused = !paused;
                if paused {
                    paused_since = now();
                } else {
                    paused_ns += now() - paused_since;
                }
                if let Some(w) = &mut widget {
                    w.state(paused, elapsed(paused, now(), paused_ns, paused_since));
                }
                if paused {
                    recorder.pause(now());
                    if let Some(rig) = &mut rig {
                        rig.pause(now());
                    }
                    state.store(PAUSED, Ordering::Release);
                } else {
                    recorder.resume(now());
                    if let Some(rig) = &mut rig {
                        rig.resume(now(), &recorder);
                    }
                    state.store(RECORDING, Ordering::Release);
                }
            }
            Ok(Ctl::Stop) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if let Some(rig) = &mut rig {
            rig.pump(now(), &recorder);
        }
        // The widget counts by itself; a resync every few seconds keeps it honest.
        if !paused && let Some(w) = &mut widget {
            let shown = elapsed(false, now(), paused_ns, paused_since);
            if shown.saturating_sub(last_shown) >= Duration::from_secs(5) {
                w.state(false, shown);
                last_shown = shown;
            }
        }
        if let Some(frames) = gpu_frames {
            // The stream only reports its end; the frames arrive through the sink's channel.
            if let Err(e) = stream.recv(Duration::ZERO) {
                failure = Some(e);
                break;
            }
            match frames.recv_timeout(TICK) {
                Ok((time_ns, frame)) => {
                    if !paused {
                        recorder.push_hw((time_ns - origin).max(0), frame);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if !paused {
                        recorder.tick(now());
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            continue;
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
                            format: if captured.hdr {
                                FrameFormat::ScRgbHalf
                            } else {
                                FrameFormat::Bgra8
                            },
                            data: f.data,
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
    if let Some(rig) = &mut rig {
        rig.finish(now(), &recorder);
    }
    let summary = recorder.stop(now())?;
    match failure {
        Some(e) => Err(anyhow::anyhow!("capture stopped: {e}")).context(format!("{summary:?}")),
        None => Ok(summary),
    }
}

/// Where the replay keeps its packets: in RAM, or in temporary files (`[replay] storage`).
fn replay_storage(config: &Config) -> replay::Storage {
    if config.replay.storage == "disk" {
        replay::Storage::Disk(std::env::temp_dir().join("Vixeeny-replay"))
    } else {
        replay::Storage::Ram
    }
}
