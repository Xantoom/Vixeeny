// SPDX-License-Identifier: GPL-3.0-or-later
#![windows_subsystem = "windows"]
//! The on-demand side of Vixeeny: the daemon starts it to capture, record or show a window, and
//! it exits after `general.app_idle_exit_seconds` without activity (plan section 1.2). The same
//! program also runs the settings, the notifications, the recording widget and the updates, each
//! as a process of its own (see the `--` modes of `run`).

use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::Duration;

mod audio_rig;
mod clipboard;
mod probe;
mod record;
mod region;
mod scroll;
mod settings;
mod side;
mod still;
mod sysinfo;
mod thumbnail;
mod toast;
mod update;
mod widget;
mod widget_math;

use anyhow::Context;
use vixeeny_common::config::Config;
use vixeeny_common::ipc::{self, ActionId, AppToDaemon, DaemonToApp, Endpoint, Hello};

fn main() {
    vixeeny_common::logging::init("vixeeny-app");
    if let Err(e) = run() {
        tracing::error!("fatal: {e:#}");
        eprintln!("vixeeny-app: {e:#}");
        std::process::exit(1);
    }
}

/// The display language: the setting, or the OS's own for `auto`.
fn lang(setting: &str) -> vixeeny_common::i18n::Lang {
    vixeeny_common::i18n::Lang::resolve(setting, vixeeny_platform::user_locale().as_deref())
}

/// The look of the native zone editor: the one every window has.
fn overlay_look() -> vixeeny_overlay::Look {
    let look = vixeeny_ui::theme::default_look();
    vixeeny_overlay::Look {
        dark: look.dark,
        accent: look.accent,
        animations: look.animations,
    }
}

/// Runs one action. Failures are logged, not fatal: the app stays available for the next one.
fn perform(action: ActionId, config: &Config, frozen: Option<ipc::Frozen>) {
    use ActionId::{CaptureAllMonitors, CaptureFullscreen, CaptureWindow};
    match action {
        ActionId::CaptureRegion => {
            if let Err(e) = region::run(config, region::Mode::Editor, frozen) {
                tracing::error!("editor failed: {e:#}");
            }
        }
        ActionId::CaptureScrolling => {
            if let Err(e) = region::run(config, region::Mode::Scroll, frozen) {
                tracing::error!("scrolling capture failed: {e:#}");
            }
        }
        CaptureFullscreen | CaptureWindow | CaptureAllMonitors => {
            match direct_capture(action, config) {
                Ok(path) => {
                    tracing::info!("saved {}", path.display());
                    toast::notify(config, &toast::Toast::Saved(toast::Saved::Image, path));
                }
                Err(e) => {
                    tracing::error!("capture failed: {e:#}");
                    let lang = lang(&config.general.language);
                    let text = toast::failure_text(&e, lang);
                    toast::notify(config, &toast::Toast::Failed(toast::Failed::Capture, text));
                }
            }
        }
        other => tracing::info!("action {other:?} is not implemented yet"),
    }
}

