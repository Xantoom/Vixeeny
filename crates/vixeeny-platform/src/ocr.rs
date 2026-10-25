// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading the text of a picture with the OCR engine built into Windows 10 and 11 (`Windows.Media.Ocr`): offline, in
//! the languages the user installed with their recognition pack. Small pictures are enlarged first: the engine misses
//! text only a few pixels high.

/// Why nothing could be read.
#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    /// No installed language comes with text recognition.
    #[error("no language with text recognition is installed")]
    NoLanguage,
    #[error("{0}")]
    Os(String),
}

impl From<windows::core::Error> for OcrError {
    fn from(e: windows::core::Error) -> Self {
        Self::Os(format!("text recognition: {e}"))
    }
}

/// How the picture is resized before it is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resize {
    /// Twice as large: small text is read far better.
    Double,
    Keep,
    /// Divided by this factor: the engine refuses pictures larger than its limit.
    Shrink(u32),
}

fn resize_for(width: u32, height: u32, max: u32) -> Resize {
    let side = width.max(height);
    if side > max {
        Resize::Shrink(side.div_ceil(max))
    } else if side * 2 <= max {
        Resize::Double
    } else {
        Resize::Keep
    }
}

/// Twice as large, interpolated (top-down BGRA, tightly packed).
fn double(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let at = |x: usize, y: usize, c: usize| f32::from(bgra[(y.min(h - 1) * w + x.min(w - 1)) * 4 + c]);
    let mut out = vec![0; w * h * 16];
    for y in 0..h * 2 {
        // Output pixel centres sit a quarter of a source pixel either side of the source centres.
        let (y0, fy) =
            if y % 2 == 0 { ((y / 2).saturating_sub(1), if y == 0 { 0.0 } else { 0.75 }) } else { (y / 2, 0.25) };
        for x in 0..w * 2 {
            let (x0, fx) =
                if x % 2 == 0 { ((x / 2).saturating_sub(1), if x == 0 { 0.0 } else { 0.75 }) } else { (x / 2, 0.25) };
            for c in 0..4 {
                let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1, y0, c) * fx;
                let bottom = at(x0, y0 + 1, c) * (1.0 - fx) + at(x0 + 1, y0 + 1, c) * fx;
                out[(y * w * 2 + x) * 4 + c] = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
    }
    out
}

/// Divided by `factor`, each output pixel the mean of its block.
fn shrink(width: u32, height: u32, bgra: &[u8], factor: u32) -> (u32, u32, Vec<u8>) {
    let (ow, oh) = (width / factor, height / factor);
    let (w, f) = (width as usize, factor as usize);
    let mut out = Vec::with_capacity(ow as usize * oh as usize * 4);
    for oy in 0..oh as usize {
        for ox in 0..ow as usize {
            let mut sum = [0u32; 4];
            for y in oy * f..(oy + 1) * f {
                for x in ox * f..(ox + 1) * f {
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += u32::from(bgra[(y * w + x) * 4 + c]);
                    }
                }
            }
            out.extend(sum.map(|s| (s / (f * f) as u32) as u8));
        }
    }
    (ow, oh, out)
}

/// The text of a picture (top-down BGRA, tightly packed), one line per line read. Empty when there is none.
pub fn recognize(width: u32, height: u32, bgra: &[u8]) -> Result<String, OcrError> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;

    if width == 0 || height == 0 || bgra.len() < width as usize * height as usize * 4 {
        return Err(OcrError::Os("text recognition: empty picture".into()));
    }
    let engine = match OcrEngine::TryCreateFromUserProfileLanguages() {
        Ok(engine) => engine,
        // None of the user's languages has recognition: any installed one that has.
        Err(_) => {
            let languages = OcrEngine::AvailableRecognizerLanguages()?;
            if languages.Size()? == 0 {
                return Err(OcrError::NoLanguage);
            }
            OcrEngine::TryCreateFromLanguage(&languages.GetAt(0)?)?
        }
    };
    let (width, height, pixels) = match resize_for(width, height, OcrEngine::MaxImageDimension()?) {
        Resize::Double => (width * 2, height * 2, double(width, height, bgra)),
        Resize::Keep => (width, height, bgra[..width as usize * height as usize * 4].to_vec()),
        Resize::Shrink(factor) => shrink(width, height, bgra, factor),
    };
    let buffer = CryptographicBuffer::CreateFromByteArray(&pixels)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, width as i32, height as i32)?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let mut lines = Vec::new();
    for line in result.Lines()? {
        lines.push(line.Text()?.to_string_lossy());
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_resized_to_what_the_engine_reads_best() {
        assert_eq!(resize_for(800, 300, 10_000), Resize::Double);
        assert_eq!(resize_for(6000, 300, 10_000), Resize::Keep);
        assert_eq!(resize_for(10_000, 300, 10_000), Resize::Keep);
        assert_eq!(resize_for(25_000, 300, 10_000), Resize::Shrink(3));
    }

    #[test]
    fn doubling_keeps_flat_areas_and_blends_edges() {
        // Two pixels: black, white.
        let src = [0, 0, 0, 255, 255, 255, 255, 255];
        let out = double(2, 1, &src);
        let blue: Vec<u8> = out.chunks(4).map(|p| p[0]).collect();
        assert_eq!(blue, [0, 64, 191, 255, 0, 64, 191, 255]);
        assert!(out.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn shrinking_averages_blocks() {
        let src: Vec<u8> = [0u8, 100, 200, 60].iter().flat_map(|&v| [v, v, v, 255]).collect();
        let (w, h, out) = shrink(2, 2, &src, 2);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, [90, 90, 90, 255]);
    }

    /// The real engine on a synthetic picture: works where a recognition language is installed.
    #[test]
    fn reads_nothing_from_a_blank_picture() {
        match recognize(64, 32, &[255; 64 * 32 * 4]) {
            Ok(text) => assert!(text.trim().is_empty(), "{text:?}"),
            Err(e) => eprintln!("no OCR here: {e}"),
        }
    }
}
