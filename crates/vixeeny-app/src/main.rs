// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg_attr(windows, windows_subsystem = "windows")]
//! On-demand Vixeeny app. M2: only the IPC client side exists; the UI arrives with M3+.
//! The app connects to the daemon, announces itself and exits after
//! `general.app_idle_exit_seconds` without activity (plan section 1.2).

use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

#[cfg(all(any(windows, target_os = "macos"), feature = "ffmpeg"))]
mod audio_rig;
#[cfg(any(windows, target_os = "macos"))]
mod clipboard;
#[cfg(any(windows, target_os = "macos"))]
mod convert;
#[cfg(any(windows, target_os = "macos", test))]
mod gallery;
#[cfg(any(windows, target_os = "macos"))]
mod ocr;
mod probe;
#[cfg(all(any(windows, target_os = "macos"), feature = "ffmpeg"))]
mod record;
#[cfg(any(windows, target_os = "macos"))]
mod region;
#[cfg(windows)]
mod scroll;
#[cfg(any(windows, target_os = "macos"))]
mod settings;
#[cfg(any(windows, target_os = "macos"))]
mod side;
#[cfg(any(windows, target_os = "macos", test))]
mod still;
mod toast;
#[cfg(windows)]
mod widget;
#[cfg(target_os = "macos")]
#[path = "widget_mac.rs"]
mod widget;
#[cfg(any(windows, target_os = "macos", test))]
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

