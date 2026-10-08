// SPDX-License-Identifier: GPL-3.0-or-later
//! The notification (plan 5.14), after the Windows 11 ones, drawn natively: a card in the corner
//! of the screen with the thumbnail of the capture, a title and the whole text. Clicking the card
//! opens the file, its button the folder (or the settings for an error). It slides in, never
//! takes the keyboard, and goes by itself after a while unless the pointer is on it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    KillTimer, SetTimer, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_SETCURSOR, WM_TIMER,
};
use windows::core::Result;

use crate::gfx::{Align, Canvas, Gfx, Paragraph, rect};
use crate::icons;
use crate::popup::{self, Popup};
use crate::scene::{self, Piece};
use crate::side::Box2;
use crate::theme::{Look, Rgba, Theme};
use crate::window::Pointer;

/// What the card says and offers.
#[derive(Debug, Clone, Default)]
pub struct ToastContent {
    pub heading: String,
    pub body: String,
    /// Width, height, RGBA.
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

// Logical pixels.
const WIDTH: f32 = 380.0;
/// The window is the card and room for its shadow: 8 on the sides, 4 above, 12 below.
const SIDE: f32 = 8.0;
const ABOVE: f32 = 4.0;
const BELOW: f32 = 12.0;
const PAD: f32 = 14.0;
const PAD_LEFT: f32 = 18.0;
const THUMB: f32 = 76.0;
const ICON: f32 = 24.0;
const LIFETIME_MS: u32 = 7000;
const ENTER: f64 = 0.26;

/// What is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardTarget {
    Card,
    Button,
    Close,
    Outside,
}

/// Where everything is, in physical pixels; the boxes are relative to the window.
pub struct CardLayout {
    pub u: f32,
    pub size: (u32, u32),
    pub card: Box2,
    /// The thumbnail, or the icon when there is none.
    pub picture: Box2,
    pub heading: (Paragraph, (f32, f32)),
    pub body: (Paragraph, (f32, f32)),
    pub button: Option<(Paragraph, Box2)>,
    pub close: Box2,
}

impl CardLayout {
    /// The card for `content` at `dpi`: as wide as a Windows notification, as high as its text.
    pub fn new(gfx: &Gfx, content: &ToastContent, dpi: u32) -> Result<Self> {
        let u = dpi.max(48) as f32 / 96.0;
        let card_w = (WIDTH - 2.0 * SIDE) * u;
        let picture_side = if content.thumb.is_some() { THUMB } else { ICON };
        let text_x = (PAD_LEFT + picture_side + PAD) * u;
        let text_w = card_w - text_x - PAD * u;
        let heading = gfx.paragraph(&content.heading, 14.0 * u, true, text_w)?;
        let body = gfx.paragraph(&content.body, 12.0 * u, false, text_w)?;
        let gap = 3.0 * u;
        let (x0, y0) = (SIDE * u, ABOVE * u);
        let top = y0 + PAD * u;
        let heading_at = (x0 + text_x, top);
        let body_at = (x0 + text_x, top + heading.height + gap);
        let mut text_h = heading.height + gap + body.height;
        let button = if content.action_label.is_empty() {
            None
        } else {
            let label = gfx.paragraph(&content.action_label, 12.0 * u, false, 1.0e5)?;
            let at = (
                x0 + text_x,
                top + text_h + gap + 6.0 * u,
                (label.width + 24.0 * u).ceil(),
                26.0 * u,
            );
            text_h += gap + 36.0 * u;
            Some((label, at))
        };
        let picture = if content.thumb.is_some() {
            (x0 + PAD_LEFT * u, top, THUMB * u, THUMB * u)
        } else {
            (x0 + PAD_LEFT * u, top + 2.0 * u, ICON * u, ICON * u)
        };
        let inner = text_h.max(picture.3).max(40.0 * u);
        let card_h = (inner + 2.0 * PAD * u).ceil();
        let card = (x0, y0, card_w, card_h);
        let size = (
            (WIDTH * u).ceil() as u32,
            (card_h + (ABOVE + BELOW) * u).ceil() as u32,
        );
        let close = (x0 + card_w - 28.0 * u, y0 + 6.0 * u, 22.0 * u, 22.0 * u);
        Ok(Self {
            u,
            size,
            card,
            picture,
            heading: (heading, heading_at),
            body: (body, body_at),
            button,
            close,
        })
    }

