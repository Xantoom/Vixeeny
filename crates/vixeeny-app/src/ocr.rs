// SPDX-License-Identifier: GPL-3.0-or-later
//! Text recognition of a zone (plan 5.6): recognise, copy the text, show it in an editable
//! window with the way to install a missing language.

use std::collections::BTreeMap;

use anyhow::Context;
use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_editor::RgbaImage;
use vixeeny_ocr::{Engine, OcrImage, Texts, WindowsEngine};

fn texts(lang: Lang) -> Texts {
    Texts {
        title: tr(Key::OcrTitle, lang).into(),
        copy: tr(Key::OcrCopy, lang).into(),
        open_settings: tr(Key::OcrOpenSettings, lang).into(),
        no_text: tr(Key::OcrNoText, lang).into(),
        language_used: tr(Key::OcrLanguageUsed, lang).into(),
        missing_languages: tr(Key::OcrMissingLanguages, lang).into(),
        no_language: tr(Key::OcrNoLanguage, lang).into(),
        failed: tr(Key::OcrFailed, lang).into(),
    }
}

fn to_bgra(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len());
    for px in img.data.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}

pub fn run(image: &RgbaImage, config: &Config) -> anyhow::Result<()> {
    let lang = Lang::resolve(&config.general.language, None);
    let ui_language = if lang == Lang::Fr { "fr" } else { "en" };
    let ocr_image = OcrImage::new(image.width, image.height, to_bgra(image))
        .context("unexpected image buffer")?;
    let requested = config.ocr.languages.clone();

    // WinRT calls block: keep them off the UI thread.
    let (result, names) = std::thread::spawn(move || {
        let engine = WindowsEngine::new();
        let names: BTreeMap<String, String> = engine
            .installed_languages()
            .map(|l| l.into_iter().map(|l| (l.tag, l.display_name)).collect())
            .unwrap_or_default();
        (
            vixeeny_ocr::recognize(&engine, ocr_image, &requested, ui_language),
            names,
        )
    })
    .join()
    .map_err(|_| anyhow::anyhow!("the OCR thread panicked"))?;

    match &result {
        Ok(r) => tracing::info!(
            "OCR: {} characters in {}",
            r.text.chars().count(),
            r.language
        ),
        Err(e) => tracing::warn!("OCR: {e}"),
    }
    let panel = vixeeny_ocr::describe(&result, &texts(lang), &|tag| {
        names.get(tag).cloned().unwrap_or_else(|| tag.to_owned())
    });
    if let Ok(r) = &result
        && !r.text.trim().is_empty()
        && let Err(e) = vixeeny_platform::clipboard::copy_text(&r.text)
    {
        tracing::warn!("clipboard: {e}");
    }
    vixeeny_ui::ocr_panel::show(
        &panel,
        |text| {
            if let Err(e) = vixeeny_platform::clipboard::copy_text(text) {
                tracing::warn!("clipboard: {e}");
            }
        },
        || {
            let _ = std::process::Command::new("explorer.exe")
                .arg("ms-settings:regionlanguage")
                .spawn();
        },
    )
    .map_err(|e| anyhow::anyhow!("OCR window: {e}"))
}