fn direct_capture(action: ActionId, config: &Config) -> anyhow::Result<std::path::PathBuf> {
    use vixeeny_capture::{CaptureOptions, Capturer};

    let started = std::time::Instant::now();
    vixeeny_platform::ensure_dpi_aware();
    let snapshot = still::Snapshot {
        monitors: vixeeny_platform::monitors()?,
        cursor: vixeeny_platform::cursor_position()?,
        foreground: vixeeny_platform::foreground_window()?,
    };
    let dir = vixeeny_common::paths::expand_user_dir(&config.paths.images)
        .context("cannot locate the images folder")?;
    let now = vixeeny_platform::local_time();
    let destination = still::Destination {
        dir: &dir,
        template: &config.paths.naming.images.template,
        per_app_subfolder: config.paths.per_app_subfolder.images,
        use_foreground_app: true,
        app_names: &config.paths.app_names,
        now: &now,
        after_save: None,
    };
    let backend = vixeeny_capture::WgcBackend::new()?;
    let mut capturer = Capturer::new(backend, snapshot.monitors.clone());
    let options = CaptureOptions {
        show_cursor: false,
        tonemap: (config.image.hdr == "tonemap_sdr")
            .then_some(tonemap_hdr as vixeeny_capture::ToneMapFn),
    };
    let (format, settings) = image_output(config);
    let copy = |image: &vixeeny_image::Bgra<'_>| {
        if let Err(e) = clipboard::copy_bgra(config, image) {
            tracing::warn!("clipboard: {e:#}");
        }
    };
    let destination = still::Destination {
        after_save: config.image.copy_to_clipboard.then_some(&copy),
        ..destination
    };
    let path = still::run(
        action,
        &snapshot,
        &mut capturer,
        &destination,
        options,
        (format, &settings),
        &vixeeny_platform::exe_metadata,
    )?;
    tracing::info!("direct capture took {:?}", started.elapsed());
    Ok(path)
}

/// HDR monitors become SDR through the documented tone mapper (`vixeeny_image::tonemap`).
fn tonemap_hdr(rgba: &[f32], info: &vixeeny_platform::HdrInfo) -> Vec<u8> {
    vixeeny_image::tonemap::tonemap_frame(
        rgba,
        &vixeeny_image::tonemap::ToneMapParams {
            sdr_white_nits: info.sdr_white_nits,
            peak_nits: info.peak_nits,
        },
    )
}

/// Output format and settings from `[image]`. An unknown value falls back to the default (PNG,
/// 4:4:4), never to a failed capture.
fn image_output(config: &Config) -> (vixeeny_image::ImageFormat, vixeeny_image::Settings) {
    use vixeeny_image::{ImageFormat, PngCompression, Settings};
    let image = &config.image;
    let mut settings = Settings::default();
    settings.png.compression = match image.png.compression.as_str() {
        "default" => PngCompression::Default,
        "high" => PngCompression::High,
        _ => PngCompression::Fast,
    };
    settings.png.oxipng_level = (image.png.optimize > 0).then(|| image.png.optimize.min(6));
    settings.jpeg.quality = image.jpeg.quality.clamp(1, 100);
    settings.webp.lossless = image.webp.lossless;
    settings.webp.quality = f32::from(image.webp.quality.min(100));
    settings.webp.effort = image.webp.effort.min(6);
    settings.avif.quality = image.avif.quality.min(100);
    settings.avif.depth = if image.avif.depth >= 10 { 10 } else { 8 };
    settings.avif.speed = image.avif.speed.min(10);
    settings.jxl.lossless = image.jxl.lossless;
    settings.jxl.distance = jxl_distance(image.jxl.quality);
    settings.jxl.effort = image.jxl.effort.clamp(1, 9);
    let format = ImageFormat::from_name(&config.image.format)
        .filter(|f| f.available())
        .unwrap_or_else(|| {
            tracing::warn!(
                "image format `{}` is not available, using PNG",
                config.image.format
            );
            ImageFormat::Png
        });
    (format, settings)
}

/// The JPEG XL distance of a 1-100 quality, as `cjxl -q` maps it (90 → 1.0, visually lossless).
fn jxl_distance(quality: u8) -> f32 {
    let q = f32::from(quality.clamp(1, 100));
    if q >= 30.0 {
        0.1 + (100.0 - q) * 0.09
    } else {
        6.4 + (30.0 - q) * 0.2
    }
}

