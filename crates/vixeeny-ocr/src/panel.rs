// SPDX-License-Identifier: GPL-3.0-or-later
//! What the OCR result window shows, built from a recognition result. The texts come from the
//! caller (translations live elsewhere); `{languages}`, `{language}`, `{command}` and `{error}`
//! are replaced.

use crate::{OcrError, Recognition};

/// Plain data for the window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OcrPanel {
    pub title: String,
    pub text: String,
    pub status: String,
    /// Explanation shown in a notice box; empty hides it.
    pub hint: String,
    pub copy_label: String,
    pub settings_label: String,
    /// Offer the button that opens the language settings.
    pub show_settings: bool,
}

/// Translated strings.
#[derive(Debug, Clone, Default)]
pub struct Texts {
    pub title: String,
    pub copy: String,
    pub open_settings: String,
    /// Status when the image holds no text.
    pub no_text: String,
    /// `{language}`
    pub language_used: String,
    /// Hint when some requested languages are missing: `{languages}`, `{command}`.
    pub missing_languages: String,
    /// Status when nothing could be read for lack of a language: `{languages}`.
    pub no_language: String,
    /// `{error}`
    pub failed: String,
}

fn fill(template: &str, pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .fold(template.to_owned(), |s, (k, v)| s.replace(k, v))
}

/// What the user runs to get the missing language: a Windows capability name.
fn install_command(tag: &str) -> String {
    crate::lang::install_hint(tag).capability
}

/// `display_name` turns a tag into the name to show (`en-US` → `English (United States)`).
pub fn describe(
    result: &Result<Recognition, OcrError>,
    texts: &Texts,
    display_name: &dyn Fn(&str) -> String,
) -> OcrPanel {
    let base = OcrPanel {
        title: texts.title.clone(),
        copy_label: texts.copy.clone(),
        settings_label: texts.open_settings.clone(),
        ..OcrPanel::default()
    };
    let hint_for = |missing: &[String]| {
        let commands: Vec<String> = missing.iter().map(|m| install_command(m)).collect();
        fill(
            &texts.missing_languages,
            &[
                ("{languages}", &missing.join(", ")),
                ("{command}", &commands.join(" ; ")),
            ],
        )
    };
    match result {
        Ok(r) => OcrPanel {
            text: r.text.clone(),
            status: if r.text.trim().is_empty() {
                texts.no_text.clone()
            } else {
                fill(
                    &texts.language_used,
                    &[("{language}", &display_name(&r.language))],
                )
            },
            hint: if r.missing.is_empty() {
                String::new()
            } else {
                hint_for(&r.missing)
            },
            show_settings: !r.missing.is_empty(),
            ..base
        },
        Err(OcrError::NoLanguage { missing, .. }) => OcrPanel {
            status: fill(&texts.no_language, &[("{languages}", &missing.join(", "))]),
            hint: hint_for(missing),
            show_settings: true,
            ..base
        },
        Err(e) => OcrPanel {
            status: fill(&texts.failed, &[("{error}", &e.to_string())]),
            ..base
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts() -> Texts {
        Texts {
            title: "T".into(),
            copy: "Copy".into(),
            open_settings: "Settings".into(),
            no_text: "No text".into(),
            language_used: "Language: {language}".into(),
            missing_languages: "Missing {languages}: {command}".into(),
            no_language: "No OCR for {languages}".into(),
            failed: "Failed: {error}".into(),
        }
    }

    fn name(tag: &str) -> String {
        format!("<{tag}>")
    }

    #[test]
    fn a_normal_result() {
        let r = Ok(Recognition {
            language: "en-US".into(),
            text: "Hi".into(),
            missing: vec![],
        });
        let p = describe(&r, &texts(), &name);
        assert_eq!(
            (p.text.as_str(), p.status.as_str(), p.hint.as_str()),
            ("Hi", "Language: <en-US>", "")
        );
        assert!(!p.show_settings);
        assert_eq!((p.title.as_str(), p.copy_label.as_str()), ("T", "Copy"));
    }

    #[test]
    fn missing_languages_come_with_the_install_command() {
        let r = Ok(Recognition {
            language: "en-US".into(),
            text: "Hi".into(),
            missing: vec!["ja".into(), "ko".into()],
        });
        let p = describe(&r, &texts(), &name);
        assert_eq!(
            p.hint,
            "Missing ja, ko: Language.OCR~~~ja-JP~0.0.1.0 ; Language.OCR~~~ko-KR~0.0.1.0"
        );
        assert!(p.show_settings);
    }

    #[test]
    fn empty_text_says_so() {
        let r = Ok(Recognition {
            language: "en-US".into(),
            text: " \n".into(),
            missing: vec![],
        });
        assert_eq!(describe(&r, &texts(), &name).status, "No text");
    }

    #[test]
    fn no_installed_language_is_explained() {
        let r = Err(OcrError::NoLanguage {
            missing: vec!["ja".into()],
            installed: vec![],
        });
        let p = describe(&r, &texts(), &name);
        assert_eq!(p.status, "No OCR for ja");
        assert!(p.hint.contains("Language.OCR~~~ja-JP"));
        assert!(p.show_settings && p.text.is_empty());
    }

    #[test]
    fn engine_failures_are_shown() {
        let p = describe(&Err(OcrError::Engine("boom".into())), &texts(), &name);
        assert_eq!(p.status, "Failed: OCR engine: boom");
        assert!(!p.show_settings);
    }
}