    pub fn at(&self, x: f32, y: f32, hovered: bool) -> CardTarget {
        let inside = |(bx, by, bw, bh): Box2| x >= bx && x < bx + bw && y >= by && y < by + bh;
        if !inside(self.card) {
            CardTarget::Outside
        } else if hovered && inside(self.close) {
            CardTarget::Close
        } else if self.button.as_ref().is_some_and(|(_, b)| inside(*b)) {
            CardTarget::Button
        } else {
            CardTarget::Card
        }
    }
}

/// What the card shows; drawn again only when this changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardLook {
    pub hovered: bool,
    pub hover: Option<CardTarget>,
    pub held: Option<CardTarget>,
}

/// The card. `thumb` is the thumbnail as a bitmap of the same device.
pub fn paint_card(
    c: &Canvas<'_>,
    t: &Theme,
    content: &ToastContent,
    l: &CardLayout,
    thumb: Option<&windows::Win32::Graphics::Direct2D::ID2D1Bitmap1>,
    look: &CardLook,
) -> Result<()> {
    let u = l.u;
    let (cx, cy, cw, ch) = l.card;
    let card = rect(cx, cy, cw, ch);
    let radius = 10.0 * u;
    c.shadow(card, radius, 16.0 * u, 4.0 * u, Rgba::hex(0x000000, 0x60))?;
    c.fill_round(card, radius, t.flyout)?;
    // The colour of the kind, along the left edge: accent for news, red for a failure.
    let kind = if content.error { t.danger } else { t.accent };
    c.push_round_clip(card, radius)?;
    let stripe = c.fill_rect(rect(cx, cy, 3.0 * u, ch), kind);
    c.pop_layer();
    stripe?;
    c.stroke_round(card, radius, 1.0, t.stroke_strong)?;

    let (px, py, pw, ph) = l.picture;
    match (thumb, &content.thumb) {
        (Some(bitmap), Some((w, h, _))) => {
            let frame = rect(px, py, pw, ph);
            c.fill_round(frame, 6.0 * u, t.bg_deep)?;
            // Contained: the whole picture, centred.
            let scale = (pw / *w as f32).min(ph / *h as f32);
            let (dw, dh) = (*w as f32 * scale, *h as f32 * scale);
            c.push_round_clip(frame, 6.0 * u)?;
            let drawn = c.picture(
                bitmap,
                rect(px + (pw - dw) / 2.0, py + (ph - dh) / 2.0, dw, dh),
            );
            c.pop_layer();
            drawn?;
        }
        _ => {
            let icon = if content.error {
                icons::ERROR
            } else {
                icons::CHECKMARK_CIRCLE
            };
            c.icon(icon, px, py, pw, kind)?;
        }
    }
    let (heading, (hx, hy)) = &l.heading;
    c.text_layout(&heading.layout, *hx, *hy, t.text)?;
    let (body, (bx, by)) = &l.body;
    c.text_layout(&body.layout, *bx, *by, t.text_2)?;
    if let Some((_, b)) = &l.button {
        let fill = if look.held == Some(CardTarget::Button) {
            t.control_pressed
        } else if look.hover == Some(CardTarget::Button) {
            t.control_hover
        } else {
            t.control
        };
        let r = rect(b.0, b.1, b.2, b.3);
        c.fill_round(r, 6.0 * u, fill)?;
        c.stroke_round(r, 6.0 * u, 1.0, t.control_stroke)?;
        c.text(&content.action_label, r, 12.0 * u, t.text, Align::Centre)?;
    }
    // Close, while the pointer is on the card.
    if look.hovered {
        let (x, y, w, h) = l.close;
        if look.hover == Some(CardTarget::Close) {
            c.fill_round(rect(x, y, w, h), 6.0 * u, t.subtle_hover)?;
        }
        c.icon(icons::DISMISS, x + 5.0 * u, y + 5.0 * u, 12.0 * u, t.text_2)?;
    }
    Ok(())
}

