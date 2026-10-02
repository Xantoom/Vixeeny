// SPDX-License-Identifier: GPL-3.0-or-later
//! The conversion window (plan 5.5): files come from drag and drop, the buttons or the command
//! line (`vixeeny-app --convert <paths…>`, the Explorer context-menu entry); the work is done by
//! `vixeeny-convert` on every core.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_convert::{Choices, Job, OnExisting, Outcome};
use vixeeny_image::{ImageFormat, Settings, SourceFormat};
use vixeeny_ui::convert_panel::{ConvertTexts, enable_file_drop, new_window, on_close, set_files};
use vixeeny_ui::{ComponentHandle, ConvertWindow};

struct State {
    files: Vec<PathBuf>,
    formats: Vec<ImageFormat>,
    choices: Choices,
    base: Settings,
    existing: OnExisting,
    output: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
    lang: Lang,
}

fn texts(lang: Lang) -> ConvertTexts {
    let t = |k| tr(k, lang).to_owned();
    ConvertTexts {
        title: t(Key::ConvTitle),
        drop_hint: t(Key::ConvDropHint),
        add_files: t(Key::ConvAddFiles),
        add_folder: t(Key::ConvAddFolder),
        clear: t(Key::ConvClear),
        format: t(Key::ConvFormat),
        quality: t(Key::ConvQuality),
        lossless: t(Key::ConvLossless),
        existing: t(Key::ConvExisting),
        rename: t(Key::ConvRename),
        overwrite: t(Key::ConvOverwrite),
        skip: t(Key::ConvSkip),
        output: t(Key::ConvOutput),
        choose: t(Key::ConvChoose),
        reset: t(Key::ConvReset),
        convert: t(Key::ConvConvert),
        cancel: t(Key::ConvCancel),
    }
}

fn format_name(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::WebP => "WebP",
        ImageFormat::Avif => "AVIF",
        ImageFormat::Jxl => "JXL",
    }
}

/// Pushes the choices into the window's controls.
fn sync(w: &ConvertWindow, s: &State) {
    let index = s
        .formats
        .iter()
        .position(|f| *f == s.choices.format)
        .unwrap_or(0);
    w.set_format_index(index as i32);
    w.set_quality(i32::from(s.choices.quality));
    w.set_show_quality(s.choices.has_quality());
    w.set_show_lossless(s.choices.has_lossless());
    w.set_lossless(s.choices.lossless);
    w.set_existing_index(match s.existing {
        OnExisting::Rename => 0,
        OnExisting::Overwrite => 1,
        OnExisting::Skip => 2,
    });
    let output = s.output.as_ref().map_or_else(
        || tr(Key::ConvSameFolder, s.lang).to_owned(),
        |p| p.display().to_string(),
    );
    w.set_output_text(output.into());
}

fn refresh_files(w: &ConvertWindow, s: &State) {
    set_files(w, &s.files);
    w.set_summary(
        tr(Key::ConvSummary, s.lang)
            .replace("{count}", &s.files.len().to_string())
            .into(),
    );
}

fn add(w: &ConvertWindow, state: &Rc<RefCell<State>>, paths: &[PathBuf]) {
    let mut s = state.borrow_mut();
    let found = vixeeny_convert::collect(paths);
    if found.is_empty() {
        w.set_status(tr(Key::ConvNothing, s.lang).into());
        return;
    }
    for file in found {
        if !s.files.contains(&file) {
            s.files.push(file);
        }
    }
    w.set_status("".into());
    w.set_progress(0.0);
    refresh_files(w, &s);
}

fn start(w: &ConvertWindow, state: &Rc<RefCell<State>>) {
    let (files, job, cancel, lang) = {
        let s = state.borrow();
        if s.files.is_empty() {
            return;
        }
        s.cancel.store(false, Ordering::Relaxed);
        let job = Job {
            inputs: Vec::new(),
            format: s.choices.format,
            settings: s.choices.apply(s.base),
            output_dir: s.output.clone(),
            on_existing: s.existing,
            threads: 0,
        };
        (s.files.clone(), job, s.cancel.clone(), s.lang)
    };
    w.set_running(true);
    w.set_progress(0.0);
    w.set_status(
        tr(Key::ConvProgress, lang)
            .replace("{done}", "0")
            .replace("{total}", &files.len().to_string())
            .into(),
    );
    let weak = w.as_weak();
    let spawned = std::thread::Builder::new()
        .name("convert".into())
        .spawn(move || {
            let progress = weak.clone();
            let summary = vixeeny_convert::run(&files, &job, &cancel, &|event| {
                if let Outcome::Failed(error) = &event.outcome {
                    tracing::warn!("convert {}: {error}", event.input.display());
                }
                let text = tr(Key::ConvProgress, lang)
                    .replace("{done}", &event.done.to_string())
                    .replace("{total}", &event.total.to_string());
                let fraction = event.done as f32 / event.total.max(1) as f32;
                let _ = progress.upgrade_in_event_loop(move |w| {
                    w.set_progress(fraction);
                    w.set_status(text.into());
                });
            });
            let was_cancelled = cancel.load(Ordering::Relaxed);
            let text = if was_cancelled {
                tr(Key::ConvCancelled, lang)
                    .replace("{converted}", &summary.converted.to_string())
                    .replace("{failed}", &summary.failed.to_string())
                    .replace("{cancelled}", &summary.cancelled.to_string())
            } else {
                tr(Key::ConvDone, lang)
                    .replace("{converted}", &summary.converted.to_string())
                    .replace("{skipped}", &summary.skipped.to_string())
                    .replace("{failed}", &summary.failed.to_string())
            };
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_running(false);
                w.set_status(text.into());
            });
        });
    if let Err(e) = spawned {
        tracing::error!("cannot start the conversion: {e}");
        w.set_running(false);
    }
}

