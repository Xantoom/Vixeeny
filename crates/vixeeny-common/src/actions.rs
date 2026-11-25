// SPDX-License-Identifier: GPL-3.0-or-later
//! Every action of Vixeeny in one table: its id, its name on the command line, its label and, for those a shortcut
//! starts, the field of the settings that holds its shortcuts and the shortcuts it has by default. A new action is one
//! line here, then one in [`ActionId::ALL`] (where the settings list it) and what it does in the app.

use serde::{Deserialize, Serialize};

use crate::i18n::Key;

macro_rules! actions {
    ($(
        $(#[$doc:meta])*
        $id:ident = $cli:literal, $label:ident $(, $field:ident = [$($key:literal),*])?;
    )*) => {
        /// An action the daemon asks the app to run. Declared in a fixed order: the messages between the two programs
        /// carry its index.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum ActionId {
            $($(#[$doc])* $id,)*
        }

        impl ActionId {
            /// Name used on the `vixeeny-app --action <name>` command line.
            pub const fn cli_name(self) -> &'static str {
                match self {
                    $(Self::$id => $cli,)*
                }
            }

            /// Its name in the settings and the menus.
            pub const fn label(self) -> Key {
                match self {
                    $(Self::$id => Key::$label,)*
                }
            }
        }

        /// The shortcuts of each action that has some (`[hotkeys]` of the settings).
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(default)]
        pub struct Hotkeys {
            $($(pub $field: Vec<String>,)?)*
        }

        impl Default for Hotkeys {
            fn default() -> Self {
                Self { $($($field: vec![$($key.to_owned()),*],)?)* }
            }
        }

        impl Hotkeys {
            /// The shortcuts of `action`; `None` for an action no shortcut starts.
            pub fn get(&self, action: ActionId) -> Option<&Vec<String>> {
                match action {
                    $($(ActionId::$id => Some(&self.$field),)?)*
                    #[allow(unreachable_patterns, reason = "every action may have a shortcut")]
                    _ => None,
                }
            }

            pub fn get_mut(&mut self, action: ActionId) -> Option<&mut Vec<String>> {
                match action {
                    $($(ActionId::$id => Some(&mut self.$field),)?)*
                    #[allow(unreachable_patterns, reason = "every action may have a shortcut")]
                    _ => None,
                }
            }
        }
    };
}

actions! {
    CaptureRegion = "capture-region", ActCaptureRegion, capture_region = ["PrintScreen"];
    CaptureWindow = "capture-window", ActCaptureWindow, capture_window = ["Alt+PrintScreen"];
    CaptureFullscreen = "capture-fullscreen", ActCaptureFullscreen, capture_fullscreen = ["Ctrl+PrintScreen"];
    CaptureAllMonitors = "capture-all-monitors", ActCaptureAll, capture_all_monitors = ["Ctrl+F7"];
    CaptureScrolling = "capture-scrolling", ActCaptureScrolling, capture_scrolling = ["Shift+PrintScreen"];
    RecordToggle = "record-toggle", ActRecordToggle, record_toggle = ["Ctrl+F9"];
    RecordPause = "record-pause", ActRecordPause, record_pause = ["Ctrl+F10"];
    /// Three keys: turning the replay off by mistake would lose the moment to keep.
    ReplayToggle = "replay-toggle", ActReplayToggle, replay_toggle = ["Ctrl+Shift+F11"];
    ReplaySave = "replay-save", ActReplaySave, replay_save = ["Ctrl+F11"];
    /// Saves the replay, then opens it in the video editor to cut it.
    ReplayEdit = "replay-edit", ActReplayEdit, replay_edit = ["Ctrl+F12"];
    OverlayToggle = "overlay-toggle", ActOverlay, overlay_toggle = ["Ctrl+F8"];
    /// The tray icon opens the settings: no shortcut.
    OpenSettings = "open-settings", ActSettings;
    /// Not a shortcut: keeps the app running while the replay is on, to start it during full-screen games (the daemon
    /// sends it at startup and when the settings change).
    ReplayWatch = "replay-watch", ActReplayToggle;
    /// Captures the zone of the last zone capture again, straight to its file (no editor).
    CaptureLastRegion = "capture-last-region", ActCaptureLastRegion, capture_last_region = ["Ctrl+Shift+PrintScreen"];
    /// Starts a recording of a zone picked on the screen, or stops the recording.
    RecordRegion = "record-region", ActRecordRegion, record_region = ["Ctrl+Shift+F9"];
    /// Starts a recording of a window picked on the screen, or stops the recording.
    RecordWindow = "record-window", ActRecordWindow, record_window = ["Ctrl+Alt+F9"];
    /// Copies the text of a zone picked on the frozen screens (no editor).
    CaptureText = "capture-text", ActCaptureText, capture_text = ["Ctrl+Alt+PrintScreen"];
    /// Pins a zone picked on the frozen screens over the windows (no editor).
    CapturePin = "capture-pin", ActCapturePin, capture_pin = ["Ctrl+Shift+F8"];
    /// Stops the recording and deletes its files (three keys: it throws the recording away).
    RecordDiscard = "record-discard", ActRecordDiscard, record_discard = ["Ctrl+Shift+F10"];
}