/// `--action <name>` (default: open the settings), and the screens the daemon froze for it
/// (`--frozen`).
fn parse_action() -> anyhow::Result<(ActionId, Option<ipc::Frozen>)> {
    let mut args = std::env::args().skip(1);
    let mut action = ActionId::OpenSettings;
    let mut frozen = None;
    while let Some(arg) = args.next() {
        if arg == "--action" {
            let name = args.next().context("--action needs a value")?;
            action = ActionId::from_cli_name(&name)
                .with_context(|| format!("unknown action `{name}`"))?;
        } else if arg == "--frozen" {
            let value = args.next().context("--frozen needs a value")?;
            frozen = ipc::Frozen::from_arg(&value);
        }
    }
    Ok((action, frozen))
}

/// An action is running (the next ones wait for it).
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Runs one action. Returns a follow-up action when the action was the overlay and the user
/// picked something in it.
fn run_action(
    action: ActionId,
    frozen: Option<ipc::Frozen>,
    config: &mut Config,
    recording: &mut Recording,
) -> anyhow::Result<Option<ActionId>> {
    match action {
        ActionId::RecordToggle
        | ActionId::RecordPause
        | ActionId::ReplayToggle
        | ActionId::ReplaySave => recording.handle(action, config),
        ActionId::ReplayWatch => recording.watch(config, true),
        ActionId::OverlayToggle => return Ok(overlay(config, recording)),
        ActionId::OpenSettings => open_settings(),
        _ => perform(action, config, frozen),
    }
    Ok(REOPEN_STRIP.take().then_some(ActionId::OverlayToggle))
}

struct Actions<'a, W: std::io::Write> {
    config: &'a mut Config,
    recording: &'a mut Recording,
    send: &'a mut W,
}

impl<W: std::io::Write> Actions<'_, W> {
    /// Runs `action`, then the one the user picked in the overlay, if any.
    fn run(&mut self, action: ActionId, frozen: Option<ipc::Frozen>) -> anyhow::Result<()> {
        use std::sync::atomic::Ordering;
        BUSY.store(true, Ordering::Release);
        // The app may run for long (the replay): each action reads the settings as they are now.
        if let Some(config) =
            vixeeny_common::paths::config_file().and_then(|path| Config::load(&path).ok())
        {
            *self.config = config;
        }
        let mut next = Some((action, frozen));
        let mut result = Ok(());
        while let Some((action, frozen)) = next.take() {
            match run_action(action, frozen, self.config, self.recording) {
                Ok(then) => next = then.map(|a| (a, None)),
                Err(e) => result = Err(e),
            }
        }
        BUSY.store(false, Ordering::Release);
        result?;
        self.recording.report(self.send)
    }
}

thread_local! {
    /// The side strip was pinned the last time it closed: it opens pinned.
    static STRIP_PINNED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The strip was pinned when the user picked the action that runs now: show it again after.
    static REOPEN_STRIP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The settings window is a process of its own: the hotkeys keep working while it is open.
fn open_settings() {
    if let Err(e) = settings::spawn(false) {
        tracing::error!("{e:#}");
    }
}

fn overlay(config: &Config, recording: &Recording) -> Option<ActionId> {
    let (recording, replay) = recording.flags();
    let outcome = match side::run(config, recording, replay, STRIP_PINNED.get()) {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!("overlay failed: {e:#}");
            return None;
        }
    };
    STRIP_PINNED.set(outcome.pinned);
    // The strip is gone once the loop ends, but give the compositor a moment to repaint before
    // a capture freezes the screen: the strip must not be in it.
    if outcome.action.is_some() {
        std::thread::sleep(Duration::from_millis(120));
    }
    // Pinned: it comes back once the action is done (see `run_action`).
    REOPEN_STRIP.set(outcome.pinned && outcome.action.is_some());
    outcome.action
}

/// The recording started by `RecordToggle`, if any (Windows with FFmpeg only).
#[derive(Default)]
struct Recording {
    handle: Option<record::Handle>,
    /// The replay buffer (`ReplayToggle`), which runs alongside a recording.
    replay: Option<record::Handle>,
    /// The game (its process) the replay was started for: the replay stops when it closes.
    replay_game: Option<u32>,
    /// A game whose replay the user stopped by hand: it is not started again for it.
    declined: Option<u32>,
    /// When the foreground was last looked at.
    watched: Option<std::time::Instant>,
    /// Asks the main loop to look at the foreground again (the game closed).
    look: Option<Sender<Wake>>,
}

