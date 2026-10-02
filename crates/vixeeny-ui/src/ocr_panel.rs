// SPDX-License-Identifier: GPL-3.0-or-later
//! The OCR result window (plan 5.6): editable text, a "Copy" button, and the way to install a
//! missing language.

use slint::ComponentHandle;

use vixeeny_ocr::OcrPanel;

use crate::OcrWindow;

/// Opens the window and returns when it is closed. `on_copy` receives the (possibly edited)
/// text; `on_open_settings` opens the OS language page.
pub fn show(
    panel: &OcrPanel,
    on_copy: impl Fn(&str) + 'static,
    on_open_settings: impl Fn() + 'static,
) -> Result<(), slint::PlatformError> {
    let window = build(panel)?;
    window.on_copy(move |text| on_copy(&text));
    window.on_open_settings(on_open_settings);
    window.run()
}

pub(crate) fn build(panel: &OcrPanel) -> Result<OcrWindow, slint::PlatformError> {
    let window = OcrWindow::new()?;
    window.set_window_title(panel.title.as_str().into());
    window.set_text(panel.text.as_str().into());
    window.set_status(panel.status.as_str().into());
    window.set_hint(panel.hint.as_str().into());
    window.set_copy_label(panel.copy_label.as_str().into());
    window.set_settings_label(panel.settings_label.as_str().into());
    window.set_show_settings(panel.show_settings);
    Ok(window)
}
