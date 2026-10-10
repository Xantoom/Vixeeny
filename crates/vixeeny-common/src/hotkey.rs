// SPDX-License-Identifier: GPL-3.0-or-later
//! Keyboard shortcuts as written in the config (`Ctrl+Shift+R`): parsing, canonical form,
//! limits and conflicts (plan 5.1). No OS code here; the daemon registers the result.
//!
//! Key names inside [`Hotkey::code`] follow the W3C `KeyboardEvent.code` names (`KeyR`,
//! `Digit5`, `F5`, `PrintScreen`), which is what the registration backends use.

use std::fmt;

use crate::config::Hotkeys;
use crate::ipc::ActionId;

/// 0 to 2 shortcuts per action.
pub const MAX_PER_ACTION: usize = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Modifiers {
    fn is_empty(self) -> bool {
        !(self.ctrl || self.alt || self.shift || self.meta)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: Modifiers,
    /// `KeyboardEvent.code` name of the key.
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyError {
    #[error("empty shortcut")]
    Empty,
    #[error("a shortcut needs a key, not only modifiers")]
    ModifierOnly,
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("`{0}` needs at least one modifier (Ctrl, Alt, Shift or Win)")]
    NeedsModifier(String),
}

/// Keys that may be used without a modifier: they do not type text.
fn allowed_bare(code: &str) -> bool {
    matches!(code, "PrintScreen" | "Pause" | "ScrollLock") || is_function_key(code)
}

fn is_function_key(code: &str) -> bool {
    code.strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

/// Maps a user-written key to its `code` name.
fn key_code(token: &str) -> Option<String> {
    let lower = token.to_ascii_lowercase();
    let mut chars = lower.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_lowercase() {
            return Some(format!("Key{}", c.to_ascii_uppercase()));
        }
        if c.is_ascii_digit() {
            return Some(format!("Digit{c}"));
        }
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=24).contains(&n)
    {
        return Some(format!("F{n}"));
    }
    for (code, names) in KEY_NAMES {
        if names.contains(&lower.as_str()) {
            return Some((*code).to_owned());
        }
    }
    // Already a code name, e.g. `KeyR` or `Digit5`.
    let code_like = |prefix: &str, ok: fn(char) -> bool| {
        token
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.len() == 1 && rest.chars().all(ok))
    };
    if code_like("Key", |c| c.is_ascii_uppercase()) || code_like("Digit", |c| c.is_ascii_digit()) {
        return Some(token.to_owned());
    }
    None
}

const KEY_NAMES: &[(&str, &[&str])] = &[
    ("PrintScreen", &["printscreen", "prtsc", "prtscn", "print"]),
    ("Pause", &["pause", "break"]),
    ("ScrollLock", &["scrolllock"]),
    ("Space", &["space"]),
    ("Enter", &["enter", "return"]),
    ("Tab", &["tab"]),
    ("Insert", &["insert", "ins"]),
    ("Delete", &["delete", "del"]),
    ("Home", &["home"]),
    ("End", &["end"]),
    ("PageUp", &["pageup", "pgup"]),
    ("PageDown", &["pagedown", "pgdn"]),
    ("ArrowUp", &["arrowup", "up"]),
    ("ArrowDown", &["arrowdown", "down"]),
    ("ArrowLeft", &["arrowleft", "left"]),
    ("ArrowRight", &["arrowright", "right"]),
];

