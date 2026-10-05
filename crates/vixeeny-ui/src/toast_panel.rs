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

/// The card's size in logical pixels: the width is fixed, the height follows the text so that
/// none of it is cut (the estimate counts the characters a line holds).
pub fn size_for(content: &ToastContent) -> (f64, f64) {
    const WIDTH: f64 = 380.0;
    // The card is 16 px narrower than the window (shadow), minus padding and the thumbnail/icon.
    let text_width =
        WIDTH - 16.0 - 18.0 - 14.0 - 14.0 - if content.thumb.is_some() { 76.0 } else { 38.0 };
    let lines = |text: &str, per_char: f64| -> f64 {
        text.split('\n')
            .map(|line| {
                ((line.chars().count() as f64 * per_char) / text_width)
                    .ceil()
                    .max(1.0)
            })
            .sum()
    };
    let mut text = lines(&content.heading, 8.0) * 20.0 + lines(&content.body, 6.8) * 16.0 + 3.0;
    if !content.action_label.is_empty() {
        text += 36.0;
    }
    let thumb = if content.thumb.is_some() { 76.0 } else { 0.0 };
    // Padding 14 + 14, then the shadow room (16 wide, 16 high with the offset).
    (WIDTH, text.max(thumb).max(40.0) + 28.0 + 16.0)
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
        crate::theme::apply(&window, crate::theme::Look::new(content.dark, None, true));
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
        let e = event.clone();
        self.show(move |what| e.set(what))?;
        slint::run_event_loop()?;
        Ok(event.get())
    }

    /// Shows the card and returns at once; `done` gets what the user did (`None`: the card went
    /// by itself or was closed) once the card is gone. For a process that shows several cards.
    pub fn show(
        &self,
        done: impl FnOnce(Option<ToastEvent>) + 'static,
    ) -> Result<(), slint::PlatformError> {
        type Done = Box<dyn FnOnce(Option<ToastEvent>)>;
        let done: std::rc::Rc<std::cell::RefCell<Option<Done>>> =
            std::rc::Rc::new(std::cell::RefCell::new(Some(Box::new(done))));
        let finish = move |w: &slint::Weak<ToastWindow>, what: Option<ToastEvent>| {
            if let Some(w) = w.upgrade() {
                let _ = w.hide();
            }
            if let Some(done) = done.borrow_mut().take() {
                done(what);
            }
        };
        let finish = std::rc::Rc::new(finish);
        let (f, w) = (finish.clone(), self.window.as_weak());
        self.window
            .on_activated(move || f(&w, Some(ToastEvent::Activated)));
        let (f, w) = (finish.clone(), self.window.as_weak());
        self.window
            .on_action(move || f(&w, Some(ToastEvent::Action)));
        let w = self.window.as_weak();
        self.window.on_dismissed(move || finish(&w, None));
        self.window.show()
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