pub fn run(config: &Config, paths: &[PathBuf]) -> anyhow::Result<()> {
    let lang = crate::lang(&config.general.language);
    let (default_format, base) = crate::image_output(config);
    let formats: Vec<ImageFormat> = [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::WebP,
        ImageFormat::Avif,
        ImageFormat::Jxl,
    ]
    .into_iter()
    .filter(|f| f.available())
    .collect();
    let state = Rc::new(RefCell::new(State {
        files: Vec::new(),
        choices: Choices::from_settings(default_format, &base),
        formats: formats.clone(),
        base,
        existing: OnExisting::Rename,
        output: None,
        cancel: Arc::new(AtomicBool::new(false)),
        lang,
    }));
    let names: Vec<&str> = formats.iter().map(|f| format_name(*f)).collect();
    let w = new_window(&texts(lang), &names).map_err(|e| anyhow::anyhow!("window: {e}"))?;
    sync(&w, &state.borrow());
    refresh_files(&w, &state.borrow());
    add_initial(&w, &state, paths);

    let (weak, st) = (w.as_weak(), state.clone());
    w.on_add_files(move || {
        let Some(w) = weak.upgrade() else { return };
        let extensions: Vec<&str> = SourceFormat::EXTENSIONS.to_vec();
        if let Some(picked) = rfd::FileDialog::new()
            .add_filter("Images", &extensions)
            .pick_files()
        {
            add(&w, &st, &picked);
        }
    });
    let (weak, st) = (w.as_weak(), state.clone());
    w.on_add_folder(move || {
        let Some(w) = weak.upgrade() else { return };
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            add(&w, &st, &[dir]);
        }
    });
    let (weak, st) = (w.as_weak(), state.clone());
    w.on_clear(move || {
        let Some(w) = weak.upgrade() else { return };
        st.borrow_mut().files.clear();
        w.set_progress(0.0);
        w.set_status("".into());
        refresh_files(&w, &st.borrow());
    });
    let (weak, st) = (w.as_weak(), state.clone());
    w.on_format_chosen(move |i| {
        let Some(w) = weak.upgrade() else { return };
        {
            let mut s = st.borrow_mut();
            let Some(&format) = usize::try_from(i).ok().and_then(|i| s.formats.get(i)) else {
                return;
            };
            s.choices = Choices::from_settings(format, &s.base);
        }
        sync(&w, &st.borrow());
    });
    let (weak, st) = (w.as_weak(), state.clone());
    w.on_quality_step(move |delta| {
        let Some(w) = weak.upgrade() else { return };
        {
            let mut s = st.borrow_mut();
            let q = (i32::from(s.choices.quality) + delta).clamp(1, 100);
            s.choices.quality = q as u8;
        }
        sync(&w, &st.borrow());
    });
    let (weak, st) = (w.as_weak(), state.clone());
    w.on_lossless_toggled(move || {
        let Some(w) = weak.upgrade() else { return };
        {
            let mut s = st.borrow_mut();
            s.choices.lossless = !s.choices.lossless;
        }
        sync(&w, &st.borrow());
    });
    let (weak, st) = (w.as_weak(), state.clone());
    w.on_existing_chosen(move |i| {
        let Some(w) = weak.upgrade() else { return };
        st.borrow_mut().existing = match i {
            1 => OnExisting::Overwrite,
            2 => OnExisting::Skip,
            _ => OnExisting::Rename,
        };
        sync(&w, &st.borrow());
    });
    let (weak, st) = (w.as_weak(), state.clone());
    w.on_choose_output(move || {
        let Some(w) = weak.upgrade() else { return };
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            st.borrow_mut().output = Some(dir);
            sync(&w, &st.borrow());
        }
    });
    let (weak, st) = (w.as_weak(), state.clone());
    w.on_reset_output(move || {
        let Some(w) = weak.upgrade() else { return };
        st.borrow_mut().output = None;
        sync(&w, &st.borrow());
    });
    let (weak, st) = (w.as_weak(), state.clone());
    w.on_start(move || {
        if let Some(w) = weak.upgrade() {
            start(&w, &st);
        }
    });
    let st = state.clone();
    w.on_cancel(move || st.borrow().cancel.store(true, Ordering::Relaxed));
    let (weak, st) = (w.as_weak(), state.clone());
    enable_file_drop(&w, move |path| {
        if let Some(w) = weak.upgrade() {
            add(&w, &st, &[path]);
        }
    });
    // Closing the window stops the work.
    let st = state.clone();
    on_close(&w, move || {
        st.borrow().cancel.store(true, Ordering::Relaxed)
    });
    w.run().map_err(|e| anyhow::anyhow!("window: {e}"))
}

fn add_initial(w: &ConvertWindow, state: &Rc<RefCell<State>>, paths: &[PathBuf]) {
    if !paths.is_empty() {
        add(w, state, paths);
    }
}