impl Hotkey {
    pub fn parse(text: &str) -> Result<Self, HotkeyError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(HotkeyError::Empty);
        }
        let mut mods = Modifiers::default();
        let mut code = None;
        for token in text.split('+').map(str::trim) {
            match modifier_name(token) {
                Some("Ctrl") => mods.ctrl = true,
                Some("Alt") => mods.alt = true,
                Some("Shift") => mods.shift = true,
                Some(_) => mods.meta = true,
                None => {
                    if code.is_some() || token.is_empty() {
                        return Err(HotkeyError::UnknownKey(text.to_owned()));
                    }
                    code = Some(
                        key_code(token).ok_or_else(|| HotkeyError::UnknownKey(token.to_owned()))?,
                    );
                }
            }
        }
        let code = code.ok_or(HotkeyError::ModifierOnly)?;
        if mods.is_empty() && !allowed_bare(&code) {
            return Err(HotkeyError::NeedsModifier(code));
        }
        Ok(Self { mods, code })
    }

    /// Spells `self` keeping the order its modifiers were written in (`Shift+Ctrl+R` stays so,
    /// as the user pressed them); `written` is the text that parsed into `self`.
    pub fn in_order(&self, written: &str) -> String {
        let mut out = String::new();
        for token in written.split('+').map(str::trim) {
            let Some(name) = modifier_name(token) else {
                continue;
            };
            if !out.split('+').any(|p| p == name) {
                out.push_str(name);
                out.push('+');
            }
        }
        out.push_str(self.key_name());
        out
    }

    /// The key without its modifiers, as written (`R`, `5`, `F8`).
    fn key_name(&self) -> &str {
        self.code
            .strip_prefix("Key")
            .or_else(|| self.code.strip_prefix("Digit"))
            .unwrap_or(&self.code)
    }
}

/// The canonical name of a modifier as written (`control` → `Ctrl`), `None` for a key.
fn modifier_name(token: &str) -> Option<&'static str> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some("Ctrl"),
        "alt" | "option" => Some("Alt"),
        "shift" => Some("Shift"),
        "win" | "meta" | "super" | "cmd" | "command" => Some("Win"),
        _ => None,
    }
}

