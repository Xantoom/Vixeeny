// SPDX-License-Identifier: GPL-3.0-or-later
#![windows_subsystem = "windows"]
//! The on-demand side of Vixeeny: the daemon starts it to capture, record or show a window, and
//! it exits after `general.app_idle_exit_seconds` without activity. The same
//! program also runs the settings, the notifications, the recording widget and the updates, each
//! as a process of its own (see the `--` modes of `run`).

use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::Duration;

mod admin;
mod audio_rig;
mod capture_backend;
mod clipboard;
mod cut;
mod probe;
mod record;
mod region;
mod report;
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
    vixeeny_platform::harden_dll_search();
    // The helper processes share the file: `settings`, `probe`, `toast`… say which one runs.
    let program = std::env::args()
        .nth(1)
        .and_then(|a| a.strip_prefix("--").map(str::to_owned))
        .filter(|a| a != "action" && a != "frozen")
        .unwrap_or_else(|| "app".into());
    vixeeny_common::logging::init(
        "vixeeny-app",
        &program,
        log_clock,
        &vixeeny_platform::os_version(),
    );
    if let Some(dir) = vixeeny_common::paths::crash_dir() {
        vixeeny_platform::crash::catch(&dir, "vixeeny-app", &program);
    }
    if let Err(e) = run() {
        tracing::error!("fatal: {e:#}");
        eprintln!("vixeeny-app: {e:#}");
        std::process::exit(1);
    }
}

/// The local time for the log lines.
fn log_clock() -> vixeeny_common::logging::Stamp {
    let t = vixeeny_platform::local_time();
    vixeeny_common::logging::Stamp {
        year: t.year,
        month: t.month,
        day: t.day,
        hour: t.hour,
        minute: t.minute,
        second: t.second,
        millisecond: t.millisecond,
        offset_minutes: vixeeny_platform::utc_offset_minutes(),
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
            vixeeny_capture::freeze::trim_reader();
        }
        ActionId::CaptureScrolling => {
            if let Err(e) = region::run(config, region::Mode::Scroll, frozen) {
                tracing::error!("scrolling capture failed: {e:#}");
            }
            vixeeny_capture::freeze::trim_reader();
        }
        CaptureFullscreen | CaptureWindow | CaptureAllMonitors => {
            match direct_capture(action, config) {
                Ok(None) => tracing::debug!("no window chosen"),
                Ok(Some((path, thumb))) => {
                    tracing::debug!("saved {}", path.display());
                    let saved = toast::Toast::Saved(toast::Saved::Image, path);
                    toast::notify_with(config, &saved, thumb);
                }
                Err(e) => {
                    tracing::error!("capture failed: {e:#}");
                    let lang = lang(&config.general.language);
                    let text = toast::failure_text(&e, lang);
                    toast::notify(config, &toast::Toast::Failed(toast::Failed::Capture, text));
                }
            }
        }
        other => tracing::debug!("action {other:?} is not implemented yet"),
    }
}

/// The window the user clicks among those on screen; `None` when they give up.
fn pick_window(
    monitors: &[vixeeny_platform::MonitorInfo],
    cursor: (i32, i32),
) -> anyhow::Result<Option<vixeeny_platform::WindowInfo>> {
    let bounds =
        vixeeny_platform::virtual_bounds(monitors.iter().map(|m| &m.rect)).context("no screen")?;
    // Those on a screen (a minimized window is far outside). Vixeeny's settings can be taken
    // too; its popups are not listed (tool windows).
    let windows: Vec<vixeeny_platform::WindowInfo> = vixeeny_platform::top_level_windows()?
        .into_iter()
        .filter(|w| w.pid != std::process::id() && w.rect.intersection(&bounds).is_some())
        .collect();
    let rects = windows.iter().map(|w| w.rect).collect();
    let scale = vixeeny_platform::monitor_at(monitors, cursor.0, cursor.1)
        .map_or(1.0, |m| m.scale_factor() as f32);
    let accent = vixeeny_platform::system_accent().unwrap_or([0, 120, 212]);
    let chosen = vixeeny_overlay::picker::pick(rects, bounds, cursor, accent, scale)?;
    Ok(chosen.and_then(|i| windows.into_iter().nth(i)))
}

