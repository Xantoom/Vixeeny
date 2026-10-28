// SPDX-License-Identifier: GPL-3.0-or-later
//! Executes the effects decided by [`Core`]: talks to the tray, the app and the OS.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use vixeeny_common::config::Config;
use vixeeny_common::hotkey::{self, Hotkey, ProblemKind};
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::{ActionId, ControlRequest, DaemonToApp, Frozen, RecState};

use crate::autostart;
use crate::core::{Core, Effect, Event, SpawnId};
use crate::server::{AppLink, EventTx};
use crate::supervisor::Spawner;

/// The daemon watches for games: to have the app run the replay with them, or tell that it is off.
fn watches_games(config: &Config) -> bool {
    config.replay.enabled || config.notifications.game_start(false)
}

/// Finds games in the foreground.
pub trait Games {
    /// Has Windows send [`Event::Foreground`] through `tx` at each change in the foreground; `false` when it refused.
    fn hook(&mut self, tx: EventTx) -> bool;
    /// The process of the full-screen game in `window` (`None`: the foreground now), if it is one.
    fn game(&mut self, window: Option<u64>, config: &Config) -> Option<u32>;
    /// Whether process `pid` is part of the game running as process `game` (its in-game web pages, in a window of
    /// another process).
    fn same_game(&self, pid: u32, game: u32) -> bool {
        pid == game
    }
}

/// No game detection (tests, or a system without it).
pub struct NoGames;

impl Games for NoGames {
    fn hook(&mut self, _: EventTx) -> bool {
        false
    }
    fn game(&mut self, _: Option<u64>, _: &Config) -> Option<u32> {
        None
    }
}

/// What the daemon shows in the OS notification area.
pub trait Tray {
    fn set_recording(&mut self, state: RecState, lang: Lang);
    fn set_language(&mut self, lang: Lang);
    /// Shows a short message to the user.
    fn notify(&mut self, message: &str);
}

/// Registers the global shortcuts with the OS. A shortcut press becomes an [`Event::Action`] sent by the backend
/// itself.
pub trait HotkeyBackend {
    /// Replaces the registered shortcuts by `bindings`. Returns the shortcuts the OS refused (typically because another
    /// application owns them), each with the reason.
    fn apply(&mut self, bindings: &[(ActionId, Hotkey)]) -> Vec<(Hotkey, String)>;
}

/// Freezes the screens the moment a zone capture is asked for (see `vixeeny_capture::freeze`); the app takes over from
/// the frozen screens.
pub trait Freezer {
    /// `None` when the screens cannot be frozen, or already are.
    fn freeze(&mut self, _cursor: bool) -> Option<Frozen>;
    /// The screens captured for the app without being shown (with the pointer when `cursor`), as
    /// [`freeze`](Self::freeze) otherwise.
    fn hold(&mut self, _cursor: bool) -> Option<Frozen> {
        None
    }
    fn thaw(&mut self);
}

/// No freezing: the app captures the screens itself.
pub struct NoFreeze;

impl Freezer for NoFreeze {
    fn freeze(&mut self, _cursor: bool) -> Option<Frozen> {
        None
    }
    fn thaw(&mut self) {}
}