impl fmt::Display for Hotkey {
    /// Canonical spelling, e.g. `Ctrl+Alt+Shift+Win+R`; parsing it gives back `self`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.mods.ctrl, "Ctrl"),
            (self.mods.alt, "Alt"),
            (self.mods.shift, "Shift"),
            (self.mods.meta, "Win"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(self.key_name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemKind {
    Invalid(HotkeyError),
    /// More than [`MAX_PER_ACTION`] shortcuts for one action.
    TooMany,
    /// Listed twice for the same action.
    Duplicate,
    /// Already bound to another action (blocking, plan 5.1).
    Conflict(ActionId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub action: ActionId,
    pub text: String,
    pub kind: ProblemKind,
}

/// The usable shortcuts of a config and what was left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolution {
    pub bindings: Vec<(ActionId, Hotkey)>,
    pub problems: Vec<Problem>,
}

/// Validates the configured shortcuts. The first claim on a combination wins, in
/// [`ActionId::ALL`] order; the others are reported and dropped.
pub fn resolve(hotkeys: &Hotkeys) -> Resolution {
    let mut out = Resolution::default();
    for (action, texts) in hotkeys.bindings() {
        let mut kept = 0;
        for text in texts {
            let mut problem = |kind| {
                out.problems.push(Problem {
                    action,
                    text: text.clone(),
                    kind,
                });
            };
            let hotkey = match Hotkey::parse(text) {
                Ok(h) => h,
                Err(e) => {
                    problem(ProblemKind::Invalid(e));
                    continue;
                }
            };
            if let Some((owner, _)) = out.bindings.iter().find(|(_, h)| *h == hotkey) {
                let kind = if *owner == action {
                    ProblemKind::Duplicate
                } else {
                    ProblemKind::Conflict(*owner)
                };
                problem(kind);
            } else if kept >= MAX_PER_ACTION {
                problem(ProblemKind::TooMany);
            } else {
                kept += 1;
                out.bindings.push((action, hotkey));
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    fn p(s: &str) -> Hotkey {
        Hotkey::parse(s).unwrap()
    }

    #[test]
    fn parses_defaults() {
        assert_eq!(p("PrintScreen").code, "PrintScreen");
        assert_eq!(
            p("Alt+PrintScreen").mods,
            Modifiers {
                alt: true,
                ..Default::default()
            }
        );
        let h = p("Ctrl+Shift+R");
        assert_eq!(h.code, "KeyR");
        assert!(h.mods.ctrl && h.mods.shift && !h.mods.alt);
    }

    #[test]
    fn spelling_is_forgiving_and_canonical() {
        assert_eq!(p("shift + control + r"), p("Ctrl+Shift+R"));
        assert_eq!(p("Ctrl+Shift+R").to_string(), "Ctrl+Shift+R");
        assert_eq!(p("super+ctrl+5").to_string(), "Ctrl+Win+5");
        assert_eq!(p("PrtSc"), p("PrintScreen"));
        assert_eq!(p("ctrl+f12").to_string(), "Ctrl+F12");
        let pressed = "shift+Control+KeyR";
        assert_eq!(p(pressed).in_order(pressed), "Shift+Ctrl+R");
        for s in ["Ctrl+Alt+PageDown", "Win+Space", "F5", "Ctrl+KeyQ"] {
            let h = p(s);
            assert_eq!(p(&h.to_string()), h);
        }
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(Hotkey::parse(""), Err(HotkeyError::Empty));
        assert_eq!(Hotkey::parse("Ctrl+Shift"), Err(HotkeyError::ModifierOnly));
        assert!(matches!(
            Hotkey::parse("Ctrl+Nope"),
            Err(HotkeyError::UnknownKey(_))
        ));
        assert!(matches!(
            Hotkey::parse("Ctrl+A+B"),
            Err(HotkeyError::UnknownKey(_))
        ));
        assert!(matches!(
            Hotkey::parse("Ctrl++"),
            Err(HotkeyError::UnknownKey(_))
        ));
        assert!(matches!(
            Hotkey::parse("R"),
            Err(HotkeyError::NeedsModifier(_))
        ));
        assert!(Hotkey::parse("F13").is_ok());
        assert!(Hotkey::parse("F25").is_err());
    }

    #[test]
    fn defaults_have_no_problem() {
        let r = resolve(&Hotkeys::default());
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.bindings.len(), 7);
    }

    #[test]
    fn at_most_two_per_action() {
        let mut k = Hotkeys::default();
        k.open_settings = ["Ctrl+1", "Ctrl+2", "Ctrl+3"].map(String::from).into();
        let r = resolve(&k);
        let count = r
            .bindings
            .iter()
            .filter(|(a, _)| *a == ActionId::OpenSettings)
            .count();
        assert_eq!(count, 2);
        assert_eq!(
            r.problems,
            vec![Problem {
                action: ActionId::OpenSettings,
                text: "Ctrl+3".into(),
                kind: ProblemKind::TooMany
            }]
        );
    }

    #[test]
    fn conflicts_block_the_later_action() {
        let mut k = Hotkeys::default();
        k.open_settings = vec!["shift+control+r".into(), "Ctrl+Shift+R".into()];
        let r = resolve(&k);
        assert!(
            r.bindings
                .contains(&(ActionId::RecordToggle, p("Ctrl+Shift+R")))
        );
        assert!(!r.bindings.iter().any(|(a, _)| *a == ActionId::OpenSettings));
        assert_eq!(r.problems.len(), 2);
        assert!(
            r.problems
                .iter()
                .all(|x| x.kind == ProblemKind::Conflict(ActionId::RecordToggle))
        );
    }

    #[test]
    fn duplicates_within_an_action_are_reported() {
        let mut k = Hotkeys::default();
        k.capture_scrolling = vec!["Ctrl+9".into(), "ctrl+9".into()];
        let r = resolve(&k);
        assert_eq!(r.problems[0].kind, ProblemKind::Duplicate);
    }

    #[test]
    fn invalid_entries_do_not_stop_the_others() {
        let mut k = Hotkeys::default();
        k.capture_region = vec!["Nope".into(), "Ctrl+Alt+P".into()];
        let r = resolve(&k);
        assert_eq!(r.problems.len(), 1);
        assert!(
            r.bindings
                .contains(&(ActionId::CaptureRegion, p("Ctrl+Alt+P")))
        );
    }
}