type Done = Box<dyn FnOnce(Option<ToastEvent>)>;

const LIFETIME_TIMER: usize = 1;

struct Card {
    content: ToastContent,
    layout: CardLayout,
    theme: Theme,
    popup: Popup,
    piece: Piece<CardLook>,
    thumb: Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap1>,
    look: Cell<CardLook>,
    press: Cell<Option<CardTarget>>,
    done: RefCell<Option<Done>>,
}

impl Card {
    fn refresh(&self, gfx: &Gfx) {
        let look = self.look.get();
        let size = self.layout.size;
        let drawn = self
            .piece
            .show(
                gfx,
                &look,
                || Ok(size),
                |c| {
                    paint_card(
                        c,
                        &self.theme,
                        &self.content,
                        &self.layout,
                        self.thumb.as_ref(),
                        &look,
                    )
                },
            )
            // SAFETY: plain call.
            .and_then(|()| unsafe { gfx.dcomp.Commit() });
        if let Err(e) = drawn {
            tracing::warn!("notification: {e}");
        }
    }

    fn set_timer(&self, on: bool) {
        // SAFETY: plain timer calls on a window of this thread.
        unsafe {
            if on {
                SetTimer(Some(self.popup.hwnd), LIFETIME_TIMER, LIFETIME_MS, None);
            } else {
                let _ = KillTimer(Some(self.popup.hwnd), LIFETIME_TIMER);
            }
        }
    }
}

struct Shared {
    gfx: Gfx,
    /// The card on screen, numbered.
    current: RefCell<Option<(u64, Rc<Card>)>>,
    count: Cell<u64>,
}

impl Shared {
    /// The card `id` is done: it goes, then `done` learns what the user did.
    fn finish(&self, id: u64, event: Option<ToastEvent>) {
        let card = {
            let mut current = self.current.borrow_mut();
            match current.as_ref() {
                Some((shown, _)) if *shown == id => current.take().map(|(_, c)| c),
                _ => None,
            }
        };
        let Some(card) = card else {
            return;
        };
        card.popup.hide();
        let done = card.done.borrow_mut().take();
        if let Some(done) = done {
            done(event);
        }
    }

    fn message(&self, id: u64, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        if let Some(r) = popup::passive(msg) {
            return Some(r);
        }
        let card = self
            .current
            .borrow()
            .as_ref()
            .filter(|(shown, _)| *shown == id)
            .map(|(_, c)| c.clone())?;
        let gfx = &self.gfx;
        let (x, y) = popup::point(lparam);
        let mut look = card.look.get();
        match msg {
            WM_TIMER if wparam.0 == LIFETIME_TIMER => {
                card.set_timer(false);
                self.finish(id, None);
                return Some(LRESULT(0));
            }
            WM_SETCURSOR if (lparam.0 & 0xffff) == 1 => {
                let hand = matches!(look.hover, Some(CardTarget::Button | CardTarget::Close));
                popup::set_pointer(if hand { Pointer::Hand } else { Pointer::Arrow });
                return Some(LRESULT(1));
            }
            WM_MOUSEMOVE => {
                card.popup.track_leave();
                if !look.hovered {
                    // The card stays while the pointer is on it.
                    card.set_timer(false);
                }
                look.hovered = true;
                look.hover = Some(card.layout.at(x, y, true));
            }
            popup::WM_MOUSELEAVE => {
                look.hovered = false;
                look.hover = None;
                card.set_timer(true);
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                // SAFETY: plain call on a window of this thread.
                unsafe { SetCapture(card.popup.hwnd) };
                card.press.set(Some(card.layout.at(x, y, look.hovered)));
            }
            WM_LBUTTONUP => {
                // SAFETY: plain call.
                let _ = unsafe { ReleaseCapture() };
                let at = card.layout.at(x, y, look.hovered);
                if card.press.take() == Some(at) {
                    let event = match at {
                        CardTarget::Card => Some(Some(ToastEvent::Activated)),
                        CardTarget::Button => Some(Some(ToastEvent::Action)),
                        CardTarget::Close => Some(None),
                        CardTarget::Outside => None,
                    };
                    if let Some(event) = event {
                        self.finish(id, event);
                        return Some(LRESULT(0));
                    }
                }
            }
            _ => return None,
        }
        look.held = card.press.get().filter(|p| look.hover == Some(*p));
        card.look.set(look);
        card.refresh(gfx);
        Some(LRESULT(0))
    }
}