/// Who received the frozen screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrozenFor {
    /// The connected app, over IPC.
    Link,
    /// An app started for the press.
    Spawn(SpawnId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

pub struct Runtime<T: Tray, S: Spawner> {
    core: Core,
    tray: T,
    hotkeys: Box<dyn HotkeyBackend>,
    spawner: S,
    link: AppLink,
    tx: EventTx,
    rx: Receiver<Event>,
    config: Config,
    config_path: Option<PathBuf>,
    os_locale: Option<String>,
    lang: Lang,
    freezer: Box<dyn Freezer>,
    /// Frozen screens not handed to the app yet.
    frozen: Option<Frozen>,
    frozen_for: Option<FrozenFor>,
    games: Box<dyn Games>,
    /// The foreground is followed (the hook is installed).
    hooked: bool,
    /// The last game the app was told about: once per game.
    last_game: Option<u32>,
    /// The process of that game last seen in front (the game, or another process of it).
    front: Option<u32>,
}

impl<T: Tray, S: Spawner> Runtime<T, S> {
    #[expect(clippy::too_many_arguments, reason = "one call site, which hands over every part of the daemon")]
    pub fn new(
        config: Config,
        config_path: Option<PathBuf>,
        os_locale: Option<String>,
        tray: T,
        hotkeys: Box<dyn HotkeyBackend>,
        spawner: S,
        link: AppLink,
        tx: EventTx,
        rx: Receiver<Event>,
    ) -> Self {
        let lang = Lang::resolve(&config.general.language, os_locale.as_deref());
        Self {
            core: Core::new(),
            tray,
            hotkeys,
            spawner,
            link,
            tx,
            rx,
            config,
            config_path,
            os_locale,
            lang,
            freezer: Box::new(NoFreeze),
            frozen: None,
            frozen_for: None,
            games: Box::new(NoGames),
            hooked: false,
            last_game: None,
            front: None,
        }
    }

    #[must_use]
    pub fn with_games(mut self, games: Box<dyn Games>) -> Self {
        self.games = games;
        self
    }

    #[must_use]
    pub fn with_freezer(mut self, freezer: Box<dyn Freezer>) -> Self {
        self.freezer = freezer;
        self
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// Applies settings that live outside the process (OS autostart).
    pub fn apply_config(&mut self) {
        self.apply_autostart();
        self.apply_hotkeys();
    }

    fn apply_autostart(&self) {
        if let Err(e) = autostart::apply(self.config.general.autostart) {
            tracing::warn!("cannot update autostart: {e}");
        }
    }

    /// Starts watching for full-screen games if the replay is on, or to say it is off: the app is started with a game
    /// (the replay runs, or it says it is off), not before.
    pub fn start_replay_if_configured(&mut self) {
        self.look_for_game(None);
    }

    /// With games watched: tells the app (starting it) about a game newly in the foreground. `true` when it did.
    fn look_for_game(&mut self, window: Option<u64>) -> bool {
        if !watches_games(&self.config) {
            return false;
        }
        if !self.hooked {
            self.hooked = self.games.hook(self.tx.clone());
        }
        match self.games.game(window, &self.config) {
            Some(pid) if self.last_game.is_none_or(|game| !self.games.same_game(pid, game)) => {
                self.last_game = Some(pid);
                self.front = Some(pid);
                self.dispatch(Event::Action(ActionId::ReplayWatch));
                true
            }
            // Another process of the same game (the game a launcher started, the game's web pages): a running app sees
            // whether its replay follows it.
            Some(pid) if self.front != Some(pid) => {
                self.front = Some(pid);
                if self.core.app_running() {
                    self.dispatch(Event::Action(ActionId::ReplayWatch));
                }
                false
            }
            _ => false,
        }
    }

    fn apply_hotkeys(&mut self) {
        let resolution = hotkey::resolve(&self.config.hotkeys);
        let mut trouble = !resolution.problems.is_empty();
        for problem in &resolution.problems {
            let (action, text) = (problem.action, &problem.text);
            match &problem.kind {
                ProblemKind::Invalid(e) => tracing::warn!("shortcut `{text}` for {action:?}: {e}"),
                ProblemKind::TooMany => {
                    tracing::warn!("shortcut `{text}` for {action:?} ignored: at most 2 per action");
                }
                ProblemKind::Duplicate => {
                    tracing::warn!("shortcut `{text}` listed twice for {action:?}");
                }
                ProblemKind::Conflict(other) => {
                    tracing::warn!("shortcut `{text}` for {action:?} conflicts with {other:?}");
                }
            }
        }
        let mut bindings = resolution.bindings;
        // The side bar turned off: its shortcut stays free.
        if !self.config.overlay.enabled {
            bindings.retain(|(action, _)| *action != ActionId::OverlayToggle);
        }
        let failures = self.hotkeys.apply(&bindings);
        for (hotkey, failure) in &failures {
            tracing::warn!("shortcut {hotkey} not registered: {failure}");
            trouble = true;
        }
        write_taken(&failures);
        if trouble {
            self.execute(Effect::Notify(Key::HotkeysUnavailable));
        }
    }

    /// Handles every queued event. Called when the platform loop was woken.
    pub fn pump(&mut self) -> Flow {
        let mut flow = Flow::Continue;
        while let Ok(event) = self.rx.try_recv() {
            if self.dispatch(event) == Flow::Quit {
                flow = Flow::Quit;
            }
        }
        flow
    }

    fn dispatch(&mut self, event: Event) -> Flow {
        tracing::debug!("event: {event:?}");
        if let Event::Foreground(window) = event {
            self.look_for_game(Some(window));
            return Flow::Continue;
        }
        self.freeze_or_thaw(&event);
        let mut flow = Flow::Continue;
        for effect in self.core.handle(event) {
            if self.execute(effect) == Flow::Quit {
                flow = Flow::Quit;
            }
        }
        flow
    }

    /// The screens freeze before anything else happens for a zone capture, and are held for a full-screen one; they go
    /// when the app that should take over from them is gone (the app itself removes them otherwise).
    fn freeze_or_thaw(&mut self, event: &Event) {
        match *event {
            Event::Action(action) | Event::Control(ControlRequest::Action(action)) if action.holds_screen() => {
                // Screens of an earlier press no action took: not this one's.
                if self.frozen.is_some() && self.frozen_for.is_none() {
                    self.thaw();
                }
                if let Some(frozen) = self.freezer.hold(self.config.image.for_action(action).show_cursor) {
                    self.frozen = Some(frozen);
                    self.frozen_for = None;
                }
            }
            Event::Action(action) | Event::Control(ControlRequest::Action(action)) if action.freezes_screen() => {
                // `None` while earlier frozen screens are still up: they serve this press.
                if let Some(frozen) = self.freezer.freeze(self.config.image.for_action(action).show_cursor) {
                    self.frozen = Some(frozen);
                    self.frozen_for = None;
                }
            }
            Event::AppDisconnected if self.frozen_for == Some(FrozenFor::Link) => self.thaw(),
            Event::AppExited { id, .. } if self.frozen_for == Some(FrozenFor::Spawn(id)) => {
                self.thaw();
            }
            _ => {}
        }
    }

    fn thaw(&mut self) {
        self.freezer.thaw();
        self.frozen = None;
        self.frozen_for = None;
    }

    /// The frozen screens, for the app that runs `action`.
    fn take_frozen(&mut self, action: ActionId, receiver: FrozenFor) -> Option<Frozen> {
        let frozen = self.frozen.take().filter(|_| action.freezes_screen() || action.holds_screen());
        if frozen.is_some() {
            self.frozen_for = Some(receiver);
        }
        frozen
    }

    fn execute(&mut self, effect: Effect) -> Flow {
        match effect {
            Effect::SpawnApp { id, action } => {
                vixeeny_platform::allow_foreground_handoff();
                let frozen = self.take_frozen(action, FrozenFor::Spawn(id));
                if let Err(e) = self.spawner.spawn(id, action, frozen.as_ref(), self.tx.clone()) {
                    tracing::error!("cannot start the app: {e}");
                    // Feed the failure back so the state machine can recover.
                    self.tx.send(Event::AppExited { id, success: false });
                }
            }
            Effect::SendToApp(DaemonToApp::RunAction { action, frozen }) => {
                vixeeny_platform::allow_foreground_handoff();
                let frozen = frozen.or_else(|| self.take_frozen(action, FrozenFor::Link));
                let carries_frozen = frozen.is_some();
                let msg = DaemonToApp::RunAction { action, frozen };
                let sent = match self.link.send(&msg) {
                    Ok(sent) => sent,
                    Err(e) => {
                        tracing::warn!("cannot send {msg:?}: {e}");
                        false
                    }
                };
                if !sent {
                    tracing::warn!("{action:?} not delivered");
                }
                if !sent && carries_frozen {
                    self.thaw();
                }
            }
            Effect::SendToApp(msg) => match self.link.send(&msg) {
                Ok(true) => {}
                Ok(false) => tracing::warn!("no app connected for {msg:?}"),
                Err(e) => tracing::warn!("cannot send {msg:?}: {e}"),
            },
            Effect::SetRecording(state) => self.tray.set_recording(state, self.lang),
            Effect::Notify(key) => {
                let text = tr(key, self.lang);
                tracing::warn!("notification: {text}");
                // Both are failures (a crash, shortcuts taken): their switch on the notifications page.
                let wanted = &self.config.notifications;
                if !(wanted.enabled && wanted.failures) {
                    return Flow::Continue;
                }
                self.tray.notify(text);
                // A crash is told on a card whose click prepares a report.
                if key == Key::AppCrashed {
                    self.spawner.show_card("failed-crash", "");
                }
            }
            Effect::PauseHotkeys(true) => {
                let _ = self.hotkeys.apply(&[]);
            }
            Effect::PauseHotkeys(false) => self.apply_hotkeys(),
            Effect::ReloadConfig => self.reload_config(),
            Effect::Quit => return Flow::Quit,
        }
        Flow::Continue
    }

    fn reload_config(&mut self) {
        let Some(path) = &self.config_path else {
            return;
        };
        match Config::load(path) {
            // Nothing to redo: the settings window asks after every change it saves.
            Ok(config) if config == self.config => {}
            Ok(config) => {
                let old = std::mem::replace(&mut self.config, config);
                // The replay turned on or off, or other games: the game in front counts again, and a running app
                // follows (its buffer goes when the replay is off).
                if old.replay != self.config.replay {
                    self.last_game = None;
                    if !self.look_for_game(None) && self.core.app_running() {
                        self.dispatch(Event::Action(ActionId::ReplayWatch));
                    }
                }
                if old.general.language != self.config.general.language {
                    self.lang = Lang::resolve(&self.config.general.language, self.os_locale.as_deref());
                    self.tray.set_language(self.lang);
                    self.tray.set_recording(self.core.recording_state(), self.lang);
                }
                if old.general.autostart != self.config.general.autostart {
                    self.apply_autostart();
                }
                // Registering them again would also repeat the warning about a taken shortcut.
                if old.hotkeys != self.config.hotkeys || old.overlay.enabled != self.config.overlay.enabled {
                    self.apply_hotkeys();
                }
                tracing::debug!("config reloaded");
            }
            // Keep running with the previous settings.
            Err(e) => tracing::warn!("config not reloaded: {e}"),
        }
    }
}

/// Tells the settings window which shortcuts did not register (the file is emptied when all did), when that changed.
/// Not in tests: they would write to the user's folder.
fn write_taken(failures: &[(Hotkey, String)]) {
    if cfg!(test) {
        return;
    }
    let Some(path) = vixeeny_common::paths::hotkeys_taken_file() else {
        return;
    };
    let text: String = failures.iter().map(|(h, _)| format!("{h}\n")).collect();
    // Unchanged, it is left alone: the settings window watches it.
    if std::fs::read_to_string(&path).is_ok_and(|was| was == text) {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, text) {
        tracing::warn!("cannot write {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};

    use crate::server::Waker;

    struct NoWake;
    impl Waker for NoWake {
        fn wake(&self) {}
    }

    #[derive(Default)]
    struct FakeTray {
        log: Arc<Mutex<Vec<String>>>,
    }
    impl Tray for FakeTray {
        fn set_recording(&mut self, state: RecState, _: Lang) {
            self.log.lock().unwrap().push(format!("rec {state:?}"));
        }
        fn set_language(&mut self, lang: Lang) {
            self.log.lock().unwrap().push(format!("lang {lang:?}"));
        }
        fn notify(&mut self, message: &str) {
            self.log.lock().unwrap().push(format!("notify {message}"));
        }
    }

    #[derive(Default)]
    struct FakeHotkeys {
        applied: Arc<Mutex<Vec<(ActionId, String)>>>,
        refuse: bool,
    }
    impl HotkeyBackend for FakeHotkeys {
        fn apply(&mut self, bindings: &[(ActionId, Hotkey)]) -> Vec<(Hotkey, String)> {
            *self.applied.lock().unwrap() = bindings.iter().map(|(a, h)| (*a, h.to_string())).collect();
            if self.refuse { bindings.iter().map(|(_, h)| (h.clone(), "taken".into())).collect() } else { Vec::new() }
        }
    }

    struct FakeSpawner {
        fail: bool,
        spawned: Arc<Mutex<Vec<(SpawnId, ActionId)>>>,
    }
    impl Spawner for FakeSpawner {
        fn spawn(&mut self, id: SpawnId, action: ActionId, _: Option<&Frozen>, _: EventTx) -> io::Result<()> {
            if self.fail {
                return Err(io::Error::other("boom"));
            }
            self.spawned.lock().unwrap().push((id, action));
            Ok(())
        }
    }

    type Log = Arc<Mutex<Vec<String>>>;
    type Spawned = Arc<Mutex<Vec<(SpawnId, ActionId)>>>;

    fn runtime(fail: bool, config: Config) -> (Runtime<FakeTray, FakeSpawner>, EventTx, Log, Spawned) {
        let (tx, rx) = channel();
        let events = EventTx::new(tx, Arc::new(NoWake));
        let tray = FakeTray::default();
        let log = tray.log.clone();
        let spawned: Spawned = Arc::default();
        let rt = Runtime::new(
            config,
            None,
            Some("fr-FR".into()),
            tray,
            Box::new(FakeHotkeys::default()),
            FakeSpawner { fail, spawned: spawned.clone() },
            AppLink::default(),
            events.clone(),
            rx,
        );
        (rt, events, log, spawned)
    }

    /// A game (process 7) is in front when the shared flag says so.
    struct FakeGames(Arc<Mutex<bool>>);

    impl Games for FakeGames {
        fn hook(&mut self, _: EventTx) -> bool {
            true
        }
        fn game(&mut self, _: Option<u64>, _: &Config) -> Option<u32> {
            self.0.lock().unwrap().then_some(7)
        }
    }

    #[test]
    fn the_app_starts_with_a_game_when_the_replay_is_on_or_says_it_is_off() {
        for (enabled, notify, watched) in
            [(false, "on", false), (true, "on", true), (false, "both", true), (true, "none", true)]
        {
            let mut config = Config::default();
            config.replay.enabled = enabled;
            config.notifications.game_start = notify.into();
            let (rt, events, _, spawned) = runtime(false, config);
            let game = Arc::new(Mutex::new(false));
            let mut rt = rt.with_games(Box::new(FakeGames(game.clone())));
            // No game: no app.
            rt.start_replay_if_configured();
            rt.pump();
            assert!(spawned.lock().unwrap().is_empty());
            // A game comes to the foreground: the app is told, once.
            *game.lock().unwrap() = true;
            for _ in 0..2 {
                events.send(Event::Foreground(1));
                rt.pump();
            }
            let expected = if watched { vec![(1, ActionId::ReplayWatch)] } else { Vec::new() };
            assert_eq!(*spawned.lock().unwrap(), expected, "{enabled} {notify}");
        }
    }

    /// The process in front (a game when set); process 8 is part of game 7 (its web pages).
    struct GameWithPages(Arc<Mutex<Option<u32>>>);

    impl Games for GameWithPages {
        fn hook(&mut self, _: EventTx) -> bool {
            true
        }
        fn game(&mut self, _: Option<u64>, _: &Config) -> Option<u32> {
            *self.0.lock().unwrap()
        }
        fn same_game(&self, pid: u32, game: u32) -> bool {
            pid == game || (pid, game) == (8, 7)
        }
    }

    #[test]
    fn the_web_pages_of_a_game_are_not_another_game() {
        let mut config = Config::default();
        config.replay.enabled = true;
        let (rt, events, _, spawned) = runtime(false, config);
        let front = Arc::new(Mutex::new(None));
        let mut rt = rt.with_games(Box::new(GameWithPages(front.clone())));
        rt.start_replay_if_configured();
        for (pid, game) in [(7, 7), (8, 7), (7, 7), (8, 7), (9, 9)] {
            *front.lock().unwrap() = Some(pid);
            events.send(Event::Foreground(1));
            rt.pump();
            assert_eq!(rt.last_game, Some(game), "{pid}");
            // A running app is still told when another process of the game comes in front (a launcher's game).
            assert_eq!(rt.front, Some(pid), "{pid}");
        }
        assert_eq!(*spawned.lock().unwrap(), vec![(1, ActionId::ReplayWatch)]);
    }

    #[test]
    fn tray_click_starts_the_app() {
        let (mut rt, tx, _, spawned) = runtime(false, Config::default());
        tx.send(Event::Action(ActionId::OpenSettings));
        assert_eq!(rt.pump(), Flow::Continue);
        assert_eq!(*spawned.lock().unwrap(), vec![(1, ActionId::OpenSettings)]);
    }

    #[test]
    fn spawn_failure_notifies_in_the_user_language_and_recovers() {
        let (mut rt, tx, log, _) = runtime(true, Config::default());
        tx.send(Event::Action(ActionId::OpenSettings));
        rt.pump();
        assert_eq!(*log.lock().unwrap(), vec!["notify Vixeeny s'est arrêté de façon inattendue".to_owned()]);
    }

    #[derive(Default, Clone)]
    struct FakeFreezer {
        log: Arc<Mutex<Vec<&'static str>>>,
    }
    impl Freezer for FakeFreezer {
        fn freeze(&mut self, _cursor: bool) -> Option<Frozen> {
            self.log.lock().unwrap().push("freeze");
            Some(Frozen { adapter: 1, screens: Vec::new() })
        }
        fn hold(&mut self, cursor: bool) -> Option<Frozen> {
            self.log.lock().unwrap().push(if cursor { "hold with cursor" } else { "hold" });
            Some(Frozen { adapter: 1, screens: Vec::new() })
        }
        fn thaw(&mut self) {
            self.log.lock().unwrap().push("thaw");
        }
    }

    #[test]
    fn a_full_screen_capture_gets_the_screens_of_its_press() {
        let mut config = Config::default();
        config.image.fullscreen.show_cursor = true;
        config.image.all_monitors.show_cursor = true;
        let (rt, tx, _, spawned) = runtime(false, config);
        let freezer = FakeFreezer::default();
        let mut rt = rt.with_freezer(Box::new(freezer.clone()));
        tx.send(Event::Action(ActionId::CaptureFullscreen));
        rt.pump();
        assert_eq!(*freezer.log.lock().unwrap(), vec!["hold with cursor"]);
        assert_eq!(spawned.lock().unwrap().last(), Some(&(1, ActionId::CaptureFullscreen)));
        assert!(rt.frozen.is_none());
        // Screens no action took go before new ones are held.
        rt.frozen = Some(Frozen { adapter: 1, screens: Vec::new() });
        rt.frozen_for = None;
        rt.freeze_or_thaw(&Event::Action(ActionId::CaptureAllMonitors));
        assert_eq!(*freezer.log.lock().unwrap(), vec!["hold with cursor", "thaw", "hold with cursor"]);
        assert!(rt.frozen.is_some());
    }

    #[test]
    fn a_zone_capture_freezes_first_and_thaws_if_its_app_dies() {
        let (rt, tx, _, spawned) = runtime(false, Config::default());
        let freezer = FakeFreezer::default();
        let mut rt = rt.with_freezer(Box::new(freezer.clone()));
        tx.send(Event::Action(ActionId::OpenSettings));
        rt.pump();
        assert!(freezer.log.lock().unwrap().is_empty());
        // The settings app exits; a zone capture starts a new app with the frozen screens.
        tx.send(Event::AppExited { id: 1, success: true });
        tx.send(Event::Action(ActionId::CaptureRegion));
        rt.pump();
        assert_eq!(*freezer.log.lock().unwrap(), vec!["freeze"]);
        assert_eq!(spawned.lock().unwrap().last(), Some(&(2, ActionId::CaptureRegion)));
        assert!(rt.frozen.is_none());
        // Another app's exit leaves them; the exit of the one that has them removes them.
        tx.send(Event::AppExited { id: 1, success: true });
        rt.pump();
        assert_eq!(*freezer.log.lock().unwrap(), vec!["freeze"]);
        tx.send(Event::AppExited { id: 2, success: false });
        rt.pump();
        assert_eq!(*freezer.log.lock().unwrap(), vec!["freeze", "thaw"]);
    }

    #[test]
    fn language_setting_beats_the_os_locale() {
        let mut config = Config::default();
        config.general.language = "en".into();
        let (rt, ..) = runtime(false, config);
        assert_eq!(rt.lang(), Lang::En);
        let (rt, ..) = runtime(false, Config::default());
        assert_eq!(rt.lang(), Lang::Fr);
    }

    #[test]
    fn hotkeys_are_registered_from_the_config() {
        let (tx, rx) = channel();
        let events = EventTx::new(tx, Arc::new(NoWake));
        let tray = FakeTray::default();
        let log = tray.log.clone();
        let hk = FakeHotkeys::default();
        let applied = hk.applied.clone();
        let mut config = Config::default();
        config.hotkeys.overlay_toggle = vec!["Ctrl+F9".into()]; // conflicts with record
        let spawner = FakeSpawner { fail: false, spawned: Arc::default() };
        let mut rt = Runtime::new(config, None, None, tray, Box::new(hk), spawner, AppLink::default(), events, rx);
        rt.apply_config();
        let applied = applied.lock().unwrap();
        assert!(applied.contains(&(ActionId::RecordToggle, "Ctrl+F9".into())));
        assert!(!applied.iter().any(|(a, _)| *a == ActionId::OverlayToggle));
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("notify")));
    }

    #[test]
    fn a_reload_redoes_only_what_changed() {
        let (tx, rx) = channel();
        let events = EventTx::new(tx, Arc::new(NoWake));
        let tray = FakeTray::default();
        let log = tray.log.clone();
        let hk = FakeHotkeys { refuse: true, ..Default::default() };
        let applied = hk.applied.clone();
        let dir = std::env::temp_dir().join(format!("vixeeny-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut config = Config::default();
        config.save(&path).unwrap();
        let mut rt = Runtime::new(
            config.clone(),
            Some(path.clone()),
            None,
            tray,
            Box::new(hk),
            FakeSpawner { fail: false, spawned: Arc::default() },
            AppLink::default(),
            events,
            rx,
        );
        let notified = || log.lock().unwrap().iter().filter(|l| l.starts_with("notify")).count();

        // A slider moved: the shortcuts stay registered, the taken one is not reported again.
        config.image.region.jpeg.quality = 42;
        config.save(&path).unwrap();
        rt.reload_config();
        assert!(applied.lock().unwrap().is_empty());
        assert_eq!(notified(), 0);
        assert_eq!(rt.config, config);

        // A shortcut changed: they are registered again.
        config.hotkeys.overlay_toggle = vec!["Ctrl+Alt+O".into()];
        config.save(&path).unwrap();
        rt.reload_config();
        assert!(!applied.lock().unwrap().is_empty());
        assert_eq!(notified(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn os_refusal_is_notified() {
        let (tx, rx) = channel();
        let events = EventTx::new(tx, Arc::new(NoWake));
        let tray = FakeTray::default();
        let log = tray.log.clone();
        let hk = FakeHotkeys { refuse: true, ..Default::default() };
        let spawner = FakeSpawner { fail: false, spawned: Arc::default() };
        let mut rt =
            Runtime::new(Config::default(), None, None, tray, Box::new(hk), spawner, AppLink::default(), events, rx);
        rt.apply_config();
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("notify")));
    }
}