/// Runs one action. Failures are logged, not fatal: the app stays available for the next one.
fn perform(action: ActionId, config: &Config) {
    use ActionId::{CaptureAllMonitors, CaptureFullscreen, CaptureWindow};
    match action {
        #[cfg(any(windows, target_os = "macos"))]
        ActionId::CaptureRegion => {
            if let Err(e) = region::run(config, region::Mode::Editor) {
                tracing::error!("editor failed: {e:#}");
            }
        }
        #[cfg(windows)]
        ActionId::CaptureScrolling => {
            if let Err(e) = region::run(config, region::Mode::Scroll) {
                tracing::error!("scrolling capture failed: {e:#}");
            }
        }
        #[cfg(any(windows, target_os = "macos"))]
        ActionId::OcrRegion => {
            if let Err(e) = region::run(config, region::Mode::Ocr) {
                tracing::error!("OCR failed: {e:#}");
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

#[cfg(any(windows, target_os = "macos"))]
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
        template: &config.paths.filename_template,
        per_app_subfolder: config.paths.per_app_subfolder.images,
        use_foreground_app: config.paths.use_foreground_app,
        app_names: &config.paths.app_names,
        now: &now,
        after_save: None,
    };
    #[cfg(windows)]
    let backend = vixeeny_capture::WgcBackend::new()?;
    #[cfg(target_os = "macos")]
    let backend = vixeeny_capture::SckBackend::new()?;
    let mut capturer = Capturer::new(backend, snapshot.monitors.clone());
    let options = CaptureOptions {
        show_cursor: false,
        // macOS captures are SDR for now.
        #[cfg(windows)]
        tonemap: (config.image.hdr == "tonemap_sdr")
            .then_some(tonemap_hdr as vixeeny_capture::ToneMapFn),
        #[cfg(not(windows))]
        tonemap: None,
    };
    let (format, settings) = image_output(config);
    let copy = |image: &vixeeny_image::Bgra<'_>| {
        if let Err(e) = clipboard::copy_bgra(image) {
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
#[cfg(windows)]
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
#[cfg(any(windows, target_os = "macos", test))]
fn image_output(config: &Config) -> (vixeeny_image::ImageFormat, vixeeny_image::Settings) {
    use vixeeny_image::{Chroma, ImageFormat, Settings};
    let mut settings = Settings::default();
    settings.jpeg.quality = config.image.jpeg.quality.clamp(1, 100);
    if let Some(chroma) = Chroma::from_name(&config.image.jpeg.chroma) {
        settings.jpeg.chroma = chroma;
    }
    settings.avif.quality = config.image.avif.quality.min(100);
    settings.avif.depth = if config.image.avif.depth >= 10 { 10 } else { 8 };
    if let Some(chroma) = Chroma::from_name(&config.image.avif.chroma) {
        settings.avif.chroma = chroma;
    }
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

#[cfg(not(any(windows, target_os = "macos")))]
fn direct_capture(_: ActionId, _: &Config) -> anyhow::Result<std::path::PathBuf> {
    anyhow::bail!("screen capture is not supported on this platform yet")
}

/// `--action <name>` (default: open the settings).
fn parse_action() -> anyhow::Result<ActionId> {
    let mut args = std::env::args().skip(1);
    let mut action = ActionId::OpenSettings;
    while let Some(arg) = args.next() {
        if arg == "--action" {
            let name = args.next().context("--action needs a value")?;
            action = ActionId::from_cli_name(&name)
                .with_context(|| format!("unknown action `{name}`"))?;
        }
    }
    Ok(action)
}

/// Runs one action. Returns a follow-up action when the action was the overlay and the user
/// picked something in it.
fn run_action(
    action: ActionId,
    config: &mut Config,
    recording: &mut Recording,
    send: &mut impl std::io::Write,
) -> anyhow::Result<Option<ActionId>> {
    match action {
        ActionId::RecordToggle
        | ActionId::RecordPause
        | ActionId::ReplayToggle
        | ActionId::ReplaySave => recording.handle(action, config),
        ActionId::OverlayToggle => return overlay(config, recording, send),
        ActionId::OpenSettings => open_settings(),
        _ => perform(action, config),
    }
    Ok(None)
}

/// The settings window is a process of its own: the hotkeys keep working while it is open.
fn open_settings() {
    #[cfg(any(windows, target_os = "macos"))]
    if let Err(e) = settings::spawn() {
        tracing::error!("{e:#}");
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    tracing::info!("the settings window needs Windows or macOS");
}

#[cfg(any(windows, target_os = "macos"))]
fn overlay(
    config: &mut Config,
    recording: &Recording,
    send: &mut impl std::io::Write,
) -> anyhow::Result<Option<ActionId>> {
    let (recording, replay) = recording.flags();
    let outcome = match side::run(config, recording, replay) {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!("overlay failed: {e:#}");
            return Ok(None);
        }
    };
    if let Some(profile) = outcome.profile {
        config.video.profile = profile;
        match vixeeny_common::paths::config_file() {
            Some(path) => match config.save(&path) {
                Ok(()) => ipc::write_msg(send, &AppToDaemon::ConfigChanged)?,
                Err(e) => tracing::error!("cannot save the profile choice: {e}"),
            },
            None => tracing::error!("no settings folder: the profile choice is not saved"),
        }
    }
    // The strip is gone once the loop ends, but give the compositor a moment to repaint before
    // a capture freezes the screen: the strip must not be in it.
    if outcome.action.is_some() {
        std::thread::sleep(Duration::from_millis(120));
    }
    Ok(outcome.action)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn overlay(
    _: &mut Config,
    _: &Recording,
    _: &mut impl std::io::Write,
) -> anyhow::Result<Option<ActionId>> {
    tracing::info!("the overlay needs Windows");
    Ok(None)
}

/// The recording started by `RecordToggle`, if any (Windows with FFmpeg only).
#[derive(Default)]
struct Recording {
    #[cfg(all(any(windows, target_os = "macos"), feature = "ffmpeg"))]
    handle: Option<record::Handle>,
    /// The replay buffer (`ReplayToggle`), which runs alongside a recording.
    #[cfg(all(any(windows, target_os = "macos"), feature = "ffmpeg"))]
    replay: Option<record::Handle>,
}

impl Recording {
    /// `(recording, replay buffer)` is running.
    #[cfg(all(any(windows, target_os = "macos"), feature = "ffmpeg"))]
    fn flags(&self) -> (bool, bool) {
        (self.handle.is_some(), self.replay.is_some())
    }

    #[cfg(not(all(any(windows, target_os = "macos"), feature = "ffmpeg")))]
    #[allow(clippy::unused_self, dead_code)]
    fn flags(&self) -> (bool, bool) {
        (false, false)
    }

    #[cfg(all(any(windows, target_os = "macos"), feature = "ffmpeg"))]
    fn active(&self) -> bool {
        self.handle.is_some() || self.replay.is_some()
    }

    #[cfg(not(all(any(windows, target_os = "macos"), feature = "ffmpeg")))]
    #[allow(clippy::unused_self)]
    fn active(&self) -> bool {
        false
    }

    #[cfg(all(any(windows, target_os = "macos"), feature = "ffmpeg"))]
    fn handle(&mut self, action: ActionId, config: &Config) {
        match action {
            ActionId::ReplayToggle => {
                match self.replay.take() {
                    // Dropping the handle stops the buffer.
                    Some(replay) => drop(replay),
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

    #[cfg(not(all(any(windows, target_os = "macos"), feature = "ffmpeg")))]
    #[allow(clippy::unused_self)]
    fn handle(&mut self, action: ActionId, _: &Config) {
        tracing::info!("action {action:?}: recording needs Windows and the `ffmpeg` feature");
    }

    /// Tells the daemon (tray icon) when the state changed, and forgets a finished recording.
    #[cfg(all(any(windows, target_os = "macos"), feature = "ffmpeg"))]
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

    #[cfg(not(all(any(windows, target_os = "macos"), feature = "ffmpeg")))]
    #[allow(clippy::unused_self)]
    fn report(&mut self, _: &mut impl std::io::Write) -> anyhow::Result<()> {
        Ok(())
    }
}

fn run() -> anyhow::Result<()> {
    // `--convert <paths…>`: the conversion window on its own, no daemon needed (it is what the
    // Explorer context-menu entry starts).
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // The probe child: prints TOML on stdout (see `vixeeny_encode::probe::run_child`).
        Some("--probe") => return probe::child(),
        // The recording widget, a process of its own (see `widget`).
        #[cfg(windows)]
        Some("--widget") => return widget::run_child(&args[1..]),
        // The settings window and gallery, a process of its own (see `settings`).
        #[cfg(any(windows, target_os = "macos"))]
        Some("--settings") => return settings::run_child(),
        // The installer adds or removes the Explorer entry.
        #[cfg(any(windows, target_os = "macos"))]
        Some(flag @ ("--install-menu" | "--uninstall-menu")) => {
            return settings::context_menu(flag == "--install-menu");
        }
        // A notification card (see `toast`).
        #[cfg(windows)]
        Some("--toast") => return toast::run_child(&args[1..]),
        // The daemon announces an update with a native notification.
        #[cfg(windows)]
        Some("--update-toast") => {
            return toast::update_toast(args.get(1).map_or("", String::as_str));
        }
        // A click on a native notification (`vixeeny://settings`).
        #[cfg(windows)]
        Some("--uri") => {
            if let Some(uri) = args.get(1) {
                toast::open_uri(uri);
            }
            return Ok(());
        }
        Some("--probe-report") => return probe::report(args.iter().any(|a| a == "--force")),
        _ => {}
    }
    // The Explorer context-menu entry (until the settings app offers the switch).
    #[cfg(any(windows, target_os = "macos"))]
    if let Some(flag) = args
        .first()
        .filter(|a| a.starts_with("--") && a.ends_with("-context-menu"))
    {
        let config = vixeeny_common::paths::config_file()
            .and_then(|path| Config::load(&path).ok())
            .unwrap_or_default();
        let lang = lang(&config.general.language);
        let result = if flag == "--install-context-menu" {
            let label = vixeeny_common::i18n::tr(vixeeny_common::i18n::Key::ConvMenuLabel, lang);
            vixeeny_platform::context_menu::install(&std::env::current_exe()?, label)
        } else {
            vixeeny_platform::context_menu::uninstall()
        };
        return result.map_err(|e| anyhow::anyhow!("{e}"));
    }
    if args.first().is_some_and(|a| a == "--convert") {
        let config = vixeeny_common::paths::config_file()
            .and_then(|path| Config::load(&path).ok())
            .unwrap_or_default();
        #[cfg(any(windows, target_os = "macos"))]
        {
            let paths: Vec<std::path::PathBuf> = args[1..].iter().map(Into::into).collect();
            return convert::run(&config, &paths);
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = config;
            anyhow::bail!("the conversion window is not supported on this platform yet");
        }
    }
    let first_action = parse_action()?;
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
    std::thread::Builder::new()
        .name("ipc-read".into())
        .spawn(move || {
            while let Ok(Some(msg)) = ipc::read_msg::<_, DaemonToApp>(&mut recv) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        })?;

    eprintln!(
        "vixeeny-app {} started for {first_action:?}",
        env!("CARGO_PKG_VERSION")
    );
    let mut recording = Recording::default();
    loop {
        // While recording the app must not exit as idle; it polls the recording state instead.
        let wait = if recording.active() {
            Duration::from_millis(200)
        } else {
            idle
        };
        match rx.recv_timeout(wait) {
            Ok(DaemonToApp::RunAction { action, .. }) => {
                // The overlay hands back the action the user picked in it.
                let mut next = Some(action);
                while let Some(action) = next.take() {
                    next = run_action(action, &mut config, &mut recording, &mut send)?;
                }
                recording.report(&mut send)?;
            }
            Err(RecvTimeoutError::Timeout) if recording.active() => {
                recording.report(&mut send)?;
            }
            Ok(DaemonToApp::ConfigChanged) => {}
            // The daemon asked us to stop, or went away.
            Ok(DaemonToApp::Shutdown) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {
                ipc::write_msg(&mut send, &AppToDaemon::Idle)?;
                return Ok(());
            }
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
        config.image.jpeg.chroma = "420".into();
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
            (70, Chroma::Yuv420)
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
    }
}
