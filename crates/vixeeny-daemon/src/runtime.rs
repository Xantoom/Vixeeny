// SPDX-License-Identifier: GPL-3.0-or-later
//! Executes the effects decided by [`Core`]: talks to the tray, the app and the OS.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use vixeeny_common::config::Config;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Quit,
}

pub struct Runtime<T: Tray, S: Spawner> {
    core: Core,
    tray: T,
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
        spawner: S,
        link: AppLink,
        tx: EventTx,
        rx: Receiver<Event>,
    ) -> Self {
        let lang = Lang::resolve(&config.general.language, os_locale.as_deref());
        Self {
            core: Core::new(),
            tray,
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
    pub fn apply_config(&self) {
        if let Err(e) = autostart::apply(self.config.general.autostart) {
            tracing::warn!("cannot update autostart: {e}");
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
pub fn menu_label(key: Key, lang: Lang) -> &'static str {
    tr(key, lang)
}

#[cfg(test)]
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
}