/// Takes and saves the capture; returns where, with the notification's thumbnail when it is
/// shown. `None`: the user gave up choosing the window.
fn direct_capture(
    action: ActionId,
    config: &Config,
) -> anyhow::Result<Option<(std::path::PathBuf, Option<toast::Thumb>)>> {
    use vixeeny_capture::{CaptureOptions, Capturer};

    vixeeny_platform::ensure_dpi_aware();
    let monitors = vixeeny_platform::monitors()?;
    let cursor = vixeeny_platform::cursor_position()?;
    let foreground = if action == ActionId::CaptureWindow {
        // The user points at the window to capture.
        match pick_window(&monitors, cursor)? {
            Some(window) => Some(window),
            None => return Ok(None),
        }
    } else {
        vixeeny_platform::foreground_window()?
    };
    let started = std::time::Instant::now();
    let snapshot = still::Snapshot {
        monitors,
        cursor,
        foreground,
    };
    let dir = vixeeny_common::paths::ready_dir(
        &config.paths.images,
        vixeeny_common::paths::DEFAULT_IMAGES,
    )
    .context("cannot make the images folder")?;
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
    let mut backend = capture_backend::Lent::take()?;
    let options = CaptureOptions {
        show_cursor: false,
        tonemap: Some(tonemap_hdr),
    };
    let (format, settings) = image_output(config);
    // The notification's thumbnail is made from the picture saved, not from the file.
    let thumb = std::sync::Mutex::new(None);
    let after = |image: &vixeeny_image::Bgra<'_>, encoded: Option<&[u8]>| {
        if config.general.notifications
            && let Ok(mut slot) = thumb.lock()
        {
            *slot = Some(thumbnail::scaled(image, 640));
        }
        if config.image.copy_to_clipboard
            && let Err(e) = clipboard::copy_bgra(config, image, encoded)
        {
            tracing::warn!("clipboard: {e:#}");
        }
    };
    let destination = still::Destination {
        after_save: (config.image.copy_to_clipboard || config.general.notifications)
            .then_some(&after),
        ..destination
    };
    // Asked once the window is captured.
    let backdrop = window_backdrop_kind(config)
        .then_some(|| window_backdrop(config).unwrap_or(vixeeny_image::backdrop::Backdrop::Blur));
    let run = |backend: &mut vixeeny_capture::WgcBackend| {
        let mut capturer = Capturer::new(backend, snapshot.monitors.clone());
        still::run(
            action,
            &snapshot,
            &mut capturer,
            &destination,
            options,
            still::Output {
                format,
                settings: &settings,
                keep_hdr: keep_hdr(config),
                backdrop: backdrop
                    .as_ref()
                    .map(|b| b as &dyn Fn() -> vixeeny_image::backdrop::Backdrop),
            },
            &vixeeny_platform::exe_metadata,
        )
    };
    let mut result = run(&mut backend);
    if backend.kept
        && let Err(still::StillError::Capture(e)) = &result
    {
        // The kept device may be gone (a driver update, the computer asleep): once more with
        // a new one.
        tracing::debug!("capture with the kept device failed ({e}), with a new one");
        backend.renew()?;
        result = run(&mut backend);
    }
    if matches!(result, Err(still::StillError::Capture(_))) {
        backend.discard();
    }
    let path = result?;
    tracing::info!("screenshot taken in {:?}", started.elapsed());
    let thumb = thumb
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok(Some((path, thumb)))
}

/// Whether window captures are set on a background.
fn window_backdrop_kind(config: &Config) -> bool {
    matches!(
        config.image.window_backdrop.as_str(),
        "wallpaper" | "blur" | "colour" | "gradient"
    )
}

/// What a window capture is set on, by the setting: the wallpaper blurred (`blur`, from
/// before, too), a plain colour or a gradient; nothing by default.
fn window_backdrop(config: &Config) -> Option<vixeeny_image::backdrop::Backdrop> {
    use vixeeny_image::backdrop::Backdrop;
    let image = &config.image;
    match image.window_backdrop.as_str() {
        "wallpaper" | "blur" => Some(wallpaper()),
        "colour" => Some(Backdrop::Colour(
            image
                .backdrop_colour()
                .or_else(vixeeny_platform::system_accent)
                .unwrap_or([0, 120, 212]),
        )),
        "gradient" => {
            let (from, to) = image.backdrop_gradient();
            Some(Backdrop::Gradient(from, to))
        }
        _ => None,
    }
}

