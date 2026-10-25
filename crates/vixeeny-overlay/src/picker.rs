// SPDX-License-Identifier: GPL-3.0-or-later
//! Pointing at the window to capture: a light veil over the screens, the window under the pointer clear of it and
//! outlined in the accent colour. A click takes it; Escape or a right click gives up.
//!
//! Nothing is drawn while the pointer moves: the veil and the outline are solid visuals the compositor stretches over
//! their rectangles.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use vixeeny_platform::PhysicalRect;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::DirectComposition::IDCompositionVisual2;
use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    WA_INACTIVE, WM_ACTIVATE, WM_CLOSE, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_SETCURSOR,
};
use windows::core::Result;

use crate::gfx::{self, Gfx};
use crate::popup::{self, Popup};
use crate::scene::{self, Px};
use crate::theme::Rgba;
use crate::window::{Capture, Focus, Pointer};

/// How dark the veil is, and how wide the outline is at 100 %.
const VEIL: f32 = 0.35;
const OUTLINE: f32 = 3.0;

/// The window under `(x, y)`: the first of `windows` (front to back) holding it.
pub fn window_at(windows: &[PhysicalRect], (x, y): (i32, i32)) -> Option<usize> {
    windows.iter().position(|w| w.contains(x, y))
}

struct Inner {
    gfx: Rc<Gfx>,
    popup: Popup,
    /// Top, bottom, left, right, around the window pointed at.
    veil: [IDCompositionVisual2; 4],
    outline: [IDCompositionVisual2; 4],
    /// The windows, in screen pixels, front to back; where the popup starts.
    windows: Vec<PhysicalRect>,
    origin: (i32, i32),
    size: (u32, u32),
    width: f32,
    pointed: Cell<Option<usize>>,
    chosen: Cell<Option<usize>>,
    done: Cell<bool>,
}

impl Inner {
    /// Clears the window pointed at of the veil and outlines it.
    fn point(&self, at: Option<usize>) -> Result<()> {
        self.pointed.set(at);
        let hole: Option<Px> = at.map(|i| {
            let w = self.windows[i];
            ((w.x - self.origin.0) as f32, (w.y - self.origin.1) as f32, w.width as f32, w.height as f32)
        });
        let area = (0.0, 0.0, self.size.0 as f32, self.size.1 as f32);
        for (v, r) in self.veil.iter().zip(scene::veil_rects(area, hole)) {
            scene::place(v, r)?;
        }
        let border = hole.map_or([(0.0, 0.0, 0.0, 0.0); 4], |h| scene::border_rects(h, self.width));
        for (v, r) in self.outline.iter().zip(border) {
            scene::place(v, r)?;
        }
        // SAFETY: plain call on the device.
        unsafe { self.gfx.dcomp.Commit() }
    }

    fn message(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_SETCURSOR if (lparam.0 & 0xffff) == 1 => {
                popup::set_pointer(Pointer::Hand);
                return Some(LRESULT(1));
            }
            WM_MOUSEMOVE => {
                let (x, y) = popup::point(lparam);
                let at = window_at(&self.windows, (x as i32 + self.origin.0, y as i32 + self.origin.1));
                if at != self.pointed.get()
                    && let Err(e) = self.point(at)
                {
                    tracing::debug!("window picker: {e}");
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = popup::point(lparam);
                let at = window_at(&self.windows, (x as i32 + self.origin.0, y as i32 + self.origin.1));
                // Beside every window: nothing to take.
                if at.is_some() {
                    self.chosen.set(at);
                    self.done.set(true);
                }
            }
            WM_RBUTTONDOWN | WM_CLOSE => self.done.set(true),
            WM_KEYDOWN if VIRTUAL_KEY(wparam.0 as u16) == VK_ESCAPE => self.done.set(true),
            // The user went elsewhere (Alt+Tab, the Windows key).
            WM_ACTIVATE if (wparam.0 & 0xffff) as u32 == WA_INACTIVE => self.done.set(true),
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

/// Lets the user click one of `windows` (in screen pixels, front to back) over `bounds`, all the screens; `pointer` is
/// where the pointer is now, `scale` the interface scale of the screens. Returns the index of the window taken, `None`
/// when the user gave up.
pub fn pick(
    windows: Vec<PhysicalRect>,
    bounds: PhysicalRect,
    pointer: (i32, i32),
    accent: [u8; 3],
    scale: f32,
) -> Result<Option<usize>> {
    let gfx = Gfx::shared()?;
    // Never in a capture: the window taken is captured once the picker is gone anyway.
    let popup = Popup::new(&gfx, bounds, Focus::Take, Capture::Hidden)?;
    let veil_colour = gfx::solid(&gfx, Rgba([0.0, 0.0, 0.0, VEIL]))?;
    let accent_colour = gfx::solid(&gfx, Rgba::rgb8(accent))?;
    let four = |surface| -> Result<[_; 4]> {
        Ok([
            scene::solid_visual(&gfx, surface)?,
            scene::solid_visual(&gfx, surface)?,
            scene::solid_visual(&gfx, surface)?,
            scene::solid_visual(&gfx, surface)?,
        ])
    };
    let (veil, outline) = (four(&veil_colour)?, four(&accent_colour)?);
    for v in veil.iter().chain(&outline) {
        scene::add(&popup.root, v)?;
    }
    let inner = Rc::new(Inner {
        gfx: gfx.clone(),
        popup,
        veil,
        outline,
        windows,
        origin: (bounds.x, bounds.y),
        size: (bounds.width, bounds.height),
        width: (OUTLINE * scale).round().max(2.0),
        pointed: Cell::new(None),
        chosen: Cell::new(None),
        done: Cell::new(false),
    });
    let weak: Weak<Inner> = Rc::downgrade(&inner);
    inner.popup.on_message(move |msg, wparam, lparam| weak.upgrade()?.message(msg, wparam, lparam));
    inner.point(window_at(&inner.windows, pointer))?;
    inner.popup.show(&gfx)?;
    popup::pump_while(|| !inner.done.get());
    inner.popup.hide();
    Ok(inner.chosen.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_window_under_the_pointer_is_taken() {
        let windows = [PhysicalRect::new(0, 0, 100, 100), PhysicalRect::new(50, 50, 200, 200)];
        assert_eq!(window_at(&windows, (60, 60)), Some(0));
        assert_eq!(window_at(&windows, (120, 60)), Some(1));
        assert_eq!(window_at(&windows, (300, 300)), None);
        // The right and bottom edges are outside.
        assert_eq!(window_at(&windows, (100, 10)), None);
    }
}
