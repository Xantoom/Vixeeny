// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure daemon logic: which effects follow from which events. No I/O, no threads.

use vixeeny_common::i18n::Key;
use vixeeny_common::ipc::{ActionId, AppToDaemon, ControlRequest, DaemonToApp, RecState};

/// Identifies one launch of `vixeeny-app`, so the exit of an old process cannot be mistaken for
/// the exit of the one that replaced it.
pub type SpawnId = u64;

/// Something that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A second launch of Vixeeny (or a CLI) asked for something.
    Control(ControlRequest),
    /// A user-triggered action (tray menu, and from M3 global hotkeys).
    Action(ActionId),
    /// The app connected over IPC.
    AppConnected,
    /// A message from the connected app.
    App(AppToDaemon),
    /// The IPC connection to the app closed.
    AppDisconnected,
    /// An app process exited.
    AppExited {
        id: SpawnId,
        success: bool,
    },
    TrayQuit,
}

/// Something to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Start `vixeeny-app`, passing the action to run once it is up.
    SpawnApp {
        id: SpawnId,
        action: ActionId,
    },
    SendToApp(DaemonToApp),
    SetRecording(RecState),
    Notify(Key),
    /// Re-read `config.toml` and apply it (language, autostart…).
    ReloadConfig,
    /// Leave the message loop.
    Quit,
}

#[derive(Debug, Default)]
pub struct Core {
    connected: bool,
    ready: bool,
    /// Launch currently expected to become (or be) the app.
    current: Option<SpawnId>,
    last_id: SpawnId,
    /// The app announced `Idle` or was told to shut down: its exit is not a crash.
    exit_expected: bool,
    recording: Option<RecState>,
    /// Actions waiting for the app to become ready.
    queued: Vec<ActionId>,
}

