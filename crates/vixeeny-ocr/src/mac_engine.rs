// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS: Vision's `VNRecognizeTextRequest`. The image is handed over as an in-memory BMP, a
//! format every macOS version decodes, so no CoreGraphics image has to be built by hand.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

use crate::{Engine, Language, OcrError, OcrImage};

#[derive(Debug, Default)]
pub struct VisionEngine;

impl VisionEngine {
    pub fn new() -> Self {
        Self
    }
}

/// Top-down 32-bit BMP of a BGRA buffer (alpha forced opaque: screen captures have none).
fn bmp(image: &OcrImage) -> Vec<u8> {
    let pixels = image.bgra.len();
    let mut out = Vec::with_capacity(54 + pixels);
    let size = u32::try_from(54 + pixels).unwrap_or(u32::MAX);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&image.width.to_le_bytes());
    // A negative height means rows run top to bottom.
    out.extend_from_slice(&(-i32::try_from(image.height).unwrap_or(i32::MAX)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // BI_RGB
    out.extend_from_slice(&u32::try_from(pixels).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for px in image.bgra.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    out
}

impl Engine for VisionEngine {
    fn installed_languages(&self) -> Result<Vec<Language>, OcrError> {
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        // SAFETY: plain query on a request we own.
        let tags = unsafe { request.supportedRecognitionLanguagesAndReturnError() }
            .map_err(|e| OcrError::Engine(e.localizedDescription().to_string()))?;
        Ok(tags
            .iter()
            .map(|tag| {
                let tag = tag.to_string();
                Language {
                    display_name: tag.clone(),
                    tag,
                }
            })
            .collect())
    }

    fn max_dimension(&self) -> u32 {
        16_384
    }

    fn recognize(&self, image: &OcrImage, language: &str) -> Result<Vec<String>, OcrError> {
        let data = NSData::with_bytes(&bmp(image));
        let options: Retained<NSDictionary<_, AnyObject>> = NSDictionary::new();
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &options,
        );
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&[NSString::from_str(
            language,
        )]));
        let as_request: &VNRequest = &request;
        handler
            .performRequests_error(&NSArray::from_slice(&[as_request]))
            .map_err(|e| OcrError::Engine(e.localizedDescription().to_string()))?;
        let mut lines = Vec::new();
        if let Some(results) = request.results() {
            for observation in results.iter() {
                if let Some(best) = observation.topCandidates(1).iter().next() {
                    lines.push(best.string().to_string());
                }
            }
        }
        Ok(lines)
    }
}
