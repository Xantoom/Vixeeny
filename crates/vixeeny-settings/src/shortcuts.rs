// SPDX-License-Identifier: GPL-3.0-or-later
//! The shortcuts table (plan 5.13, section 3): two slots per action, checked as they are typed.

use vixeeny_common::config::{Config, Hotkeys};
use vixeeny_common::hotkey::{Hotkey, MAX_PER_ACTION};
use vixeeny_common::ipc::ActionId;

/// What is wrong with a shortcut that was typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not a key combination (the parser's message).
    Invalid(String),
    /// Already used by this action.
    Duplicate,
    /// Already used by another action.
    Conflict(ActionId),
}

fn list_mut(hotkeys: &mut Hotkeys, action: ActionId) -> &mut Vec<String> {
    match action {
        ActionId::CaptureRegion => &mut hotkeys.capture_region,
        ActionId::CaptureWindow => &mut hotkeys.capture_window,
        ActionId::CaptureFullscreen => &mut hotkeys.capture_fullscreen,
        ActionId::CaptureAllMonitors => &mut hotkeys.capture_all_monitors,
        ActionId::CaptureScrolling => &mut hotkeys.capture_scrolling,
        ActionId::RecordToggle => &mut hotkeys.record_toggle,
        ActionId::RecordPause => &mut hotkeys.record_pause,
        // Watching for games has no shortcut of its own (never listed).
        ActionId::ReplayToggle | ActionId::ReplayWatch => &mut hotkeys.replay_toggle,
        ActionId::ReplaySave => &mut hotkeys.replay_save,
        ActionId::OverlayToggle => &mut hotkeys.overlay_toggle,
        ActionId::OpenSettings => &mut hotkeys.open_settings,
    }
}

/// The table: every action with its [`MAX_PER_ACTION`] slots (empty text = free slot).
pub fn table(config: &Config) -> Vec<(ActionId, [String; MAX_PER_ACTION])> {
    config
        .hotkeys
        .bindings()
        .into_iter()
        .map(|(action, texts)| {
            let mut slots: [String; MAX_PER_ACTION] = Default::default();
            for (slot, text) in slots.iter_mut().zip(texts) {
                slot.clone_from(text);
            }
            (action, slots)
        })
        .collect()
}

/// What the settings page lists: the scrolling capture is chosen in the capture overlay, not
/// from a shortcut page (its shortcut still works if the settings file has one).
pub fn listed(config: &Config) -> Vec<(ActionId, [String; MAX_PER_ACTION])> {
    table(config)
        .into_iter()
        .filter(|(action, _)| *action != ActionId::CaptureScrolling)
        .collect()
}

/// Sets slot `slot` of `action` to `text` (empty clears it). The shortcut is written with its
/// canonical names, its modifiers in the order given. Nothing changes when it is refused.
pub fn set(config: &mut Config, action: ActionId, slot: usize, text: &str) -> Result<(), Refusal> {
    let mut slots = table(config)
        .into_iter()
        .find(|(a, _)| *a == action)
        .map(|(_, s)| s)
        .unwrap_or_default();
    let Some(target) = slots.get_mut(slot) else {
        return Ok(());
    };
    let text = text.trim();
    if text.is_empty() {
        target.clear();
    } else {
        let hotkey = Hotkey::parse(text).map_err(|e| Refusal::Invalid(e.to_string()))?;
        for (other, texts) in config.hotkeys.bindings() {
            for (index, used) in texts.iter().enumerate() {
                if other == action && index == slot {
                    continue; // the slot being replaced
                }
                if Hotkey::parse(used).is_ok_and(|h| h == hotkey) {
                    return Err(if other == action {
                        Refusal::Duplicate
                    } else {
                        Refusal::Conflict(other)
                    });
                }
            }
        }
        *target = hotkey.in_order(text);
    }
    *list_mut(&mut config.hotkeys, action) = slots.into_iter().filter(|s| !s.is_empty()).collect();
    Ok(())
}

/// Shortcuts Windows keeps for itself: a program never receives them.
const KEPT_BY_WINDOWS: &[&str] = &[
    "Alt+Tab",
    "Alt+F4",
    "Ctrl+Alt+Delete",
    "Win+A",
    "Win+D",
    "Win+E",
    "Win+G",
    "Win+H",
    "Win+I",
    "Win+K",
    "Win+L",
    "Win+P",
    "Win+R",
    "Win+S",
    "Win+U",
    "Win+V",
    "Win+X",
    "Win+Tab",
    "Win+Space",
    "Win+PrintScreen",
    "Win+Shift+S",
    "Win+Alt+B",
    "Win+Alt+G",
    "Win+Alt+M",
    "Win+Alt+R",
    "Win+Alt+T",
    "Win+Alt+PrintScreen",
];

/// The same key combination, however it is written.
pub fn same(a: &str, b: &str) -> bool {
    matches!((Hotkey::parse(a), Hotkey::parse(b)), (Ok(a), Ok(b)) if a == b)
}

/// Whether Windows keeps `text` for itself.
pub fn kept_by_windows(text: &str) -> bool {
    KEPT_BY_WINDOWS.iter().any(|k| same(k, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_two_slots_per_action_with_the_defaults_filled_in() {
        let config = Config::default();
        let table = table(&config);
        assert_eq!(table.len(), 11);
        let save = table
            .iter()
            .find(|(a, _)| *a == ActionId::ReplaySave)
            .unwrap();
        assert_eq!(save.1, ["Ctrl+Shift+S".to_owned(), String::new()]);
    }

    #[test]
    fn a_shortcut_keeps_its_order_in_canonical_names_and_can_be_cleared() {
        let mut c = Config::default();
        set(&mut c, ActionId::ReplayToggle, 0, "shift + ctrl + f9").unwrap();
        assert_eq!(c.hotkeys.replay_toggle, ["Shift+Ctrl+F9"]);
        set(&mut c, ActionId::ReplayToggle, 1, "Alt+F9").unwrap();
        assert_eq!(c.hotkeys.replay_toggle.len(), 2);
        set(&mut c, ActionId::ReplayToggle, 0, "").unwrap();
        assert_eq!(c.hotkeys.replay_toggle, ["Alt+F9"]);
    }

    #[test]
    fn refused_shortcuts_change_nothing() {
        let mut c = Config::default();
        let before = c.hotkeys.clone();
        assert!(matches!(
            set(&mut c, ActionId::ReplayToggle, 0, "Ctrl+Nope"),
            Err(Refusal::Invalid(_))
        ));
        // Ctrl+Shift+S belongs to the replay save.
        assert_eq!(
            set(&mut c, ActionId::ReplayToggle, 0, "Ctrl+Shift+S"),
            Err(Refusal::Conflict(ActionId::ReplaySave))
        );
        assert_eq!(c.hotkeys, before);
        // The same shortcut twice in one action.
        set(&mut c, ActionId::ReplayToggle, 0, "Alt+F9").unwrap();
        assert_eq!(
            set(&mut c, ActionId::ReplayToggle, 1, "Alt+F9"),
            Err(Refusal::Duplicate)
        );
    }

    #[test]
    fn the_shortcuts_of_windows_are_known_however_written() {
        assert!(kept_by_windows("Alt+Win+R"));
        assert!(kept_by_windows("Shift+Win+S"));
        assert!(!kept_by_windows("Ctrl+Shift+R"));
        assert!(same("Shift+Ctrl+KeyR", "Ctrl+Shift+R"));
        assert!(!same("", ""));
    }
}
