// SPDX-License-Identifier: GPL-3.0-or-later
//! Linux: the `tesseract` program (Apache-2.0, packaged by every distribution). It is run as a
//! child process rather than linked, so the application needs no Tesseract library and a user
//! without it only gets a message that says what to install. The image goes in as a BMP on
//! standard input, the text comes back on standard output.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::{Engine, Language, OcrError, OcrImage};

/// Tesseract language code, BCP-47 primary tag, English name.
const LANGUAGES: &[(&str, &str, &str)] = &[
    ("eng", "en", "English"),
    ("fra", "fr", "French"),
    ("deu", "de", "German"),
    ("spa", "es", "Spanish"),
    ("ita", "it", "Italian"),
    ("por", "pt", "Portuguese"),
    ("nld", "nl", "Dutch"),
    ("pol", "pl", "Polish"),
    ("swe", "sv", "Swedish"),
    ("tur", "tr", "Turkish"),
    ("rus", "ru", "Russian"),
    ("ukr", "uk", "Ukrainian"),
    ("ara", "ar", "Arabic"),
    ("hin", "hi", "Hindi"),
    ("jpn", "ja", "Japanese"),
    ("kor", "ko", "Korean"),
    ("chi_sim", "zh-CN", "Chinese (Simplified)"),
    ("chi_tra", "zh-TW", "Chinese (Traditional)"),
];

/// The BCP-47 tag for a Tesseract code (unknown codes stay as they are).
pub fn tag_of(code: &str) -> String {
    LANGUAGES
        .iter()
        .find(|(c, ..)| *c == code)
        .map_or_else(|| code.to_owned(), |(_, tag, _)| (*tag).to_owned())
}

/// The Tesseract code for a tag; `zh-Hant`/`zh-TW`/`zh-HK` are the traditional model.
pub fn code_of(tag: &str) -> String {
    let lower = tag.to_ascii_lowercase();
    if lower.starts_with("zh") {
        let traditional = ["hant", "tw", "hk", "mo"]
            .iter()
            .any(|s| lower.split(['-', '_']).any(|part| part == *s));
        return if traditional { "chi_tra" } else { "chi_sim" }.to_owned();
    }
    let primary = crate::lang::primary(tag);
    LANGUAGES
        .iter()
        .find(|(_, t, _)| crate::lang::primary(t) == primary)
        .map_or_else(|| tag.to_owned(), |(code, ..)| (*code).to_owned())
}

/// How to get Tesseract and one language's data, per distribution family.
pub fn install_command(tag: &str) -> String {
    let code = code_of(tag);
    let deb = code.replace('_', "-");
    format!(
        "apt install tesseract-ocr-{deb} · dnf install tesseract-langpack-{} · pacman -S tesseract-data-{code}",
        code.split('_').next().unwrap_or(&code),
    )
}

#[derive(Debug, Default)]
pub struct TesseractEngine;

impl TesseractEngine {
    pub fn new() -> Self {
        Self
    }
}

const NOT_INSTALLED: &str = "Tesseract is not installed (apt install tesseract-ocr · dnf install tesseract · pacman -S tesseract)";

fn run(args: &[&str], stdin: Option<&[u8]>) -> Result<String, OcrError> {
    let mut child = Command::new("tesseract")
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                OcrError::Engine(NOT_INSTALLED.into())
            } else {
                OcrError::Engine(e.to_string())
            }
        })?;
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // Tesseract reads the whole image before it writes anything: no deadlock. A broken
        // pipe means it already failed; its status below says why.
        let _ = pipe.write_all(data);
    }
    let output = child
        .wait_with_output()
        .map_err(|e| OcrError::Engine(e.to_string()))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(OcrError::Engine(err.trim().to_owned()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Lines of `tesseract --list-langs`, without its header and the `osd` (orientation) data.
fn parse_languages(listing: &str) -> Vec<Language> {
    listing
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|code| !code.is_empty() && *code != "osd" && !code.contains(' '))
        .map(|code| Language {
            tag: tag_of(code),
            display_name: LANGUAGES
                .iter()
                .find(|(c, ..)| *c == code)
                .map_or_else(|| code.to_owned(), |(.., name)| (*name).to_owned()),
        })
        .collect()
}

impl Engine for TesseractEngine {
    fn installed_languages(&self) -> Result<Vec<Language>, OcrError> {
        Ok(parse_languages(&run(&["--list-langs"], None)?))
    }

    fn max_dimension(&self) -> u32 {
        // Leptonica refuses images beyond 32 767 px; staying lower keeps memory sane.
        10_000
    }

    fn recognize(&self, image: &OcrImage, language: &str) -> Result<Vec<String>, OcrError> {
        let code = code_of(language);
        let text = run(&["stdin", "stdout", "-l", &code], Some(&image.to_bmp()))?;
        Ok(text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_languages_without_osd() {
        let langs = parse_languages(
            "List of available languages in \"/usr/share/tessdata/\" (4):\neng\nfra\nosd\nchi_sim\n",
        );
        let tags: Vec<_> = langs.iter().map(|l| l.tag.as_str()).collect();
        assert_eq!(tags, ["en", "fr", "zh-CN"]);
        assert_eq!(langs[1].display_name, "French");
    }

    #[test]
    fn tags_and_codes_round_trip() {
        assert_eq!(code_of("fr-FR"), "fra");
        assert_eq!(code_of("zh-Hant-TW"), "chi_tra");
        assert_eq!(code_of("zh-CN"), "chi_sim");
        assert_eq!(code_of("ja"), "jpn");
        assert_eq!(code_of("xyz"), "xyz");
        assert_eq!(tag_of("deu"), "de");
    }

    #[test]
    fn install_command_names_the_packages() {
        let c = install_command("zh-CN");
        assert!(c.contains("tesseract-ocr-chi-sim"));
        assert!(c.contains("tesseract-data-chi_sim"));
    }
}
