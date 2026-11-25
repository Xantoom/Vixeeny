// SPDX-License-Identifier: GPL-3.0-or-later
//! What changed in the version running, built into the program from `changelog/<version>/` (see `build.rs`). The
//! release workflow publishes the same files: the English notes on the release page, the French ones in a comment of it
//! and both in `latest.json`.

use vixeeny_common::i18n::Lang;

mod built {
    include!(concat!(env!("OUT_DIR"), "/changelog.rs"));
}

/// The notes of the version running, in `lang` when they exist in it, else in English; `None` for a build without
/// notes.
pub fn running(lang: Lang) -> Option<&'static str> {
    pick(built::EN, built::FR, lang)
}

fn pick(en: Option<&'static str>, fr: Option<&'static str>, lang: Lang) -> Option<&'static str> {
    let en = en.map(str::trim).filter(|t| !t.is_empty());
    let fr = fr.map(str::trim).filter(|t| !t.is_empty());
    match lang {
        Lang::Fr => fr.or(en),
        Lang::En => en,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_french_notes_fall_back_to_english() {
        assert_eq!(pick(Some("- A\n"), Some("- B"), Lang::Fr), Some("- B"));
        assert_eq!(pick(Some("- A\n"), None, Lang::Fr), Some("- A"));
        assert_eq!(pick(Some("- A"), Some(" "), Lang::Fr), Some("- A"));
        assert_eq!(pick(None, Some("- B"), Lang::En), None);
    }

    /// A released version has its notes in both languages (the release workflow checks it too).
    #[test]
    fn a_release_has_notes_in_both_languages() {
        let version = env!("CARGO_PKG_VERSION");
        if built::EN.is_some() || built::FR.is_some() {
            assert!(built::EN.is_some() && built::FR.is_some(), "changelog/{version}: one language is missing");
        }
    }
}
