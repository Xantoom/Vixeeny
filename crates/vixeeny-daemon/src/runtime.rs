// SPDX-License-Identifier: GPL-3.0-or-later
//! Executes the effects decided by [`Core`]: talks to the tray, the app and the OS.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use vixeeny_common::config::Config;
use vixeeny_common::hotkey::{self, Hotkey, ProblemKind};
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::{ActionId, RecState};

use crate::autostart;
use crate::core::{Core, Effect, Event};
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
    /// Replaces the registered shortcuts by `bindings`. Returns one message per shortcut the
    /// OS refused (typically because another application owns it).
    fn apply(&mut self, bindings: &[(ActionId, Hotkey)]) -> Vec<String>;
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
        }
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

    /// Starts the replay buffer if the settings say so (once, when the daemon starts).
    pub fn start_replay_if_configured(&mut self) {
        if self.config.replay.enabled_on_start {
            self.tx.send(Event::Action(ActionId::ReplayToggle));
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
        for failure in self.hotkeys.apply(&resolution.bindings) {
            tracing::warn!("shortcut not registered: {failure}");
            trouble = true;
        }
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
        let mut flow = Flow::Continue;
        for effect in self.core.handle(event) {
            if self.execute(effect) == Flow::Quit {
                flow = Flow::Quit;
            }
        }
        flow
    }

    fn execute(&mut self, effect: Effect) -> Flow {
        match effect {
            Effect::SpawnApp { id, action } => {
                if let Err(e) = self.spawner.spawn(id, action, self.tx.clone()) {
                    tracing::error!("cannot start the app: {e}");
                    // Feed the failure back so the state machine can recover.
                    self.tx.send(Event::AppExited { id, success: false });
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
            Effect::NotifyUpdate(version) => {
                let text = tr(Key::UpdateAvailable, self.lang).replace("{version}", &version);
                tracing::info!("{text}");
                if !(self.config.general.notification_style == "native"
                    && native_update_toast(&text))
                {
                    self.tray.notify(&text);
                }
            }
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
                self.config = config;
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

    /// Entry point for tray menu clicks.
    pub fn open_settings_event() -> Event {
        Event::Action(ActionId::OpenSettings)
    }
}

/// Convenience used by the tray implementations.
/// Asks the app (next to the daemon) to show the clickable Windows notification. `false` when it
/// could not be started: the caller falls back to the tray balloon.
fn native_update_toast(text: &str) -> bool {
    #[cfg(windows)]
    {
        let started = std::env::current_exe().and_then(|mut path| {
            path.set_file_name(format!("vixeeny-app{}", std::env::consts::EXE_SUFFIX));
            std::process::Command::new(path)
                .args(["--update-toast", text])
                .spawn()
        });
        match started {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("cannot start the update notification: {e}");
                false
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        false
    }
}

pub fn menu_label(key: Key, lang: Lang) -> &'static str {
    tr(key, lang)
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};

    use crate::core::SpawnId;
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
        fn apply(&mut self, bindings: &[(ActionId, Hotkey)]) -> Vec<String> {
            *self.applied.lock().unwrap() =
                bindings.iter().map(|(a, h)| (*a, h.to_string())).collect();
            if self.refuse {
                vec!["taken".into()]
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
        fn spawn(&mut self, id: SpawnId, action: ActionId, _: EventTx) -> io::Result<()> {
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
    fn the_replay_buffer_starts_with_the_daemon_only_when_asked() {
        let (mut rt, _, _, spawned) = runtime(false, Config::default());
        rt.start_replay_if_configured();
        rt.pump();
        assert!(spawned.lock().unwrap().is_empty());

        let mut config = Config::default();
        config.replay.enabled_on_start = true;
        let (mut rt, _, _, spawned) = runtime(false, config);
        rt.start_replay_if_configured();
        rt.pump();
        assert_eq!(*spawned.lock().unwrap(), vec![(1, ActionId::ReplayToggle)]);
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
        tx.send(Runtime::<FakeTray, FakeSpawner>::open_settings_event());
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
