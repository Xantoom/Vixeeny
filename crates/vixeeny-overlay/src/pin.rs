// SPDX-License-Identifier: GPL-3.0-or-later
//! A capture pinned over the other windows: a borderless window that shows the picture until it is closed. Dragged
//! anywhere to move it; the wheel zooms it about the pointer; a double click or Escape closes it; a right click offers
//! to copy it or close it.

use std::cell::Cell;
use std::rc::Rc;

use vixeeny_editor::RgbaImage;
use vixeeny_platform::PhysicalRect;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    HTCAPTION, WM_CLOSE, WM_CONTEXTMENU, WM_KEYDOWN, WM_MOUSEWHEEL, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_SETCURSOR,
    WM_SYSKEYDOWN,
};
use windows::core::Result;

use crate::gfx::{Gfx, rect};
use crate::popup::{self, Popup};
use crate::theme::Rgba;
use crate::window::{Capture, Focus, Pointer};

/// The zooms the wheel steps through, in percent.
const ZOOMS: [u32; 12] = [10, 25, 33, 50, 67, 75, 100, 125, 150, 200, 300, 400];

/// The zoom one wheel notch from `zoom`, up or down; it stays at the ends.
pub fn next_zoom(zoom: u32, up: bool) -> u32 {
    if up {
        ZOOMS.into_iter().find(|z| *z > zoom).unwrap_or(zoom)
    } else {
        ZOOMS.into_iter().rev().find(|z| *z < zoom).unwrap_or(zoom)
    }
}

/// Where a `size` picture shown at `zoom` % goes, its point under `pointer` staying there, from `from` (the window at
/// `old` %). Never smaller than one pixel.
pub fn zoomed(from: PhysicalRect, old: u32, zoom: u32, size: (u32, u32), pointer: (i32, i32)) -> PhysicalRect {
    let side = |n: u32| ((u64::from(n) * u64::from(zoom) + 50) / 100).max(1) as u32;
    let (w, h) = (side(size.0), side(size.1));
    let ratio = f64::from(zoom) / f64::from(old.max(1));
    let keep = |p: i32, start: i32| p - (f64::from(p - start) * ratio).round() as i32;
    PhysicalRect::new(keep(pointer.0, from.x), keep(pointer.1, from.y), w, h)
}

/// The hairline around the picture, so it is not taken for the screen under it.
const BORDER: Rgba = Rgba::hex(0x80_80_80, 0xC0);

/// What the right click offers, in the user's language.
pub struct Menu {
    pub copy: String,
    pub close: String,
}

/// Shows `img` pinned with its top left corner at `at` (physical pixels) and runs until it is closed. `copy` puts the
/// picture on the clipboard. `dark`: the menu's theme.
pub fn run(img: &RgbaImage, at: (i32, i32), menu: &Menu, dark: bool, copy: impl Fn() + 'static) -> Result<()> {
    let gfx = Rc::new(Gfx::new()?);
    let size = (img.width.max(1), img.height.max(1));
    let geometry = PhysicalRect::new(at.0, at.1, size.0, size.1);
    let popup = Rc::new(Popup::new(&gfx, geometry, Focus::Take, Capture::Own)?);
    let bitmap = gfx.bitmap(img, (0, 0, size.0, size.1))?;
    let draw = {
        let (gfx, root) = (gfx.clone(), popup.root.clone());
        move |w: u32, h: u32| -> Result<()> {
            let surface = gfx.opaque_surface(w, h)?;
            gfx.draw(&surface, w, h, |c| {
                let r = rect(0.0, 0.0, w as f32, h as f32);
                c.picture(&bitmap, r)?;
                c.stroke_round(r, 0.0, 1.0, BORDER)
            })?;
            // SAFETY: plain calls on the popup's own visual and device.
            unsafe {
                root.SetContent(&surface)?;
                gfx.dcomp.Commit()
            }
        }
    };
    draw(size.0, size.1)?;
    let open = Rc::new(Cell::new(true));
    let place = Rc::new(Cell::new((geometry, 100u32)));
    let items = [menu.copy.clone(), menu.close.clone()];
    {
        let (popup_ref, open, place) = (Rc::downgrade(&popup), open.clone(), place.clone());
        popup.on_message(move |msg, wparam: WPARAM, lparam: LPARAM| {
            let close = || {
                open.set(false);
                Some(LRESULT(0))
            };
            match msg {
                // Dragged anywhere: Windows moves it as by a title bar.
                WM_NCHITTEST => Some(LRESULT(HTCAPTION as isize)),
                WM_SETCURSOR => {
                    popup::set_pointer(Pointer::Arrow);
                    Some(LRESULT(1))
                }
                WM_NCLBUTTONDBLCLK | WM_CLOSE => close(),
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    // SAFETY: plain call.
                    let ctrl = unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } < 0;
                    match wparam.0 as u16 {
                        k if k == VK_ESCAPE.0 => close(),
                        k if ctrl && k == u16::from(b'C') => {
                            copy();
                            Some(LRESULT(0))
                        }
                        _ => None,
                    }
                }
                WM_CONTEXTMENU => {
                    let popup = popup_ref.upgrade()?;
                    match popup::context_menu(popup.hwnd, &items, dark) {
                        Some(0) => copy(),
                        Some(1) => {
                            open.set(false);
                        }
                        _ => {}
                    }
                    Some(LRESULT(0))
                }
                WM_MOUSEWHEEL => {
                    let popup = popup_ref.upgrade()?;
                    let up = (wparam.0 >> 16) as u16 as i16 > 0;
                    let (from, old) = place.get();
                    // The wheel's point is in screen coordinates.
                    let pointer = (i32::from(lparam.0 as u16 as i16), i32::from((lparam.0 >> 16) as u16 as i16));
                    let zoom = next_zoom(old, up);
                    if zoom != old {
                        let to = zoomed(from, old, zoom, size, pointer);
                        if draw(to.width, to.height).is_ok() {
                            popup.place(to);
                            place.set((to, zoom));
                        }
                    }
                    Some(LRESULT(0))
                }
                _ => None,
            }
        });
    }
    popup.show(&gfx)?;
    popup::pump_while(|| open.get());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wheel_steps_through_the_zooms_and_stops_at_the_ends() {
        assert_eq!(next_zoom(100, true), 125);
        assert_eq!(next_zoom(100, false), 75);
        assert_eq!(next_zoom(400, true), 400);
        assert_eq!(next_zoom(10, false), 10);
        // A zoom between two steps goes to the next one.
        assert_eq!(next_zoom(110, true), 125);
        assert_eq!(next_zoom(110, false), 100);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let at = PhysicalRect::new(100, 100, 400, 200);
        // From 100 % to 200 % about (300, 200): the picture's point (200, 100) stays there.
        let to = zoomed(at, 100, 200, (400, 200), (300, 200));
        assert_eq!(to, PhysicalRect::new(-100, 0, 800, 400));
        // Back to 100 %: where it was.
        assert_eq!(zoomed(to, 200, 100, (400, 200), (300, 200)), at);
        // Never empty.
        assert_eq!(zoomed(at, 100, 10, (4, 4), (100, 100)).width, 1);
    }
}
