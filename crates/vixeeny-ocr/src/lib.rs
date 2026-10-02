// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-ocr — text recognition with the engine of the OS (plan 5.6): `Windows.Media.Ocr`
//! today; Vision (macOS) and Tesseract (Linux) arrive with M20–M22. Language selection and
//! result choice are pure code behind the [`Engine`] trait, tested with a fake engine.

mod image;
pub mod lang;
pub mod panel;
#[cfg(windows)]
mod windows_engine;

pub use image::OcrImage;
pub use lang::{InstallHint, LanguagePlan, install_hint};
pub use panel::{OcrPanel, Texts, describe};
#[cfg(windows)]
pub use windows_engine::WindowsEngine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    /// BCP-47 tag as the engine spells it (`en-US`, `ja`).
    pub tag: String,
    /// Name to show to the user.
    pub display_name: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OcrError {
    /// None of the requested languages has an engine installed (CA-OCR-2): never a crash, the
    /// caller shows how to install them.
    #[error("no OCR language available (missing: {})", missing.join(", "))]
    NoLanguage {
        missing: Vec<String>,
        /// What the engine does have.
        installed: Vec<String>,
    },
    #[error("OCR is not supported on this platform yet")]
    Unsupported,
    #[error("OCR engine: {0}")]
    Engine(String),
}

pub trait Engine {
    /// Languages with a recognizer installed.
    fn installed_languages(&self) -> Result<Vec<Language>, OcrError>;
    /// Largest image side the engine accepts.
    fn max_dimension(&self) -> u32;
    /// Recognises one image in one language; returns the text lines in reading order.
    fn recognize(&self, image: &OcrImage, language: &str) -> Result<Vec<String>, OcrError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recognition {
    /// Language whose result was kept.
    pub language: String,
    /// Lines joined with `\n`.
    pub text: String,
    /// Requested languages that are not installed (to explain how to add them).
    pub missing: Vec<String>,
}

/// Reads `image`: every usable language in `requested` is tried and the result that best fits its
/// own writing system is kept (see [`lang::script_score`]). `ui_language` resolves `auto`.
pub fn recognize(
    engine: &dyn Engine,
    image: OcrImage,
    requested: &[String],
    ui_language: &str,
) -> Result<Recognition, OcrError> {
    let installed = engine.installed_languages()?;
    let tags: Vec<String> = installed.iter().map(|l| l.tag.clone()).collect();
    let plan = lang::plan(requested, &tags, ui_language);
    if plan.usable.is_empty() {
        return Err(OcrError::NoLanguage {
            missing: plan.missing,
            installed: tags,
        });
    }
    let image = image.fit_within(engine.max_dimension());
    let mut best: Option<(f32, String, String)> = None;
    let mut last_error = None;
    for tag in &plan.usable {
        match engine.recognize(&image, tag) {
            Ok(lines) => {
                let text = lines.join("\n");
                let score = lang::script_score(tag, &text);
                // `>` keeps the earlier (higher priority) language on ties.
                if best.as_ref().is_none_or(|(s, ..)| score > *s) {
                    best = Some((score, tag.clone(), text));
                }
            }
            Err(e) => {
                tracing::warn!("OCR in {tag} failed: {e}");
                last_error = Some(e);
            }
        }
    }
    match best {
        Some((_, language, text)) => Ok(Recognition {
            language,
            text,
            missing: plan.missing,
        }),
        None => Err(last_error.unwrap_or(OcrError::Engine("no result".into()))),
    }
}

/// The engine of this platform, if there is one.
#[cfg(windows)]
pub fn system_engine() -> Result<Box<dyn Engine>, OcrError> {
    Ok(Box::new(WindowsEngine::new()))
}

#[cfg(not(windows))]
pub fn system_engine() -> Result<Box<dyn Engine>, OcrError> {
    Err(OcrError::Unsupported)
}

#[cfg(test)]
mod tests;