impl ActionId {
    /// All actions, in the order the settings list them (the first claim on a shortcut wins in this order).
    pub const ALL: [Self; 19] = [
        Self::CaptureRegion,
        Self::CaptureLastRegion,
        Self::CaptureText,
        Self::CapturePin,
        Self::CaptureWindow,
        Self::CaptureFullscreen,
        Self::CaptureAllMonitors,
        Self::CaptureScrolling,
        Self::RecordToggle,
        Self::RecordRegion,
        Self::RecordWindow,
        Self::RecordPause,
        Self::RecordDiscard,
        Self::ReplayToggle,
        Self::ReplaySave,
        Self::ReplayEdit,
        Self::OverlayToggle,
        Self::OpenSettings,
        Self::ReplayWatch,
    ];

    /// The screenshots: their settings may delay them.
    pub const fn is_capture(self) -> bool {
        matches!(
            self,
            Self::CaptureRegion
                | Self::CaptureWindow
                | Self::CaptureFullscreen
                | Self::CaptureAllMonitors
                | Self::CaptureScrolling
                | Self::CaptureLastRegion
                | Self::CaptureText
                | Self::CapturePin
        )
    }

    /// Start or stop a recording.
    pub const fn toggles_recording(self) -> bool {
        matches!(self, Self::RecordToggle | Self::RecordRegion | Self::RecordWindow)
    }

    /// Actions that start on the frozen screen (a zone is picked on it).
    pub const fn freezes_screen(self) -> bool {
        matches!(self, Self::CaptureRegion | Self::CaptureScrolling | Self::CaptureText | Self::CapturePin)
    }

    /// The daemon captures the screens the moment the shortcut is pressed, for the app to save (they are not shown).
    pub const fn holds_screen(self) -> bool {
        matches!(self, Self::CaptureFullscreen | Self::CaptureAllMonitors | Self::CaptureLastRegion)
    }

    pub fn from_cli_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.cli_name() == name)
    }
}

impl Hotkeys {
    /// Every action a shortcut starts, with its shortcuts, in [`ActionId::ALL`] order.
    pub fn bindings(&self) -> impl Iterator<Item = (ActionId, &[String])> {
        ActionId::ALL.into_iter().filter_map(|action| Some((action, self.get(action)?.as_slice())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_is_listed_once_and_found_by_its_name() {
        for action in ActionId::ALL {
            assert_eq!(ActionId::ALL.iter().filter(|a| **a == action).count(), 1, "{action:?}");
            assert_eq!(ActionId::from_cli_name(action.cli_name()), Some(action));
        }
        assert_eq!(ActionId::from_cli_name("nope"), None);
    }
}
