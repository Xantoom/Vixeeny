// SPDX-License-Identifier: GPL-3.0-or-later
//! Which OCR languages to use and how to pick the best result (plan 5.6). Pure and testable.

/// Primary subtag of a BCP-47 tag, lower-cased: `ja-JP` → `ja`.
pub fn primary(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The languages to try, in priority order, and the requested ones that are not installed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LanguagePlan {
    /// Installed tags (as the engine spells them), best first, without duplicates.
    pub usable: Vec<String>,
    /// Requested primary subtags with no installed engine.
    pub missing: Vec<String>,
}

/// `requested` comes from `[ocr] languages`; `auto` stands for the interface language, then
/// English. `installed` are the engine's tags (e.g. `en-US`, `ja`).
pub fn plan(requested: &[String], installed: &[String], ui_language: &str) -> LanguagePlan {
    let mut wanted: Vec<String> = Vec::new();
    let mut push = |p: String| {
        if !p.is_empty() && !wanted.contains(&p) {
            wanted.push(p);
        }
    };
    for r in requested {
        if r.eq_ignore_ascii_case("auto") {
            push(primary(ui_language));
            push("en".to_owned());
        } else {
            push(primary(r));
        }
    }
    let mut out = LanguagePlan::default();
    for want in wanted {
        match installed.iter().find(|tag| primary(tag) == want) {
            Some(tag) if !out.usable.contains(tag) => out.usable.push(tag.clone()),
            Some(_) => {}
            None => out.missing.push(want),
        }
    }
    out
}

/// How to install the OCR module of a language on Windows (plan 5.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallHint {
    /// `Add-WindowsCapability -Online -Name <capability>` (elevated PowerShell).
    pub capability: String,
    /// Opens the language page of the Windows settings.
    pub settings_uri: &'static str,
}

