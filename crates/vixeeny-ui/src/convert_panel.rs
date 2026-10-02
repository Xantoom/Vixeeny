// SPDX-License-Identifier: GPL-3.0-or-later
//! The batch conversion window (plan 5.5): texts, and file drops from the desktop.

use std::path::PathBuf;
use std::rc::Rc;

use slint::{ModelRc, SharedString, VecModel};

use crate::ConvertWindow;

/// Translated labels.
#[derive(Debug, Clone, Default)]
pub struct ConvertTexts {
    pub title: String,
    pub drop_hint: String,
    pub add_files: String,
    pub add_folder: String,
    pub clear: String,
    pub format: String,
    pub quality: String,
    pub lossless: String,
    pub existing: String,
    pub rename: String,
    pub overwrite: String,
    pub skip: String,
    pub output: String,
    pub choose: String,
    pub reset: String,
    pub convert: String,
    pub cancel: String,
}

fn strings(items: &[&str]) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(VecModel::from(
        items
            .iter()
            .map(|s| SharedString::from(*s))
            .collect::<Vec<_>>(),
    )))
}

/// A window with its labels set; `formats` are the chips of the format row.
pub fn new_window(
    texts: &ConvertTexts,
    formats: &[&str],
) -> Result<ConvertWindow, slint::PlatformError> {
    let w = ConvertWindow::new()?;
    w.set_window_title(texts.title.as_str().into());
    w.set_drop_hint(texts.drop_hint.as_str().into());
    w.set_add_files_label(texts.add_files.as_str().into());
    w.set_add_folder_label(texts.add_folder.as_str().into());
    w.set_clear_label(texts.clear.as_str().into());
    w.set_format_caption(texts.format.as_str().into());
    w.set_quality_caption(texts.quality.as_str().into());
    w.set_lossless_label(texts.lossless.as_str().into());
    w.set_existing_caption(texts.existing.as_str().into());
    w.set_existing_labels(strings(&[&texts.rename, &texts.overwrite, &texts.skip]));
    w.set_output_caption(texts.output.as_str().into());
    w.set_output_choose_label(texts.choose.as_str().into());
    w.set_output_reset_label(texts.reset.as_str().into());
    w.set_convert_label(texts.convert.as_str().into());
    w.set_cancel_label(texts.cancel.as_str().into());
    w.set_formats(strings(formats));
    Ok(w)
}

/// Calls `on_close` when the user closes the window (the window then goes away).
pub fn on_close(window: &ConvertWindow, on_close: impl Fn() + 'static) {
    use slint::ComponentHandle;
    window.window().on_close_requested(move || {
        on_close();
        slint::CloseRequestResponse::HideWindow
    });
}

/// Shows `files` (file names with their folder) in the list.
pub fn set_files(window: &ConvertWindow, files: &[PathBuf]) {
    let items: Vec<SharedString> = files
        .iter()
        .map(|p| SharedString::from(p.display().to_string()))
        .collect();
    window.set_files(ModelRc::from(Rc::new(VecModel::from(items))));
}

/// Calls `on_drop` for each file or folder dropped on the window and highlights the list while
/// something hovers over it. Needs the real (winit) backend; a no-op otherwise.
#[cfg(feature = "desktop")]
pub fn enable_file_drop(window: &ConvertWindow, on_drop: impl Fn(PathBuf) + 'static) {
    use slint::ComponentHandle;
    use slint::winit_030::winit::event::WindowEvent;
    use slint::winit_030::{EventResult, WinitWindowAccessor};
    let weak = window.as_weak();
    window.window().on_winit_window_event(move |_, event| {
        match event {
            WindowEvent::HoveredFile(_) => {
                if let Some(w) = weak.upgrade() {
                    w.set_hovering(true);
                }
            }
            WindowEvent::HoveredFileCancelled => {
                if let Some(w) = weak.upgrade() {
                    w.set_hovering(false);
                }
            }
            WindowEvent::DroppedFile(path) => {
                if let Some(w) = weak.upgrade() {
                    w.set_hovering(false);
                }
                on_drop(path.clone());
            }
            _ => {}
        }
        EventResult::Propagate
    });
}
