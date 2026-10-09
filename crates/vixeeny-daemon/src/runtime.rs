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

/// What the daemon shows in the OS notification area.
pub trait Tray {
    fn set_recording(&mut self, state: RecState, lang: Lang);
    fn set_language(&mut self, lang: Lang);
    /// Shows a short message to the user. Real toast notifications arrive with M17.
    fn notify(&mut self, message: &str);
}

/// Registers the global shortcuts with the OS. A shortcut press becomes an
/// [`Event::Action`] sent by the backend itself.
pub trait HotkeyBackend {
    /// Replaces the registered shortcuts by `bindings`. Returns the shortcuts the OS refused
    /// (typically because another application owns them), each with the reason.
    fn apply(&mut self, bindings: &[(ActionId, Hotkey)]) -> Vec<(Hotkey, String)>;
}

/// Freezes the screens the moment a zone capture is asked for (see
/// `vixeeny_capture::freeze`); the app takes over from the frozen screens.
pub trait Freezer {
    /// `None` when the screens cannot be frozen, or already are.
    fn freeze(&mut self) -> Option<Frozen>;
    fn thaw(&mut self);
}

/// No freezing: the app captures the screens itself.
pub struct NoFreeze;

impl Freezer for NoFreeze {
    fn freeze(&mut self) -> Option<Frozen> {
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
}

impl<T: Tray, S: Spawner> Runtime<T, S> {
    #[allow(clippy::too_many_arguments)]
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
        }
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
        if let Err(e) = autostart::apply(self.config.general.autostart) {
            tracing::warn!("cannot update autostart: {e}");
        }
        self.apply_hotkeys();
    }

    /// Starts watching for full-screen games if the replay is on (once, when the daemon
    /// starts): the app starts the replay with a game.
    pub fn start_replay_if_configured(&mut self) {
        if self.config.replay.enabled {
            self.tx.send(Event::Action(ActionId::ReplayWatch));
        }
    }

    /// The very first start opens the welcome assistant (the settings process shows it).
    pub fn open_wizard_if_first_run(&mut self) {
        if !self.config.general.first_run_done {
            self.tx.send(Event::Action(ActionId::OpenSettings));
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
                    tracing::warn!(
                        "shortcut `{text}` for {action:?} ignored: at most 3 per action"
                    );
                }
                ProblemKind::Duplicate => {
                    tracing::warn!("shortcut `{text}` listed twice for {action:?}");
                }
                ProblemKind::Conflict(other) => {
                    tracing::warn!("shortcut `{text}` for {action:?} conflicts with {other:?}");
                }
            }
        }
        let failures = self.hotkeys.apply(&resolution.bindings);
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
        self.freeze_or_thaw(&event);
        let mut flow = Flow::Continue;
        for effect in self.core.handle(event) {
            if self.execute(effect) == Flow::Quit {
                flow = Flow::Quit;
            }
        }
        flow
    }

    /// The screens freeze before anything else happens for a zone capture; they go when the
    /// app that should take over from them is gone (the app itself removes them otherwise).
    fn freeze_or_thaw(&mut self, event: &Event) {
        match *event {
            Event::Action(action) | Event::Control(ControlRequest::Action(action))
                if action.freezes_screen() =>
            {
                // `None` while earlier frozen screens are still up: they serve this press.
                if let Some(frozen) = self.freezer.freeze() {
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
        let frozen = self.frozen.take().filter(|_| action.freezes_screen());
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
                if let Err(e) = self
                    .spawner
                    .spawn(id, action, frozen.as_ref(), self.tx.clone())
                {
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
                self.tray.notify(text);
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
            Ok(config) => {
                // The app follows the replay being turned on or off (and reads the settings).
                let replay = self.config.replay.enabled || config.replay.enabled;
                self.config = config;
                if replay {
                    self.tx.send(Event::Action(ActionId::ReplayWatch));
                }
                self.lang = Lang::resolve(&self.config.general.language, self.os_locale.as_deref());
                self.tray.set_language(self.lang);
                self.tray
                    .set_recording(self.core.recording_state(), self.lang);
                self.apply_config();
                tracing::info!("config reloaded");
            }
            // Keep running with the previous settings.
            Err(e) => tracing::warn!("config not reloaded: {e}"),
        }
    }
}

/// Tells the settings window which shortcuts did not register (the file is emptied when all
/// did). Not in tests: they would write to the user's folder.
fn write_taken(failures: &[(Hotkey, String)]) {
    if cfg!(test) {
        return;
    }
    let Some(path) = vixeeny_common::paths::hotkeys_taken_file() else {
        return;
    };
    let text: String = failures.iter().map(|(h, _)| format!("{h}\n")).collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, text) {
        tracing::warn!("cannot write {}: {e}", path.display());
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
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
            *self.applied.lock().unwrap() =
                bindings.iter().map(|(a, h)| (*a, h.to_string())).collect();
            if self.refuse {
                bindings
                    .iter()
                    .map(|(_, h)| (h.clone(), "taken".into()))
                    .collect()
            } else {
                Vec::new()
            }
        }
    }

    struct FakeSpawner {
        fail: bool,
        spawned: Arc<Mutex<Vec<(SpawnId, ActionId)>>>,
    }
    impl Spawner for FakeSpawner {
        fn spawn(
            &mut self,
            id: SpawnId,
            action: ActionId,
            _: Option<&Frozen>,
            _: EventTx,
        ) -> io::Result<()> {
            if self.fail {
                return Err(io::Error::other("boom"));
            }
            self.spawned.lock().unwrap().push((id, action));
            Ok(())
        }
    }

    type Log = Arc<Mutex<Vec<String>>>;
    type Spawned = Arc<Mutex<Vec<(SpawnId, ActionId)>>>;

    fn runtime(
        fail: bool,
        config: Config,
    ) -> (Runtime<FakeTray, FakeSpawner>, EventTx, Log, Spawned) {
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
            FakeSpawner {
                fail,
                spawned: spawned.clone(),
            },
            AppLink::default(),
            events.clone(),
            rx,
        );
        (rt, events, log, spawned)
    }

    #[test]
    fn the_app_watches_for_games_only_when_the_replay_is_on() {
        let (mut rt, _, _, spawned) = runtime(false, Config::default());
        rt.start_replay_if_configured();
        rt.pump();
        assert!(spawned.lock().unwrap().is_empty());

        let mut config = Config::default();
        config.replay.enabled = true;
        let (mut rt, _, _, spawned) = runtime(false, config);
        rt.start_replay_if_configured();
        rt.pump();
        assert_eq!(*spawned.lock().unwrap(), vec![(1, ActionId::ReplayWatch)]);
    }

    #[test]
    fn the_assistant_opens_once_on_the_first_start() {
        let (mut rt, _, _, spawned) = runtime(false, Config::default());
        rt.open_wizard_if_first_run();
        rt.pump();
        assert_eq!(*spawned.lock().unwrap(), vec![(1, ActionId::OpenSettings)]);

        let mut config = Config::default();
        config.general.first_run_done = true;
        let (mut rt, _, _, spawned) = runtime(false, config);
        rt.open_wizard_if_first_run();
        rt.pump();
        assert!(spawned.lock().unwrap().is_empty());
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
        assert_eq!(
            *log.lock().unwrap(),
            vec!["notify Vixeeny s'est arrêté de façon inattendue".to_owned()]
        );
    }

    #[derive(Default, Clone)]
    struct FakeFreezer {
        log: Arc<Mutex<Vec<&'static str>>>,
    }
    impl Freezer for FakeFreezer {
        fn freeze(&mut self) -> Option<Frozen> {
            self.log.lock().unwrap().push("freeze");
            Some(Frozen {
                adapter: 1,
                screens: Vec::new(),
            })
        }
        fn thaw(&mut self) {
            self.log.lock().unwrap().push("thaw");
        }
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
        tx.send(Event::AppExited {
            id: 1,
            success: true,
        });
        tx.send(Event::Action(ActionId::CaptureRegion));
        rt.pump();
        assert_eq!(*freezer.log.lock().unwrap(), vec!["freeze"]);
        assert_eq!(
            spawned.lock().unwrap().last(),
            Some(&(2, ActionId::CaptureRegion))
        );
        assert!(rt.frozen.is_none());
        // Another app's exit leaves them; the exit of the one that has them removes them.
        tx.send(Event::AppExited {
            id: 1,
            success: true,
        });
        rt.pump();
        assert_eq!(*freezer.log.lock().unwrap(), vec!["freeze"]);
        tx.send(Event::AppExited {
            id: 2,
            success: false,
        });
        rt.pump();
        assert_eq!(*freezer.log.lock().unwrap(), vec!["freeze", "thaw"]);
    }

    #[test]
    fn quit_flows_out_of_pump() {
        let (mut rt, tx, _, _) = runtime(false, Config::default());
        tx.send(Event::TrayQuit);
        assert_eq!(rt.pump(), Flow::Quit);
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
    fn recording_state_reaches_the_tray() {
        use vixeeny_common::ipc::AppToDaemon;
        let (mut rt, tx, log, _) = runtime(false, Config::default());
        tx.send(Event::App(AppToDaemon::RecordingStateChanged(
            RecState::Recording,
        )));
        rt.pump();
        assert_eq!(*log.lock().unwrap(), vec!["rec Recording".to_owned()]);
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
        config.hotkeys.open_settings = vec!["Ctrl+Shift+R".into()]; // conflicts with record
        let spawner = FakeSpawner {
            fail: false,
            spawned: Arc::default(),
        };
        let mut rt = Runtime::new(
            config,
            None,
            None,
            tray,
            Box::new(hk),
            spawner,
            AppLink::default(),
            events,
            rx,
        );
        rt.apply_config();
        let applied = applied.lock().unwrap();
        assert!(applied.contains(&(ActionId::RecordToggle, "Ctrl+Shift+R".into())));
        assert!(!applied.iter().any(|(a, _)| *a == ActionId::OpenSettings));
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("notify")));
    }

    #[test]
    fn os_refusal_is_notified() {
        let (tx, rx) = channel();
        let events = EventTx::new(tx, Arc::new(NoWake));
        let tray = FakeTray::default();
        let log = tray.log.clone();
        let hk = FakeHotkeys {
            refuse: true,
            ..Default::default()
        };
        let spawner = FakeSpawner {
            fail: false,
            spawned: Arc::default(),
        };
        let mut rt = Runtime::new(
            Config::default(),
            None,
            None,
            tray,
            Box::new(hk),
            spawner,
            AppLink::default(),
            events,
            rx,
        );
        rt.apply_config();
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("notify")));
    }
}