pub fn install_hint(tag: &str) -> InstallHint {
    let full = match primary(tag).as_str() {
        "ja" => "ja-JP".to_owned(),
        "ko" => "ko-KR".to_owned(),
        "zh" => "zh-CN".to_owned(),
        "en" => "en-US".to_owned(),
        "de" => "de-DE".to_owned(),
        "fr" => "fr-FR".to_owned(),
        "es" => "es-ES".to_owned(),
        "it" => "it-IT".to_owned(),
        "pt" => "pt-BR".to_owned(),
        "ru" => "ru-RU".to_owned(),
        _ => tag.to_owned(),
    };
    InstallHint {
        capability: format!("Language.OCR~~~{full}~0.0.1.0"),
        settings_uri: "ms-settings:regionlanguage",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    Latin,
    Cyrillic,
    Greek,
    Han,
    Kana,
    Hangul,
    Arabic,
    Hebrew,
    Thai,
    Devanagari,
    /// Digits, punctuation, spaces: valid in every language.
    Neutral,
    Other,
}

fn script_of(c: char) -> Script {
    match c as u32 {
        _ if c.is_whitespace() || c.is_ascii_digit() || c.is_ascii_punctuation() => Script::Neutral,
        0x00A0..=0x00BF | 0x2000..=0x206F | 0x3000..=0x303F | 0xFF00..=0xFF0F => Script::Neutral,
        0x0041..=0x024F | 0x1E00..=0x1EFF => Script::Latin,
        0x0370..=0x03FF => Script::Greek,
        0x0400..=0x052F => Script::Cyrillic,
        0x0590..=0x05FF => Script::Hebrew,
        0x0600..=0x06FF | 0x0750..=0x077F => Script::Arabic,
        0x0900..=0x097F => Script::Devanagari,
        0x0E00..=0x0E7F => Script::Thai,
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Script::Kana,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => Script::Han,
        0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => Script::Hangul,
        _ => Script::Other,
    }
}

fn allowed(tag: &str, s: Script) -> bool {
    use Script::{
        Arabic, Cyrillic, Devanagari, Greek, Han, Hangul, Hebrew, Kana, Latin, Neutral, Thai,
    };
    if s == Neutral {
        return true;
    }
    match primary(tag).as_str() {
        "ja" => matches!(s, Han | Kana | Latin),
        "ko" => matches!(s, Hangul | Han | Latin),
        "zh" => matches!(s, Han | Latin),
        "ru" | "uk" | "bg" | "sr" | "be" | "mk" => matches!(s, Cyrillic | Latin),
        "el" => matches!(s, Greek | Latin),
        "ar" | "fa" | "ur" => matches!(s, Arabic | Latin),
        "he" => matches!(s, Hebrew | Latin),
        "th" => matches!(s, Thai | Latin),
        "hi" | "mr" | "ne" => matches!(s, Devanagari | Latin),
        _ => s == Latin,
    }
}

/// How well `text` fits the writing system of `tag`: characters of its scripts count +1, other
/// letters −½. When several engines read the same image, the best score wins; a Latin engine
/// cannot score on Japanese text and a Japanese engine reading Latin text scores the same as the
/// Latin one (then the priority order decides).
pub fn script_score(tag: &str, text: &str) -> f32 {
    text.chars()
        .map(|c| match script_of(c) {
            Script::Neutral => 0.0,
            s if allowed(tag, s) => 1.0,
            _ => -0.5,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn primary_subtag() {
        assert_eq!(primary("ja-JP"), "ja");
        assert_eq!(primary("EN_us"), "en");
        assert_eq!(primary(""), "");
    }

    #[test]
    fn auto_is_the_interface_language_then_english() {
        let installed = v(&["en-US", "fr-FR", "de-DE"]);
        let p = plan(&v(&["auto"]), &installed, "fr");
        assert_eq!(p.usable, ["fr-FR", "en-US"]);
        assert!(p.missing.is_empty());
        let p = plan(&v(&["auto"]), &installed, "en-GB");
        assert_eq!(p.usable, ["en-US"]);
    }

    #[test]
    fn explicit_languages_keep_their_priority_and_report_the_missing() {
        let installed = v(&["en-US", "de-DE"]);
        let p = plan(&v(&["ja", "de", "ko", "en"]), &installed, "fr");
        assert_eq!(p.usable, ["de-DE", "en-US"]);
        assert_eq!(p.missing, ["ja", "ko"]);
    }

    #[test]
    fn nothing_installed_is_reported_not_fatal() {
        let p = plan(&v(&["ja"]), &[], "en");
        assert!(p.usable.is_empty());
        assert_eq!(p.missing, ["ja"]);
        let p = plan(&[], &v(&["en-US"]), "en");
        assert_eq!(p, LanguagePlan::default());
    }

    #[test]
    fn duplicates_collapse() {
        let p = plan(&v(&["en", "en-GB", "auto"]), &v(&["en-US"]), "en");
        assert_eq!(p.usable, ["en-US"]);
    }

    #[test]
    fn install_hints() {
        let h = install_hint("ja");
        assert_eq!(h.capability, "Language.OCR~~~ja-JP~0.0.1.0");
        assert_eq!(h.settings_uri, "ms-settings:regionlanguage");
        assert_eq!(
            install_hint("sv-SE").capability,
            "Language.OCR~~~sv-SE~0.0.1.0"
        );
    }

    #[test]
    fn scores_follow_the_script() {
        assert!(script_score("ja", "日本語のテキスト") > 5.0);
        assert!(script_score("en-US", "日本語のテキスト") < 0.0);
        assert!(script_score("ko", "안녕하세요") > 4.0);
        assert!(script_score("ja", "안녕하세요") < 0.0);
        assert_eq!(
            script_score("de", "Größe 12, ok?"),
            script_score("en", "Größe 12, ok?")
        );
        assert!(script_score("ru", "Привет мир") > 8.0);
        assert_eq!(script_score("en", "  123 !? "), 0.0);
        assert!(script_score("ja", "Hello 世界") > script_score("en", "Hello 世界"));
    }
}
