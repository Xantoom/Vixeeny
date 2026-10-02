// SPDX-License-Identifier: GPL-3.0-or-later
//! The notification card (plan 5.14).

use slint::{ComponentHandle, SharedString};

use crate::ToastWindow;

/// What the card says and offers.
#[derive(Debug, Clone, Default)]
pub struct ToastContent {
    pub heading: String,
    pub body: String,
    /// RGBA, width, height.
    pub thumb: Option<(u32, u32, Vec<u8>)>,
    pub error: bool,
    pub dark: bool,
    /// The button; empty for none.
    pub action_label: String,
}

/// What the user did with the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastEvent {
    /// The card itself was clicked.
    Activated,
    /// The button was clicked.
    Action,
}

/// Where the card goes: the bottom-right corner of a monitor, `margin` physical pixels from the
/// edges (the taskbar is not known here, so the margin includes room for it).
pub fn corner(
    monitor: (i32, i32, u32, u32),
    size: (u32, u32),
    margin: u32,
    taskbar: u32,
) -> (i32, i32) {
    let (x, y, w, h) = monitor;
    (
        x + w as i32 - size.0 as i32 - margin as i32,
        y + h as i32 - size.1 as i32 - (margin + taskbar) as i32,
    )
}

pub struct ToastPanel {
    window: ToastWindow,
}

impl ToastPanel {
    pub fn new(content: &ToastContent) -> Result<Self, slint::PlatformError> {
        let window = ToastWindow::new()?;
        window.set_heading(SharedString::from(content.heading.as_str()));
        window.set_body(SharedString::from(content.body.as_str()));
        window.set_error(content.error);
        window.set_dark(content.dark);
        window.set_action_label(SharedString::from(content.action_label.as_str()));
        if let Some((w, h, rgba)) = &content.thumb {
            let buffer =
                slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(rgba, *w, *h);
            window.set_thumb(slint::Image::from_rgba8(buffer));
            window.set_has_thumb(true);
        }
        Ok(Self { window })
    }

    pub fn window(&self) -> &ToastWindow {
        &self.window
    }

    /// Physical pixels.
    pub fn set_geometry(&self, x: i32, y: i32, width: u32, height: u32) {
        let w = self.window.window();
        w.set_size(slint::PhysicalSize::new(width, height));
        w.set_position(slint::PhysicalPosition::new(x, y));
    }

    /// Shows the card until it is clicked, its button is, or it dismisses itself; gives back what
    /// the user did, if anything.
    pub fn run(&self) -> Result<Option<ToastEvent>, slint::PlatformError> {
        let event = std::rc::Rc::new(std::cell::Cell::new(None));
        let (e, w) = (event.clone(), self.window.as_weak());
        self.window.on_activated(move || {
            e.set(Some(ToastEvent::Activated));
            if let Some(w) = w.upgrade() {
                let _ = w.hide();
            }
        });
        let (e, w) = (event.clone(), self.window.as_weak());
        self.window.on_action(move || {
            e.set(Some(ToastEvent::Action));
            if let Some(w) = w.upgrade() {
                let _ = w.hide();
            }
        });
        let w = self.window.as_weak();
        self.window.on_dismissed(move || {
            if let Some(w) = w.upgrade() {
                let _ = w.hide();
            }
        });
        self.window.run()?;
        Ok(event.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_sits_in_the_bottom_right_corner() {
        assert_eq!(corner((0, 0, 1920, 1080), (360, 96), 16, 48), (1544, 920));
        // A second monitor to the left keeps its own corner.
        assert_eq!(
            corner((-1920, 0, 1920, 1080), (360, 96), 16, 48),
            (-376, 920)
        );
    }
}