/// The cards of a process, one at a time: a new card takes the place of the one on screen.
pub struct Toasts {
    shared: Rc<Shared>,
}

impl Toasts {
    pub fn new() -> Result<Self> {
        Ok(Self {
            shared: Rc::new(Shared {
                gfx: Gfx::new()?,
                current: RefCell::default(),
                count: Cell::new(0),
            }),
        })
    }

    /// No card is on screen.
    pub fn is_empty(&self) -> bool {
        self.shared.current.borrow().is_none()
    }

    /// Shows a card in the bottom-right corner of `monitor` (physical pixels, at `dpi`), in the
    /// place of the one on screen (whose `done` is not called). `done` learns what the user did
    /// (`None`: the card went by itself or was closed) once the card is gone.
    pub fn show(
        &self,
        content: &ToastContent,
        monitor: (i32, i32, u32, u32),
        dpi: u32,
        animate: bool,
        done: impl FnOnce(Option<ToastEvent>) + 'static,
    ) -> Result<()> {
        let shared = &self.shared;
        let gfx = &shared.gfx;
        let layout = CardLayout::new(gfx, content, dpi)?;
        let u = layout.u;
        let (x, y) = corner(monitor, layout.size, (16.0 * u) as u32, (48.0 * u) as u32);
        let popup = Popup::new(gfx, (x, y, layout.size.0, layout.size.1), false, false)?;
        let piece = Piece::new(gfx, true)?;
        scene::add(&popup.root, &piece.visual)?;
        let thumb = match &content.thumb {
            Some((w, h, rgba)) => match gfx.rgba_bitmap(*w, *h, rgba) {
                Ok(b) => Some(b),
                Err(e) => {
                    tracing::warn!("notification thumbnail: {e}");
                    None
                }
            },
            None => None,
        };
        let card = Rc::new(Card {
            content: content.clone(),
            layout,
            theme: Theme::new(Look {
                dark: content.dark,
                animations: animate,
                ..Look::default()
            }),
            popup,
            piece,
            thumb,
            look: Cell::new(CardLook {
                hovered: false,
                hover: None,
                held: None,
            }),
            press: Cell::new(None),
            done: RefCell::new(Some(Box::new(done))),
        });
        let id = shared.count.get() + 1;
        shared.count.set(id);
        let weak: Weak<Shared> = Rc::downgrade(shared);
        card.popup.on_message(move |msg, wparam, lparam| {
            weak.upgrade()?.message(id, msg, wparam, lparam)
        });
        // Drawn where it slides in from, transparent: it appears with the slide.
        let slide = 40.0 * u;
        scene::set_offset(&card.piece.visual, if animate { slide } else { 0.0 }, 0.0)?;
        if let Some(effect) = &card.piece.effect {
            // SAFETY: plain property call.
            unsafe { effect.SetOpacity2(if animate { 0.0 } else { 1.0 })? };
        }
        card.refresh(gfx);
        card.popup.show(gfx)?;
        if animate && let Some(effect) = &card.piece.effect {
            // SAFETY: plain property calls with animations of the same device.
            unsafe {
                card.piece
                    .visual
                    .SetOffsetX(&scene::ease_out(gfx, slide, 0.0, ENTER)?)?;
                effect.SetOpacity(&scene::ease_out(gfx, 0.0, 1.0, ENTER)?)?;
                gfx.dcomp.Commit()?;
            }
        }
        card.set_timer(true);
        // The previous card goes: the new one has its place.
        let old = shared.current.borrow_mut().replace((id, card));
        drop(old);
        Ok(())
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
