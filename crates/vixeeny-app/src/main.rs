// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg_attr(windows, windows_subsystem = "windows")]
//! On-demand Vixeeny app. M2: only the IPC client side exists; the UI arrives with M3+.
//! The app connects to the daemon, announces itself and exits after
//! `general.app_idle_exit_seconds` without activity (plan section 1.2).

use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

#[cfg(any(windows, test))]
mod still;

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

/// Runs one action. Failures are logged, not fatal: the app stays available for the next one.
fn perform(action: ActionId, config: &Config) {
    use ActionId::{CaptureAllMonitors, CaptureFullscreen, CaptureWindow};
    match action {
        CaptureFullscreen | CaptureWindow | CaptureAllMonitors => {
            match direct_capture(action, config) {
                Ok(path) => tracing::info!("saved {}", path.display()),
                Err(e) => tracing::error!("capture failed: {e:#}"),
            }
        }
        other => tracing::info!("action {other:?} is not implemented yet"),
    }
}

#[cfg(windows)]
fn direct_capture(action: ActionId, config: &Config) -> anyhow::Result<std::path::PathBuf> {
    use vixeeny_capture::{CaptureOptions, Capturer, WgcBackend};

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
    };
    let mut capturer = Capturer::new(WgcBackend::new()?, snapshot.monitors.clone());
    let options = CaptureOptions {
        show_cursor: false,
        tonemap: (config.image.hdr == "tonemap_sdr")
            .then_some(tonemap_hdr as vixeeny_capture::ToneMapFn),
    };
    let (format, settings) = image_output(config);
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
#[cfg(any(windows, test))]
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

#[cfg(not(windows))]
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

fn run() -> anyhow::Result<()> {
    let first_action = parse_action()?;
    let config = vixeeny_common::paths::config_file()
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
    loop {
        match rx.recv_timeout(idle) {
            Ok(DaemonToApp::RunAction { action, .. }) => {
                perform(action, &config);
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
