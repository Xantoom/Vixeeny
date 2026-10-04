// SPDX-License-Identifier: GPL-3.0-or-later
//! The control window of a scrolling capture (plan 5.7).

use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, Weak};

use crate::ScrollWindow;

/// Translated texts of the window.
#[derive(Debug, Clone, Default)]
pub struct ScrollTexts {
    pub title: String,
    pub intro: String,
    pub start: String,
    pub finish: String,
    pub cancel: String,
}

/// What the user decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollEvent {
    Start,
    Finish,
    Cancel,
}

/// Builds the window next to the zone and keeps it responsive while the capture thread feeds it.
pub struct ScrollPanel {
    window: ScrollWindow,
}

/// A handle usable from the capture thread.
#[derive(Clone)]
pub struct ScrollHandle {
    window: Weak<ScrollWindow>,
}

impl ScrollHandle {
    /// `rgba` is `width * height * 4` bytes. Silently ignored once the window is gone.
    pub fn show_preview(&self, width: u32, height: u32, rgba: Vec<u8>, status: String) {
        let _ = self.window.upgrade_in_event_loop(move |w| {
            if let Some(buffer) = pixel_buffer(width, height, &rgba) {
                w.set_preview(Image::from_rgba8(buffer));
            }
            w.set_status(status.into());
        });
    }

    pub fn show_status(&self, status: String) {
        let _ = self
            .window
            .upgrade_in_event_loop(move |w| w.set_status(status.into()));
    }
}

fn pixel_buffer(width: u32, height: u32, rgba: &[u8]) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    if rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(rgba);
    Some(buffer)
}

impl ScrollPanel {
    pub fn new(texts: &ScrollTexts) -> Result<Self, slint::PlatformError> {
        let window = ScrollWindow::new()?;
        crate::theme::apply(&window, crate::theme::default_look());
        window.set_status(texts.intro.as_str().into());
        window.set_start_label(texts.start.as_str().into());
        window.set_finish_label(texts.finish.as_str().into());
        window.set_cancel_label(texts.cancel.as_str().into());
        Ok(Self { window })
    }

    pub fn handle(&self) -> ScrollHandle {
        ScrollHandle {
            window: self.window.as_weak(),
        }
    }

    pub fn set_position(&self, x: i32, y: i32, width: u32, height: u32) {
        let w = self.window.window();
        w.set_size(slint::PhysicalSize::new(width, height));
        w.set_position(slint::PhysicalPosition::new(x, y));
    }

    /// Runs until the window closes. `on_event` is called for each button; the window closes
    /// itself on `Finish` and `Cancel`.
    pub fn run(self, on_event: impl Fn(ScrollEvent) + 'static) -> Result<(), slint::PlatformError> {
        let on_event = std::rc::Rc::new(on_event);
        let (weak, f) = (self.window.as_weak(), on_event.clone());
        self.window.on_start(move || {
            if let Some(w) = weak.upgrade() {
                w.set_running(true);
            }
            f(ScrollEvent::Start);
        });
        let (weak, f) = (self.window.as_weak(), on_event.clone());
        self.window.on_finish(move || {
            f(ScrollEvent::Finish);
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
            }
        });
        let (weak, f) = (self.window.as_weak(), on_event.clone());
        self.window.on_cancel(move || {
            f(ScrollEvent::Cancel);
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
            }
        });
        let f = on_event;
        self.window.window().on_close_requested(move || {
            f(ScrollEvent::Cancel);
            slint::CloseRequestResponse::HideWindow
        });
        self.window.run()
    }
}
