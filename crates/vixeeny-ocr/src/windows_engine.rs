// SPDX-License-Identifier: GPL-3.0-or-later
//! `Windows.Media.Ocr`.

use windows::Globalization::Language as WinLanguage;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::HSTRING;

use crate::{Engine, Language, OcrError, OcrImage};

fn engine_err(what: &str, e: windows::core::Error) -> OcrError {
    OcrError::Engine(format!("{what}: {e}"))
}

pub struct WindowsEngine;

impl WindowsEngine {
    pub fn new() -> Self {
        // WinRT needs COM on the calling thread. Already initialised (even in another mode) is
        // fine: the error is ignored on purpose.
        // SAFETY: plain initialisation call with a valid mode.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        Self
    }
}

impl Default for WindowsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for WindowsEngine {
    fn installed_languages(&self) -> Result<Vec<Language>, OcrError> {
        let list =
            OcrEngine::AvailableRecognizerLanguages().map_err(|e| engine_err("languages", e))?;
        let mut out = Vec::new();
        for lang in list {
            out.push(Language {
                tag: lang
                    .LanguageTag()
                    .map_err(|e| engine_err("tag", e))?
                    .to_string(),
                display_name: lang
                    .DisplayName()
                    .map_err(|e| engine_err("name", e))?
                    .to_string(),
            });
        }
        Ok(out)
    }

    fn max_dimension(&self) -> u32 {
        OcrEngine::MaxImageDimension().unwrap_or(2600).max(1)
    }

    fn recognize(&self, image: &OcrImage, language: &str) -> Result<Vec<String>, OcrError> {
        let lang = WinLanguage::CreateLanguage(&HSTRING::from(language))
            .map_err(|e| engine_err("language", e))?;
        let engine =
            OcrEngine::TryCreateFromLanguage(&lang).map_err(|e| engine_err("engine", e))?;
        let writer = DataWriter::new().map_err(|e| engine_err("writer", e))?;
        writer
            .WriteBytes(&image.bgra)
            .map_err(|e| engine_err("bytes", e))?;
        let buffer = writer.DetachBuffer().map_err(|e| engine_err("buffer", e))?;
        let width =
            i32::try_from(image.width).map_err(|_| OcrError::Engine("image too wide".into()))?;
        let height =
            i32::try_from(image.height).map_err(|_| OcrError::Engine("image too tall".into()))?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            width,
            height,
            BitmapAlphaMode::Ignore,
        )
        .map_err(|e| engine_err("bitmap", e))?;
        let result = engine
            .RecognizeAsync(&bitmap)
            .and_then(|op| op.join())
            .map_err(|e| engine_err("recognize", e))?;
        let mut lines = Vec::new();
        for line in result.Lines().map_err(|e| engine_err("lines", e))? {
            lines.push(line.Text().map_err(|e| engine_err("text", e))?.to_string());
        }
        Ok(lines)
    }
}