/// The desktop's wallpaper as a background; the window itself, blurred, when its picture
/// cannot be read (a format not read here, a file gone).
fn wallpaper() -> vixeeny_image::backdrop::Backdrop {
    use vixeeny_image::backdrop::{Backdrop, Picture};
    match vixeeny_platform::wallpaper() {
        vixeeny_platform::Wallpaper::Picture(path) => std::fs::read(&path)
            .ok()
            .and_then(|bytes| Picture::read(&bytes))
            .map_or(Backdrop::Blur, |p| {
                Backdrop::Picture(std::sync::Arc::new(p))
            }),
        vixeeny_platform::Wallpaper::Colour(c) => Backdrop::Colour(c),
    }
}

/// On an HDR screen, the user's choice: keep HDR (`keep_hdr`), or convert to SDR. Formats that
/// cannot hold HDR (WebP) are always converted.
fn keep_hdr(config: &Config) -> bool {
    matches!(config.image.hdr.as_str(), "keep_hdr" | "hdr")
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
    settings.avif.speed = image.avif.speed.min(vixeeny_common::config::AVIF_MAX_SPEED);
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
) -> Option<ActionId> {
    match action {
        ActionId::RecordToggle
        | ActionId::RecordPause
        | ActionId::ReplayToggle
        | ActionId::ReplaySave
        | ActionId::ReplayEdit => recording.handle(action, config),
        ActionId::ReplayWatch => recording.watch(config),
        ActionId::OverlayToggle => return overlay(config, recording),
        ActionId::OpenSettings => open_settings(),
        _ => perform(action, config, frozen),
    }
    REOPEN_STRIP.take().then_some(ActionId::OverlayToggle)
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
        let mut next = run_action(action, frozen, self.config, self.recording);
        while let Some(action) = next {
            next = run_action(action, None, self.config, self.recording);
        }
        BUSY.store(false, Ordering::Release);
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
    // The strip is gone once the loop ends, but the screen must show it gone before a capture
    // freezes it: two compositions (the one under way may still have it).
    if outcome.action.is_some() {
        vixeeny_platform::wait_for_composition();
        vixeeny_platform::wait_for_composition();
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
    /// The game (its process and executable) the replay was started for: the replay stops when
    /// it closes.
    replay_game: Option<(u32, String)>,
    /// A game whose replay the user stopped by hand: it is not started again for it.
    declined: Option<u32>,
    /// The games announced while they run, the replay on or off: each is told once per run
    /// of the game, and again only when the replay changed.
    announced: Vec<(u32, bool)>,
    /// Tells games from other full-screen programs.
    games: vixeeny_platform::games::Detector,
    /// Asks the main loop to look at the foreground again (the game closed).
    look: Option<Sender<Wake>>,
}

/// What wakes the main loop.
enum Wake {
    Daemon(DaemonToApp),
    /// The game of the replay closed.
    Look,
    /// A recording paused, resumed or ended.
    Recording,
}

impl Recording {
    /// `(recording, replay buffer)` is running.
    fn flags(&self) -> (bool, bool) {
        (self.handle.is_some(), self.replay.is_some())
    }

    /// Called when the daemon saw a game come to the foreground, when the replay settings
    /// changed, and when the replay's game closed. With the replay on: starts it for the game
    /// in front, stops it when that game closes (or the replay was turned off). Tells about
    /// each game in a notification, the replay on or off.
    fn watch(&mut self, config: &Config) {
        // The capture ended by itself (the game's window went, it may have made another): the
        // replay starts again with the game in front.
        if self.replay.as_ref().is_some_and(record::Handle::finished) {
            tracing::info!("the replay's capture ended");
            self.replay = None;
            self.replay_game = None;
        }
        if let Some((pid, exe)) = &self.replay_game {
            let never = config.replay.never.contains(exe);
            if !config.replay.enabled || never || !vixeeny_platform::process_alive(*pid) {
                tracing::info!("the game closed (or the replay was turned off): replay stopped");
                if never {
                    self.declined = Some(*pid);
                }
                self.replay_game = None;
                self.stop_replay();
            }
            return;
        }
        // Turned off in the settings: the buffer goes, its memory or files with it.
        if !config.replay.enabled {
            self.stop_replay();
        }
        let Some(game) = self.games.game_in_front(game_lists(config)) else {
            return;
        };
        if !config.replay.enabled {
            if config.replay.notify == "both" && self.first_time(&game, false) {
                announce(config, false, &game);
            }
            return;
        }
        let declined = self
            .declined
            .is_some_and(|pid| vixeeny_platform::games::part_of_game(game.pid, pid));
        if self.replay.is_some() || declined {
            return;
        }
        if self.start_replay(config, &game)
            && config.replay.notify != "none"
            && self.first_time(&game, true)
        {
            announce(config, true, &game);
        }
    }

    /// Whether `game` (or the game it is part of) is not yet announced in this run of it with
    /// the replay `on` (or off).
    fn first_time(&mut self, game: &vixeeny_platform::WindowInfo, on: bool) -> bool {
        self.announced
            .retain(|(pid, _)| vixeeny_platform::process_alive(*pid));
        let known = self
            .announced
            .iter_mut()
            .find(|(pid, _)| vixeeny_platform::games::part_of_game(game.pid, *pid));
        match known {
            Some((_, told)) if *told == on => false,
            Some((_, told)) => {
                *told = on;
                true
            }
            None => {
                self.announced.push((game.pid, on));
                true
            }
        }
    }

    /// Starts the replay for `game`; `false` (logged) when it cannot.
    fn start_replay(&mut self, config: &Config, game: &vixeeny_platform::WindowInfo) -> bool {
        match record::start_replay(config, game) {
            Ok(handle) => {
                // The program, not the window title (which may be personal).
                let exe = game
                    .exe_path
                    .as_deref()
                    .and_then(vixeeny_platform::games::exe_name)
                    .unwrap_or_default();
                tracing::info!("full-screen game ({exe}): replay started");
                self.replay = Some(handle);
                self.replay_game = Some((game.pid, exe));
                self.declined = None;
                if let Some(look) = self.look.clone() {
                    vixeeny_platform::on_process_exit(game.pid, move || {
                        let _ = look.send(Wake::Look);
                    });
                }
                true
            }
            Err(e) => {
                tracing::error!("cannot start the replay buffer: {e:#}");
                // Not again for this game.
                self.declined = Some(game.pid);
                false
            }
        }
    }

    /// Stops the buffer: its memory is freed, its files on disk removed.
    fn stop_replay(&mut self) {
        if let Some(replay) = self.replay.take() {
            drop(replay);
            tracing::info!("replay stopped, its buffer released");
        }
    }

    fn handle(&mut self, action: ActionId, config: &Config) {
        match action {
            // During a game only: stops the replay (it stays off for that game), or starts it
            // again.
            ActionId::ReplayToggle => {
                if self.replay.is_some() {
                    let game = self.replay_game.take();
                    self.declined = game.as_ref().map(|(pid, _)| *pid);
                    self.stop_replay();
                    if config.replay.notify != "none"
                        && let Some(window) = vixeeny_platform::foreground_window().ok().flatten()
                        && game.is_some_and(|(pid, _)| pid == window.pid)
                    {
                        announce(config, false, &window);
                    }
                } else if let Some(game) = self.games.game_in_front(game_lists(config)) {
                    if self.start_replay(config, &game) && config.replay.notify != "none" {
                        announce(config, true, &game);
                    }
                } else {
                    tracing::debug!("replay toggle ignored: no full-screen game");
                }
                return;
            }
            ActionId::ReplaySave | ActionId::ReplayEdit => {
                match &self.replay {
                    Some(replay) if action == ActionId::ReplayEdit => replay.save_and_edit(),
                    Some(replay) => replay.save(),
                    None => tracing::debug!("replay save ignored: the buffer is not running"),
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

/// The notification that a game started, the replay running with it or not.
fn announce(config: &Config, on: bool, game: &vixeeny_platform::WindowInfo) {
    if let Some(exe) = &game.exe_path {
        let name = vixeeny_platform::games::display_name(exe, &game.title);
        toast::notify(config, &toast::Toast::Replay(on, exe.into(), name));
    }
}

/// The user's word on which programs are games.
fn game_lists(config: &Config) -> vixeeny_platform::games::Lists<'_> {
    vixeeny_platform::games::Lists {
        always: &config.replay.always,
        never: &config.replay.never,
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
        let id = vixeeny_platform::WindowId(handle);
        // Slint's icon does not reach the taskbar: the executable's own does.
        vixeeny_platform::set_exe_icon(id);
        let _ = vixeeny_platform::style_window(id, look.dark, look.caption());
        vixeeny_platform::mica(id)
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
        // Ahead of time, nobody waiting: it gives way to everything else.
        Some("--warm-probe") => {
            vixeeny_platform::run_in_background();
            return probe::warm();
        }
        // The recording widget, a process of its own (see `widget`).
        Some("--widget") => return widget::run_child(&args[1..]),
        // The settings window and gallery, a process of its own (see `settings`).
        Some("--settings") => return settings::run_child(&args[1..]),
        // A notification card (see `toast`).
        Some("--toast") => return toast::run_child(&args[1..]),
        Some("--toast-host") => return toast::run_host(),
        // The video editor of the replays (see `cut`).
        Some("--cut") => return cut::run_child(&args[1..]),
        // A saved picture in the zone editor (see `region`).
        Some("--edit") => return region::run_edit(&args[1..]),
        // The daily update check, and the installation (see `update`).
        Some("--update") => return update::run_child(&args[1..]),
        // The administrator's rights (see `admin`).
        Some("--admin") => return admin::run_child(&args[1..]),
        Some("--system-info") => {
            vixeeny_platform::attach_console();
            // The displays in physical pixels, as the settings window reports them.
            vixeeny_platform::ensure_dpi_aware();
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
    // What the first action needs from the graphics card is made while the app starts: the
    // device reading the frozen screens, the capture device.
    // The editor draws with the device reading the frozen screens; without them it makes its
    // own, ahead too.
    if let Some(frozen) = &first_frozen {
        vixeeny_capture::freeze::warm(frozen);
    } else if matches!(
        first_action,
        ActionId::CaptureRegion | ActionId::CaptureScrolling | ActionId::OverlayToggle
    ) {
        vixeeny_overlay::gfx::warm();
    }
    if matches!(
        first_action,
        ActionId::CaptureFullscreen
            | ActionId::CaptureWindow
            | ActionId::CaptureAllMonitors
            | ActionId::CaptureScrolling
    ) {
        capture_backend::warm();
    }
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
                // The overlay's shortcut again while the strip is open: it closes, nothing
                // waits to open it once more.
                if let DaemonToApp::RunAction {
                    action: ActionId::OverlayToggle,
                    frozen,
                } = &mut msg
                    && side::dismiss_open()
                {
                    if let Some(frozen) = frozen.take() {
                        vixeeny_capture::freeze::release(&frozen);
                    }
                    continue;
                }
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
    {
        let wake = look.clone();
        record::on_change(move || {
            let _ = wake.send(Wake::Recording);
        });
    }
    // Dropped last, whatever ends the app: the replay files being written are completed first.
    let _saves = record::SavesDone;
    let mut recording = Recording {
        look: Some(look),
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
    let mut idle_from = std::time::Instant::now();
    loop {
        // While recording, or while the replay runs with a game, the app stays: it sleeps until
        // it is told something (the daemon, the recording, Windows). Otherwise it ends once
        // idle for a while; the daemon, which watches for games, starts it again with one.
        let busy = recording.handle.is_some() || recording.replay.is_some() || record::saving();
        let woken = if busy {
            idle_from = std::time::Instant::now();
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(idle.saturating_sub(idle_from.elapsed()))
        };
        match woken {
            Ok(Wake::Daemon(DaemonToApp::RunAction { action, frozen })) => {
                idle_from = std::time::Instant::now();
                Actions {
                    config: &mut config,
                    recording: &mut recording,
                    send: &mut send,
                }
                .run(action, frozen)?;
                recording.report(&mut send)?;
            }
            Ok(Wake::Look) => recording.watch(&config),
            Ok(Wake::Recording) => {
                recording.report(&mut send)?;
                if recording
                    .replay
                    .as_ref()
                    .is_some_and(record::Handle::finished)
                {
                    recording.watch(&config);
                }
            }
            Ok(Wake::Daemon(DaemonToApp::ConfigChanged)) => {}
            // The daemon asked us to stop, or went away.
            Ok(Wake::Daemon(DaemonToApp::Shutdown)) | Err(RecvTimeoutError::Disconnected) => {
                return Ok(());
            }
            // Idle for long enough: the memory goes back to the system.
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
        let native = cfg!(feature = "native-codecs");
        // JPEG, by default and when asked; PNG in a build without it.
        let jpeg = if native {
            ImageFormat::Jpeg
        } else {
            ImageFormat::Png
        };
        let mut config = Config::default();
        assert_eq!(image_output(&config).0, jpeg);
        config.image.format = "jpeg".into();
        config.image.jpeg.quality = 70;
        let (format, settings) = image_output(&config);
        assert_eq!(format, jpeg);
        assert_eq!(
            (settings.jpeg.quality, settings.jpeg.chroma),
            (70, Chroma::Yuv444)
        );
        config.image.format = "bmp".into(); // unknown
        assert_eq!(image_output(&config).0, ImageFormat::Png);
        config.image.format = "avif".into();
        assert_eq!(image_output(&config).0, ImageFormat::Avif);
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
