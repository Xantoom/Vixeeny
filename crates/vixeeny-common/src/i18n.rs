// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal translations for the daemon, which links neither Slint nor gettext (plan 4.1).
//! The app and settings UI use Slint's `@tr()`; only the tray menu and notifications of the
//! daemon go through this table. French and English are shipped (plan 5.16).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Fr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    TrayTooltip,
    TrayTooltipRecording,
    MenuSettings,
    MenuQuit,
    AppCrashed,
    HotkeysUnavailable,
}

impl Lang {
    /// Parses a BCP-47-ish tag (`fr`, `fr-FR`, `fr_FR.UTF-8`); unknown languages fall back to
    /// English.
    pub fn from_tag(tag: &str) -> Self {
        let primary = tag
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match primary.as_str() {
            "fr" => Self::Fr,
            _ => Self::En,
        }
    }

    /// Resolves the `general.language` setting. `auto` uses `os_locale`, the tag reported by
    /// the OS, when available, else the `LC_ALL` / `LC_MESSAGES` / `LANG` variables.
    pub fn resolve(setting: &str, os_locale: Option<&str>) -> Self {
        if setting != "auto" {
            return Self::from_tag(setting);
        }
        if let Some(tag) = os_locale {
            return Self::from_tag(tag);
        }
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty()))
            .map_or(Self::En, |tag| Self::from_tag(&tag))
    }
}

pub fn tr(key: Key, lang: Lang) -> &'static str {
    match (key, lang) {
        (Key::TrayTooltip, _) => "Vixeeny",
        (Key::TrayTooltipRecording, Lang::En) => "Vixeeny — recording",
        (Key::TrayTooltipRecording, Lang::Fr) => "Vixeeny — enregistrement",
        (Key::MenuSettings, Lang::En) => "Settings",
        (Key::MenuSettings, Lang::Fr) => "Paramètres",
        (Key::MenuQuit, Lang::En) => "Quit",
        (Key::MenuQuit, Lang::Fr) => "Quitter",
        (Key::AppCrashed, Lang::En) => "Vixeeny stopped unexpectedly",
        (Key::AppCrashed, Lang::Fr) => "Vixeeny s'est arrêté de façon inattendue",
        (Key::HotkeysUnavailable, Lang::En) => {
            "Some shortcuts could not be registered (already used elsewhere?)"
        }
        (Key::HotkeysUnavailable, Lang::Fr) => {
            "Certains raccourcis n'ont pas pu être enregistrés (déjà utilisés ailleurs ?)"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        assert_eq!(Lang::from_tag("fr"), Lang::Fr);
        assert_eq!(Lang::from_tag("fr-FR"), Lang::Fr);
        assert_eq!(Lang::from_tag("fr_CA.UTF-8"), Lang::Fr);
        assert_eq!(Lang::from_tag("en-US"), Lang::En);
        assert_eq!(Lang::from_tag("de"), Lang::En);
        assert_eq!(Lang::from_tag(""), Lang::En);
    }

    #[test]
    fn explicit_setting_wins() {
        assert_eq!(Lang::resolve("fr", Some("en-US")), Lang::Fr);
        assert_eq!(Lang::resolve("auto", Some("fr-FR")), Lang::Fr);
    }
}