/// What wakes the main loop.
enum Wake {
    Daemon(DaemonToApp),
    /// Another window came to the foreground, or the game of the replay closed.
    Look,
}

/// While the replay is on, the foreground is looked at when it changes, and this often in case a
/// game went full screen after taking the focus.
const LOOK_AGAIN: Duration = Duration::from_secs(3);

impl Recording {
    /// `(recording, replay buffer)` is running.
    fn flags(&self) -> (bool, bool) {
        (self.handle.is_some(), self.replay.is_some())
    }

    /// With the replay on: starts it when a full-screen game comes to the foreground, stops it
    /// when that game closes. Looks every [`LOOK_AGAIN`] at most, unless `now`.
    fn watch(&mut self, config: &Config, now: bool) {
        if !now && self.watched.is_some_and(|t| t.elapsed() < LOOK_AGAIN) {
            return;
        }
        self.watched = Some(std::time::Instant::now());
        if let Some(pid) = self.replay_game {
            if !config.replay.enabled || !vixeeny_platform::process_alive(pid) {
                tracing::info!("the game closed (or the replay was turned off): replay stopped");
                self.replay_game = None;
                self.replay = None;
            }
            return;
        }
        if !config.replay.enabled || self.replay.is_some() {
            return;
        }
        let Some(game) = still::game_in_front() else {
            return;
        };
        if self.declined == Some(game.pid) {
            return;
        }
        match record::start_replay(config) {
            Ok(handle) => {
                tracing::info!("full-screen game ({}): replay started", game.title);
                self.replay = Some(handle);
                self.replay_game = Some(game.pid);
                if let Some(look) = self.look.clone() {
                    vixeeny_platform::on_process_exit(game.pid, move || {
                        let _ = look.send(Wake::Look);
                    });
                }
            }
            Err(e) => {
                tracing::error!("cannot start the replay buffer: {e:#}");
                // Not again for this game.
                self.declined = Some(game.pid);
            }
        }
    }

    fn handle(&mut self, action: ActionId, config: &Config) {
        match action {
            ActionId::ReplayToggle => {
                match self.replay.take() {
                    // Dropping the handle stops the buffer; stopped by hand during a game, it
                    // stays off for that game.
                    Some(replay) => {
                        drop(replay);
                        self.declined = self.replay_game.take();
                    }
                    None => match record::start_replay(config) {
                        Ok(handle) => self.replay = Some(handle),
                        Err(e) => tracing::error!("cannot start the replay buffer: {e:#}"),
                    },
                }
                return;
            }
            ActionId::ReplaySave => {
                match &self.replay {
                    Some(replay) => replay.save(),
                    None => tracing::info!("replay save ignored: the buffer is not running"),
                }
                return;
            }
            _ => {}
        }
        match (action, &self.handle) {
            (ActionId::RecordToggle, Some(handle)) => handle.stop(),
            (ActionId::RecordToggle, None) => match record::start(config) {
                Ok(handle) => self.handle = Some(handle),
                Err(e) => {
                    tracing::error!("cannot start the recording: {e:#}");
                    let text = toast::failure_text(&e, lang(&config.general.language));
                    toast::notify(
                        config,
                        &toast::Toast::Failed(toast::Failed::Recording, text),
                    );
                }
            },
            (_, Some(handle)) => handle.pause_toggle(),
            (_, None) => {}
        }
    }