impl Core {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn recording_state(&self) -> RecState {
        self.recording.unwrap_or(RecState::Idle)
    }

    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        let mut fx = Vec::new();
        match event {
            Event::Control(ControlRequest::OpenSettings) => {
                self.run(ActionId::OpenSettings, &mut fx);
            }
            Event::Control(ControlRequest::Quit) | Event::TrayQuit => self.quit(&mut fx),
            Event::Control(ControlRequest::ReloadConfig) => fx.push(Effect::ReloadConfig),
            Event::Control(ControlRequest::Ping) => {}
            Event::Action(action) => self.run(action, &mut fx),
            Event::AppConnected => self.connected = true,
            Event::App(msg) => self.on_app_message(msg, &mut fx),
            Event::AppDisconnected => {
                self.connected = false;
                self.ready = false;
                self.reset_recording(&mut fx);
            }
            Event::AppExited { id, success } => self.on_exit(id, success, &mut fx),
        }
        fx
    }

    fn run(&mut self, action: ActionId, fx: &mut Vec<Effect>) {
        if self.connected && self.ready && !self.exit_expected {
            fx.push(Effect::SendToApp(DaemonToApp::RunAction {
                action,
                frozen_frame: None,
            }));
        } else if self.current.is_some() && !self.exit_expected {
            // Starting up: deliver once `Ready` arrives.
            self.queued.push(action);
        } else {
            self.last_id += 1;
            self.current = Some(self.last_id);
            self.exit_expected = false;
            self.ready = false;
            // The spawned app receives its first action on its command line.
            fx.push(Effect::SpawnApp {
                id: self.last_id,
                action,
            });
        }
    }

    fn on_app_message(&mut self, msg: AppToDaemon, fx: &mut Vec<Effect>) {
        match msg {
            AppToDaemon::Ready => {
                self.ready = true;
                for action in self.queued.drain(..) {
                    fx.push(Effect::SendToApp(DaemonToApp::RunAction {
                        action,
                        frozen_frame: None,
                    }));
                }
            }
            AppToDaemon::RecordingStateChanged(state) => {
                self.recording = Some(state);
                fx.push(Effect::SetRecording(state));
            }
            AppToDaemon::Idle => {
                self.exit_expected = true;
                self.ready = false;
            }
            AppToDaemon::ConfigChanged => fx.push(Effect::ReloadConfig),
            // Self-update arrives with M18.
            AppToDaemon::RequestRestartForUpdate => {}
        }
    }

    fn on_exit(&mut self, id: SpawnId, success: bool, fx: &mut Vec<Effect>) {
        if self.current != Some(id) {
            // An earlier app that was already replaced.
            return;
        }
        self.current = None;
        self.connected = false;
        self.ready = false;
        self.queued.clear();
        self.reset_recording(fx);
        if !success && !self.exit_expected {
            fx.push(Effect::Notify(Key::AppCrashed));
        }
        self.exit_expected = false;
    }

    /// A vanished app cannot still be recording: clear the tray dot.
    fn reset_recording(&mut self, fx: &mut Vec<Effect>) {
        if self.recording_state() != RecState::Idle {
            self.recording = Some(RecState::Idle);
            fx.push(Effect::SetRecording(RecState::Idle));
        }
    }

    fn quit(&mut self, fx: &mut Vec<Effect>) {
        if self.connected {
            self.exit_expected = true;
            fx.push(Effect::SendToApp(DaemonToApp::Shutdown));
        }
        fx.push(Effect::Quit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn(id: SpawnId, action: ActionId) -> Effect {
        Effect::SpawnApp { id, action }
    }

    fn run(action: ActionId) -> Effect {
        Effect::SendToApp(DaemonToApp::RunAction {
            action,
            frozen_frame: None,
        })
    }

    /// Spawns launch 1 and brings it to the ready state.
    fn ready_app() -> Core {
        let mut core = Core::new();
        assert_eq!(
            core.handle(Event::Action(ActionId::OpenSettings)),
            vec![spawn(1, ActionId::OpenSettings)]
        );
        core.handle(Event::AppConnected);
        assert!(core.handle(Event::App(AppToDaemon::Ready)).is_empty());
        core
    }

    #[test]
    fn first_action_spawns_the_app() {
        ready_app();
    }

    #[test]
    fn actions_while_starting_are_queued_then_delivered() {
        let mut core = Core::new();
        core.handle(Event::Action(ActionId::OpenSettings));
        assert!(
            core.handle(Event::Action(ActionId::CaptureRegion))
                .is_empty()
        );
        core.handle(Event::AppConnected);
        assert_eq!(
            core.handle(Event::App(AppToDaemon::Ready)),
            vec![run(ActionId::CaptureRegion)]
        );
    }

    #[test]
    fn running_app_gets_actions_directly() {
        let mut core = ready_app();
        assert_eq!(
            core.handle(Event::Action(ActionId::CaptureWindow)),
            vec![run(ActionId::CaptureWindow)]
        );
    }

    #[test]
    fn second_launch_opens_settings() {
        let mut core = Core::new();
        assert_eq!(
            core.handle(Event::Control(ControlRequest::OpenSettings)),
            vec![spawn(1, ActionId::OpenSettings)]
        );
    }

    #[test]
    fn idle_app_is_replaced_on_next_action_and_its_exit_is_ignored() {
        let mut core = ready_app();
        core.handle(Event::App(AppToDaemon::Idle));
        assert_eq!(
            core.handle(Event::Action(ActionId::CaptureRegion)),
            vec![spawn(2, ActionId::CaptureRegion)]
        );
        // The old process quits after the new one started: nothing happens, in particular no
        // crash notification and the new launch stays tracked.
        assert!(
            core.handle(Event::AppExited {
                id: 1,
                success: false
            })
            .is_empty()
        );
        assert!(
            core.handle(Event::Action(ActionId::CaptureWindow))
                .is_empty()
        );
    }

    #[test]
    fn clean_exit_after_idle_is_silent_and_allows_a_new_launch() {
        let mut core = ready_app();
        core.handle(Event::App(AppToDaemon::Idle));
        assert!(core.handle(Event::AppDisconnected).is_empty());
        assert!(
            core.handle(Event::AppExited {
                id: 1,
                success: true
            })
            .is_empty()
        );
        assert_eq!(
            core.handle(Event::Action(ActionId::OpenSettings)),
            vec![spawn(2, ActionId::OpenSettings)]
        );
    }

    #[test]
    fn crash_without_idle_notifies_exactly_once() {
        let mut core = ready_app();
        assert!(core.handle(Event::AppDisconnected).is_empty());
        assert_eq!(
            core.handle(Event::AppExited {
                id: 1,
                success: false
            }),
            vec![Effect::Notify(Key::AppCrashed)]
        );
    }

    #[test]
    fn crash_during_startup_notifies_and_a_retry_works() {
        let mut core = Core::new();
        core.handle(Event::Action(ActionId::OpenSettings));
        assert_eq!(
            core.handle(Event::AppExited {
                id: 1,
                success: false
            }),
            vec![Effect::Notify(Key::AppCrashed)]
        );
        assert_eq!(
            core.handle(Event::Action(ActionId::OpenSettings)),
            vec![spawn(2, ActionId::OpenSettings)]
        );
    }

    #[test]
    fn recording_state_drives_the_tray_and_resets_when_the_app_vanishes() {
        let mut core = ready_app();
        assert_eq!(
            core.handle(Event::App(AppToDaemon::RecordingStateChanged(
                RecState::Recording
            ))),
            vec![Effect::SetRecording(RecState::Recording)]
        );
        assert_eq!(
            core.handle(Event::AppDisconnected),
            vec![Effect::SetRecording(RecState::Idle)]
        );
        assert_eq!(core.recording_state(), RecState::Idle);
        assert_eq!(
            core.handle(Event::AppExited {
                id: 1,
                success: false
            }),
            vec![Effect::Notify(Key::AppCrashed)]
        );
    }

    #[test]
    fn a_reload_request_from_the_settings_window_reloads_the_config() {
        let mut core = Core::new();
        assert_eq!(
            core.handle(Event::Control(ControlRequest::ReloadConfig)),
            vec![Effect::ReloadConfig]
        );
    }

    #[test]
    fn config_changed_triggers_reload() {
        let mut core = Core::new();
        assert_eq!(
            core.handle(Event::App(AppToDaemon::ConfigChanged)),
            vec![Effect::ReloadConfig]
        );
    }

    #[test]
    fn quit_tells_a_connected_app_to_shut_down() {
        let mut core = ready_app();
        assert_eq!(
            core.handle(Event::TrayQuit),
            vec![Effect::SendToApp(DaemonToApp::Shutdown), Effect::Quit]
        );
    }

    #[test]
    fn quit_without_app_just_quits() {
        let mut core = Core::new();
        assert_eq!(
            core.handle(Event::Control(ControlRequest::Quit)),
            vec![Effect::Quit]
        );
    }

    #[test]
    fn ping_does_nothing() {
        let mut core = Core::new();
        assert!(core.handle(Event::Control(ControlRequest::Ping)).is_empty());
    }
}