    /// Tells the daemon (tray icon) when the state changed, and forgets a finished recording.
    fn report(&mut self, send: &mut impl std::io::Write) -> anyhow::Result<()> {
        let Some(handle) = self.handle.as_mut() else {
            return Ok(());
        };
        let finished = handle.finished();
        let state = if finished {
            Some(ipc::RecState::Idle)
        } else {
            handle.changed()
        };
        if let Some(state) = state {
            ipc::write_msg(send, &AppToDaemon::RecordingStateChanged(state))?;
        }
        if finished {
            self.handle = None;
        }
        Ok(())
    }
}

/// Light or dark, the system accent, the title-bar styling: what every window of this process
/// starts from.
fn init_look() {
    let config = vixeeny_common::paths::config_file()
        .and_then(|path| Config::load(&path).ok())
        .unwrap_or_default();
    vixeeny_ui::theme::set_default(settings::look_of(&config));
    vixeeny_ui::theme::set_dresser(|handle, look| {
        let _ = vixeeny_platform::style_window(
            vixeeny_platform::WindowId(handle),
            look.dark,
            look.caption(),
        );
    });
}

fn run() -> anyhow::Result<()> {
    if let Err(e) = vixeeny_platform::set_app_id() {
        tracing::warn!("{e}");
    }
    init_look();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // The probe child: prints TOML on stdout (see `vixeeny_encode::probe::run_child`).
        Some("--probe") => return probe::child(),
        Some("--warm-probe") => return probe::warm(),
        // The recording widget, a process of its own (see `widget`).
        Some("--widget") => return widget::run_child(&args[1..]),
        // The settings window and gallery, a process of its own (see `settings`).
        Some("--settings") => return settings::run_child(&args[1..]),
        // A notification card (see `toast`).
        Some("--toast") => return toast::run_child(&args[1..]),
        Some("--toast-host") => return toast::run_host(),
        // The daily update check, and the installation (see `update`).
        Some("--update") => return update::run_child(&args[1..]),
        Some("--system-info") => {
            vixeeny_platform::attach_console();
            let config = vixeeny_common::paths::config_file()
                .and_then(|path| Config::load(&path).ok())
                .unwrap_or_default();
            let probe = probe::current(false).ok();
            print!(
                "{}",
                sysinfo::format(&sysinfo::collect(&config, probe.as_ref()))
            );
            return Ok(());
        }
        Some("--probe-report") => return probe::report(args.iter().any(|a| a == "--force")),
        _ => {}
    }
    let (first_action, first_frozen) = parse_action()?;
    let mut config = vixeeny_common::paths::config_file()
        .and_then(|path| Config::load(&path).ok())
        .unwrap_or_default();
    let idle = config.general.app_idle_exit_seconds;
    let idle = Duration::from_secs(u64::from(idle));

    let stream = Endpoint::current_user()
        .connect()
        .context("the Vixeeny daemon is not running")?;
    let (mut recv, mut send) = {
        use ipc::StreamTrait;
        stream.split()
    };
    ipc::write_msg(
        &mut send,
        &Hello::App {
            pid: std::process::id(),
        },
    )?;
    ipc::write_msg(&mut send, &AppToDaemon::Ready)?;

    let (tx, rx) = channel();
    let look = tx.clone();
    std::thread::Builder::new()
        .name("ipc-read".into())
        .spawn(move || {
            while let Ok(Some(mut msg)) = ipc::read_msg::<_, DaemonToApp>(&mut recv) {
                // Frozen while another action runs: they would hide it, and the action that
                // waits for its turn captures the screen itself.
                if let DaemonToApp::RunAction { frozen, .. } = &mut msg
                    && BUSY.load(std::sync::atomic::Ordering::Acquire)
                    && let Some(frozen) = frozen.take()
                {
                    vixeeny_capture::freeze::release(&frozen);
                }
                if tx.send(Wake::Daemon(msg)).is_err() {
                    break;
                }
            }
        })?;

    eprintln!(
        "vixeeny-app {} started for {first_action:?}",
        env!("CARGO_PKG_VERSION")
    );
    let mut recording = Recording {
        look: Some(look.clone()),
        ..Recording::default()
    };
    // The daemon started this process for an action: it comes on the command line, not over the
    // connection (it is not queued on the daemon's side).
    let mut actions = Actions {
        config: &mut config,
        recording: &mut recording,
        send: &mut send,
    };
    actions.run(first_action, first_frozen)?;
    let mut hooked = false;
    // Only what the daemon sends counts as activity: not the foreground changing.
    let mut idle_from = std::time::Instant::now();
    loop {
        // While recording, the app polls the recording's state; while the replay is on, it
        // waits for a game. It does not exit as idle meanwhile.
        let polling = recording.handle.is_some();
        let watching = config.replay.enabled || recording.replay.is_some();
        if watching && !hooked {
            let look = look.clone();
            hooked = vixeeny_platform::on_foreground_change(move || {
                let _ = look.send(Wake::Look);
            });
        }
        if polling || watching {
            idle_from = std::time::Instant::now();
        }
        let wait = if polling {
            Duration::from_millis(200)
        } else if watching {
            LOOK_AGAIN
        } else {
            idle.saturating_sub(idle_from.elapsed())
        };
        match rx.recv_timeout(wait) {
            Ok(Wake::Daemon(DaemonToApp::RunAction { action, frozen })) => {
                idle_from = std::time::Instant::now();
                Actions {
                    config: &mut config,
                    recording: &mut recording,
                    send: &mut send,
                }
                .run(action, frozen)?;
            }
            Ok(Wake::Look) => recording.watch(&config, true),
            Err(RecvTimeoutError::Timeout) if polling || watching => {
                recording.watch(&config, false);
                recording.report(&mut send)?;
            }
            Ok(Wake::Daemon(DaemonToApp::ConfigChanged)) => {}
            // The daemon asked us to stop, or went away.
            Ok(Wake::Daemon(DaemonToApp::Shutdown)) | Err(RecvTimeoutError::Disconnected) => {
                return Ok(());
            }
            Err(RecvTimeoutError::Timeout) if idle_from.elapsed() >= idle => {
                ipc::write_msg(&mut send, &AppToDaemon::Idle)?;
                return Ok(());
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vixeeny_image::{Chroma, ImageFormat};

    #[test]
    fn config_maps_to_encoder_settings() {
        let mut config = Config::default();
        assert_eq!(image_output(&config).0, ImageFormat::Png);
        config.image.format = "jpeg".into();
        config.image.jpeg.quality = 70;
        let (format, settings) = image_output(&config);
        let native = cfg!(feature = "native-codecs");
        assert_eq!(
            format,
            if native {
                ImageFormat::Jpeg
            } else {
                ImageFormat::Png
            }
        );
        assert_eq!(
            (settings.jpeg.quality, settings.jpeg.chroma),
            (70, Chroma::Yuv444)
        );
        config.image.format = "bmp".into(); // unknown
        assert_eq!(image_output(&config).0, ImageFormat::Png);
        config.image.format = "avif".into();
        let expected = if cfg!(feature = "native-codecs") {
            ImageFormat::Avif
        } else {
            ImageFormat::Png
        };
        assert_eq!(image_output(&config).0, expected);
        // Every format's own options reach its encoder.
        config.image.png.compression = "high".into();
        config.image.png.optimize = 9;
        config.image.webp.lossless = false;
        config.image.avif.speed = 2;
        config.image.jxl.quality = 90;
        let settings = image_output(&config).1;
        assert_eq!(
            settings.png.compression,
            vixeeny_image::PngCompression::High
        );
        assert_eq!(settings.png.oxipng_level, Some(6));
        assert!(!settings.webp.lossless);
        assert_eq!(settings.avif.speed, 2);
        assert!((settings.jxl.distance - 1.0).abs() < 1e-4);
        config.image.png.optimize = 0;
        assert_eq!(image_output(&config).1.png.oxipng_level, None);
    }
}
