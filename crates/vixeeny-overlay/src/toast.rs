// SPDX-License-Identifier: GPL-3.0-or-later
//! The notification, after the Windows 11 ones, drawn natively: a card in the corner of the screen under a line with
//! the app's icon and name, with the thumbnail of the capture, a title and the whole text. Clicking the card opens the
//! file, its button the folder (or the settings for an error): it sinks a little under the press and fades as it goes.
//! It slides in, never takes the keyboard, and goes by itself after a while unless the pointer is on it; a bar in the
//! Windows accent colour, under its first line, empties until then. A screenshot shows as its picture under its title:
//! a click opens it, a right-click offers a menu.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use vixeeny_editor::RgbaImage;
use vixeeny_platform::PhysicalRect;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::DirectComposition::IDCompositionScaleTransform;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    KillTimer, SetTimer, WM_CAPTURECHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONUP,
    WM_SETCURSOR, WM_TIMER,
};
use windows::core::Result;

use crate::gfx::{Align, Canvas, Gfx, Paragraph, rect};
use crate::icons;
use crate::popup::{self, Popup};
use crate::scene::{self, Piece};
use crate::side::Box2;
use crate::theme::{Look, Rgba, Theme};
use crate::window::{Capture, Focus, Pointer};

/// What the card says and offers.
#[derive(Debug, Clone, Default)]
pub struct ToastContent {
    pub heading: String,
    pub body: String,
    /// Width, height, RGBA.
    pub thumb: Option<RgbaImage>,
    pub error: bool,
    /// Something to know, not a failure: amber.
    pub warning: bool,
    pub dark: bool,
    /// The button; empty for none.
    pub action_label: String,
    /// Only the picture (a screenshot): no text, and a right-click menu.
    pub picture_only: bool,
    /// The right-click menu: open, edit, show in the folder, close.
    pub menu: Vec<String>,
    /// In the top-right corner (over a game, where its own notifications go), not the bottom.
    pub top: bool,
    /// The app's icon, before its name at the top of the card.
    pub app_icon: Option<RgbaImage>,
    /// A click on the card does something (opens the file, the settings): it lights under the pointer.
    pub clickable: bool,
}

/// What the user did with the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastEvent {
    /// The card itself was clicked (or "Open" in its menu).
    Activated,
    /// The button was clicked (or "Show in the folder" in its menu).
    Action,
    /// "Edit" in its menu.
    Edit,
}

/// Where the card goes: the bottom-right (or `top`-right) corner of the work area of a monitor (what the taskbar leaves
/// free), `margin` physical pixels from its edges.
pub fn corner(work: PhysicalRect, size: (u32, u32), margin: u32, top: bool) -> (i32, i32) {
    (
        work.x + work.width as i32 - size.0 as i32 - margin as i32,
        if top { work.y + margin as i32 } else { work.y + work.height as i32 - size.1 as i32 - margin as i32 },
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
/// The line at the top: the app's icon and name (a screenshot's title), as far from the top edge as from the bar.
const HEADER_TOP: f32 = 10.5;
const HEADER: f32 = 16.0;
/// The bar under the line at the top, across the card, that empties until the card goes; the room above it.
const BAR: f32 = 3.0;
const BAR_ABOVE: f32 = 10.5;
/// A press sinks the card to this scale.
const PRESSED: f32 = 0.97;
const PRESS: f64 = 0.08;
/// A card clicked shrinks a little more as it fades out.
const CLICKED: f32 = 0.92;
/// How long every card stays; past it, a card under the pointer waits for the pointer to leave.
const LIFETIME_MS: u32 = 2000;
/// The largest picture-only card, logical pixels.
const PICTURE_MAX: (f32, f32) = (320.0, 200.0);
/// Lines of the body at most: a longer message is cut (the logs hold all of it).
const BODY_LINES: u32 = 4;
const ENTER: f64 = 0.26;
/// The card going by itself or closed: it slides back out the way it came in, and fades.
const LEAVE: f64 = 0.22;

/// What is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardTarget {
    Card,
    Button,
    Close,
    Outside,
}

/// A paragraph and where it starts.
pub type Placed = (Paragraph, (f32, f32));

/// Where everything is, in physical pixels; the boxes are relative to the window.
pub struct CardLayout {
    pub u: f32,
    pub size: (u32, u32),
    pub card: Box2,
    /// The thumbnail, or the icon when there is none.
    pub picture: Box2,
    pub heading: Placed,
    pub body: Placed,
    pub button: Option<(Paragraph, Box2)>,
    pub close: Box2,
    /// The app's icon and the name beside it.
    pub app_icon: Box2,
    pub header: Placed,
    /// The bar that empties until the card goes.
    pub countdown: Box2,
}

impl CardLayout {
    /// The card for `content` at `dpi`: as wide as a Windows notification, as high as its text.
    pub fn new(gfx: &Gfx, content: &ToastContent, dpi: u32) -> Result<Self> {
        let u = dpi.max(48) as f32 / 96.0;
        if content.picture_only
            && let Some(thumb) = &content.thumb
        {
            return Self::picture(gfx, content, (thumb.width, thumb.height), u);
        }
        let card_w = (WIDTH - 2.0 * SIDE) * u;
        let (x0, y0) = (SIDE * u, ABOVE * u);
        let (app_icon, header) = header(gfx, "Vixeeny", (x0, y0), card_w, u)?;
        let picture_side = if content.thumb.is_some() { THUMB } else { ICON };
        let text_x = (PAD_LEFT + picture_side + PAD) * u;
        let text_w = card_w - text_x - PAD * u;
        let heading = gfx.paragraph(&content.heading, 14.0 * u, true, text_w)?;
        let body = gfx.paragraph_lines(&content.body, 12.0 * u, false, text_w, BODY_LINES)?;
        let gap = 3.0 * u;
        let countdown = countdown_box((x0, y0), card_w, u);
        let top = countdown.1 + countdown.3 + PAD * u;
        let heading_at = (x0 + text_x, top);
        let body_at = (x0 + text_x, top + heading.height + gap);
        let mut text_h = heading.height + gap + body.height;
        let button = if content.action_label.is_empty() {
            None
        } else {
            let label = gfx.paragraph(&content.action_label, 12.0 * u, false, 1.0e5)?;
            let at = (x0 + text_x, top + text_h + gap + 6.0 * u, (label.width + 24.0 * u).ceil(), 26.0 * u);
            text_h += gap + 36.0 * u;
            Some((label, at))
        };
        let picture = if content.thumb.is_some() {
            (x0 + PAD_LEFT * u, top, THUMB * u, THUMB * u)
        } else {
            (x0 + PAD_LEFT * u, top + 2.0 * u, ICON * u, ICON * u)
        };
        let inner = text_h.max(picture.3);
        let card_h = (top + inner + PAD * u - y0).ceil();
        let card = (x0, y0, card_w, card_h);
        let size = ((WIDTH * u).ceil() as u32, (card_h + (ABOVE + BELOW) * u).ceil() as u32);
        Ok(Self {
            u,
            size,
            card,
            picture,
            heading: (heading, heading_at),
            body: (body, body_at),
            button,
            close: close_box((x0, y0), card_w, u),
            app_icon,
            header,
            countdown,
        })
    }

    /// The card of a picture under its title: the picture's shape, as large as fits [`PICTURE_MAX`] (and no narrower
    /// than its title needs).
    fn picture(gfx: &Gfx, content: &ToastContent, (w, h): (u32, u32), u: f32) -> Result<Self> {
        let scale = (PICTURE_MAX.0 / w.max(1) as f32).min(PICTURE_MAX.1 / h.max(1) as f32);
        let card_w = (w as f32 * scale).max(200.0).round() * u;
        let picture_h = (h as f32 * scale).max(48.0).round() * u;
        let (x0, y0) = (SIDE * u, ABOVE * u);
        let title = if content.heading.is_empty() { "Vixeeny" } else { &content.heading };
        let (app_icon, header) = header(gfx, title, (x0, y0), card_w, u)?;
        let countdown = countdown_box((x0, y0), card_w, u);
        let picture = (x0, countdown.1 + countdown.3, card_w, picture_h);
        let card_h = picture.1 + picture_h - y0;
        let card = (x0, y0, card_w, card_h);
        let empty = || gfx.paragraph("", 12.0 * u, false, 1.0);
        Ok(Self {
            u,
            size: ((card_w + 2.0 * SIDE * u).ceil() as u32, (card_h + (ABOVE + BELOW) * u).ceil() as u32),
            card,
            picture,
            heading: (empty()?, (x0, y0)),
            body: (empty()?, (x0, y0)),
            button: None,
            close: close_box((x0, y0), card_w, u),
            app_icon,
            header,
            countdown,
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

/// The app's icon and the title beside it, on the line at the top of a card at `(x0, y0)`, `card_w` wide.
fn header(gfx: &Gfx, title: &str, (x0, y0): (f32, f32), card_w: f32, u: f32) -> Result<(Box2, Placed)> {
    let icon = (x0 + PAD_LEFT * u, y0 + HEADER_TOP * u, HEADER * u, HEADER * u);
    let text_x = icon.0 + icon.2 + 8.0 * u;
    // Room for the close button on the right.
    let label = gfx.paragraph_lines(title, 12.0 * u, false, x0 + card_w - (PAD_LEFT + 22.0) * u - text_x, 1)?;
    let text_y = icon.1 + (icon.3 - label.height) / 2.0;
    Ok((icon, (label, (text_x, text_y))))
}

/// The countdown bar: the whole width of the card, under the line at the top.
fn countdown_box((x0, y0): (f32, f32), card_w: f32, u: f32) -> Box2 {
    (x0, y0 + (HEADER_TOP + HEADER + BAR_ABOVE) * u, card_w, BAR * u)
}

/// The close button, at the end of the line at the top: its cross as far from the right edge as the icon is from the
/// left one.
fn close_box((x0, y0): (f32, f32), card_w: f32, u: f32) -> Box2 {
    (x0 + card_w - (PAD_LEFT + 17.0) * u, y0 + (HEADER_TOP + HEADER / 2.0 - 11.0) * u, 22.0 * u, 22.0 * u)
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
    app_icon: Option<&windows::Win32::Graphics::Direct2D::ID2D1Bitmap1>,
    look: &CardLook,
) -> Result<()> {
    let u = l.u;
    let (cx, cy, cw, ch) = l.card;
    let card = rect(cx, cy, cw, ch);
    let radius = 10.0 * u;
    c.shadow(card, radius, 16.0 * u, 4.0 * u, Rgba::hex(0x000000, 0x60))?;
    c.fill_round(card, radius, t.flyout)?;
    // Lit under the pointer when a click on it does something.
    let lit = look.hover == Some(CardTarget::Card) && (content.clickable || content.picture_only);
    if lit {
        c.fill_round(card, radius, t.subtle_hover)?;
    }
    paint_header(c, t, l, app_icon, look)?;
    // The bar's track parts the title from the rest; the bar itself is a visual of its own.
    let (bx, by, bw, bh) = l.countdown;
    c.fill_rect(rect(bx, by, bw, bh), t.divider)?;
    if content.picture_only
        && let Some(bitmap) = thumb
    {
        let (px, py, pw, ph) = l.picture;
        c.push_round_clip(card, radius)?;
        let drawn = c.picture(bitmap, rect(px, py, pw, ph));
        c.pop_layer();
        drawn?;
        return c.stroke_round(card, radius, 1.0, t.stroke_strong);
    }
    let kind = kind_colour(t, content);
    c.stroke_round(card, radius, 1.0, t.stroke_strong)?;

    let (px, py, pw, ph) = l.picture;
    match (thumb, &content.thumb) {
        (Some(bitmap), Some(picture)) => {
            let (w, h) = (&picture.width, &picture.height);
            let frame = rect(px, py, pw, ph);
            c.fill_round(frame, 6.0 * u, t.bg_deep)?;
            // Contained: the whole picture, centred.
            let scale = (pw / *w as f32).min(ph / *h as f32);
            let (dw, dh) = (*w as f32 * scale, *h as f32 * scale);
            c.push_round_clip(frame, 6.0 * u)?;
            let drawn = c.picture(bitmap, rect(px + (pw - dw) / 2.0, py + (ph - dh) / 2.0, dw, dh));
            c.pop_layer();
            drawn?;
        }
        _ => {
            let icon = if content.error {
                icons::ERROR
            } else if content.warning {
                icons::WARNING
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
    Ok(())
}

/// The colour of the kind of card: accent for news, amber for a warning, red for a failure.
fn kind_colour(t: &Theme, content: &ToastContent) -> Rgba {
    if content.error {
        t.danger
    } else if content.warning {
        t.warning
    } else {
        t.accent
    }
}

/// The line at the top: the app's icon, the name (or title), and the close button while the pointer is on the card.
fn paint_header(
    c: &Canvas<'_>,
    t: &Theme,
    l: &CardLayout,
    app_icon: Option<&windows::Win32::Graphics::Direct2D::ID2D1Bitmap1>,
    look: &CardLook,
) -> Result<()> {
    let u = l.u;
    if let Some(bitmap) = app_icon {
        let (x, y, w, h) = l.app_icon;
        c.picture(bitmap, rect(x, y, w, h))?;
    }
    let (label, (x, y)) = &l.header;
    c.text_layout(&label.layout, *x, *y, t.text_2)?;
    if look.hovered {
        let (x, y, w, h) = l.close;
        if look.hover == Some(CardTarget::Close) {
            c.fill_round(rect(x, y, w, h), 6.0 * u, t.subtle_hover)?;
        }
        c.icon(icons::DISMISS, x + 5.0 * u, y + 5.0 * u, 12.0 * u, t.text_2)?;
    }
    Ok(())
}

/// The full countdown bar, `w` × `h`, in the Windows accent colour: its own visual, which the compositor shrinks to
/// nothing over the card's life.
pub fn paint_countdown(c: &Canvas<'_>, t: &Theme, w: f32, h: f32) -> Result<()> {
    c.fill_rect(rect(0.0, 0.0, w, h), t.accent)
}

type Done = Box<dyn FnOnce(Option<ToastEvent>)>;

const LIFETIME_TIMER: usize = 1;
/// The end of the card's way out.
const LEAVE_TIMER: usize = 2;

struct Card {
    content: ToastContent,
    layout: CardLayout,
    theme: Theme,
    popup: Popup,
    piece: Piece<CardLook>,
    thumb: Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap1>,
    app_icon: Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap1>,
    /// The countdown bar, shrunk from its left end by the compositor (held for its surface).
    _bar: Piece<bool>,
    bar_scale: IDCompositionScaleTransform,
    /// The card sinking under a press, around its centre.
    press_scale: IDCompositionScaleTransform,
    look: Cell<CardLook>,
    press: Cell<Option<CardTarget>>,
    done: RefCell<Option<Done>>,
    animate: bool,
    /// On its way out: what `done` learns once it is gone.
    leaving: Cell<Option<Option<ToastEvent>>>,
    /// Its time is up while the pointer is on it: it goes when the pointer leaves.
    expired: Cell<bool>,
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
                        self.app_icon.as_ref(),
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

    /// Starts the card's life again (`on`), its bar emptying until it ends, or holds it (the bar full).
    fn set_timer(&self, gfx: &Gfx, on: bool) {
        let ms = LIFETIME_MS;
        // SAFETY: plain timer and property calls, with an animation of the same device, on a window of this thread.
        let bar = unsafe {
            if on {
                SetTimer(Some(self.popup.hwnd), LIFETIME_TIMER, ms, None);
            } else {
                let _ = KillTimer(Some(self.popup.hwnd), LIFETIME_TIMER);
            }
            if on && self.animate {
                scene::linear(gfx, 1.0, 0.0, f64::from(ms) / 1000.0).and_then(|a| self.bar_scale.SetScaleX(&a))
            } else {
                self.bar_scale.SetScaleX2(1.0)
            }
            .and_then(|()| gfx.dcomp.Commit())
        };
        if let Err(e) = bar {
            tracing::warn!("notification: {e}");
        }
    }

    /// Sinks the card under a press (`true`), or lifts it back.
    fn press(&self, gfx: &Gfx, down: bool) {
        if !self.animate {
            return;
        }
        let (from, to) = if down { (1.0, PRESSED) } else { (PRESSED, 1.0) };
        // SAFETY: plain property calls with animations of the same device.
        let pressed = unsafe {
            scene::ease_out(gfx, from, to, PRESS)
                .and_then(|a| {
                    self.press_scale.SetScaleX(&a)?;
                    self.press_scale.SetScaleY(&a)
                })
                .and_then(|()| gfx.dcomp.Commit())
        };
        if let Err(e) = pressed {
            tracing::warn!("notification: {e}");
        }
    }

    /// Shrinks the card clicked a little more while it fades; `false` when it cannot (no animations).
    fn shrink_out(&self, gfx: &Gfx) -> Result<bool> {
        let Some(effect) = self.piece.effect.as_ref().filter(|_| self.animate) else {
            return Ok(false);
        };
        // SAFETY: plain property and timer calls, with animations of the same device, on a window of this thread.
        unsafe {
            let shrink = scene::ease_out(gfx, PRESSED, CLICKED, LEAVE)?;
            self.press_scale.SetScaleX(&shrink)?;
            self.press_scale.SetScaleY(&shrink)?;
            effect.SetOpacity(&scene::ease_out(gfx, 1.0, 0.0, LEAVE)?)?;
            gfx.dcomp.Commit()?;
            SetTimer(Some(self.popup.hwnd), LEAVE_TIMER, (LEAVE * 1000.0) as u32, None);
        }
        Ok(true)
    }

    /// Slides the card out to the side it came from while it fades; `false` when it cannot (no animations): it goes at
    /// once then.
    fn slide_out(&self, gfx: &Gfx) -> Result<bool> {
        let Some(effect) = self.piece.effect.as_ref().filter(|_| self.animate) else {
            return Ok(false);
        };
        let width = self.layout.size.0 as f32;
        // SAFETY: plain property and timer calls, with animations of the same device, on a window of this thread.
        unsafe {
            self.piece.visual.SetOffsetX(&scene::ease_in(gfx, 0.0, width, LEAVE)?)?;
            effect.SetOpacity(&scene::ease_in(gfx, 1.0, 0.0, LEAVE)?)?;
            gfx.dcomp.Commit()?;
            SetTimer(Some(self.popup.hwnd), LEAVE_TIMER, (LEAVE * 1000.0) as u32, None);
        }
        Ok(true)
    }
}

struct Shared {
    gfx: std::rc::Rc<Gfx>,
    /// The card on screen, numbered.
    current: RefCell<Option<(u64, Rc<Card>)>>,
    count: Cell<u64>,
    /// A card went: what it held is given back once its message is done.
    trim_due: Cell<bool>,
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
        self.trim_due.set(true);
    }

    fn message(&self, id: u64, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let result = self.handle(id, msg, wparam, lparam);
        // Once the card is gone (dropped with the message), unless `done` showed another: the process waits for the
        // next one with as little as it can hold.
        if self.trim_due.take() && self.current.borrow().is_none() {
            self.gfx.trim();
        }
        result
    }

    /// The card `id` goes by itself or is closed: out of the screen first, then [`Shared::finish`].
    fn leave(&self, id: u64, card: &Card, event: Option<ToastEvent>) {
        card.set_timer(&self.gfx, false);
        card.leaving.set(Some(event));
        match card.slide_out(&self.gfx) {
            Ok(true) => {}
            Ok(false) => self.finish(id, event),
            Err(e) => {
                tracing::warn!("notification: {e}");
                self.finish(id, event);
            }
        }
    }

    /// The user chose `event` on the card `id`: it is done at once, while the card shrinks and fades away.
    fn activate(&self, id: u64, card: &Card, event: Option<ToastEvent>) {
        card.set_timer(&self.gfx, false);
        card.leaving.set(Some(None));
        let done = card.done.borrow_mut().take();
        if let Some(done) = done {
            done(event);
        }
        match card.shrink_out(&self.gfx) {
            Ok(true) => {}
            Ok(false) => self.finish(id, None),
            Err(e) => {
                tracing::warn!("notification: {e}");
                self.finish(id, None);
            }
        }
    }

    fn handle(&self, id: u64, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        if let Some(r) = popup::passive(msg) {
            return Some(r);
        }
        let card = self.current.borrow().as_ref().filter(|(shown, _)| *shown == id).map(|(_, c)| c.clone())?;
        // On its way out: nothing more to do with it but let it go.
        if let Some(event) = card.leaving.get() {
            if msg == WM_TIMER && wparam.0 == LEAVE_TIMER {
                // SAFETY: plain timer call on a window of this thread.
                let _ = unsafe { KillTimer(Some(card.popup.hwnd), LEAVE_TIMER) };
                self.finish(id, event);
            }
            return (msg == WM_TIMER).then_some(LRESULT(0));
        }
        let gfx = &self.gfx;
        let (x, y) = popup::point(lparam);
        let mut look = card.look.get();
        match msg {
            WM_TIMER if wparam.0 == LIFETIME_TIMER => {
                // SAFETY: plain timer call on a window of this thread.
                let _ = unsafe { KillTimer(Some(card.popup.hwnd), LIFETIME_TIMER) };
                // Under the pointer (or held), it stays until the pointer leaves.
                if look.hovered || card.press.get().is_some() {
                    card.expired.set(true);
                } else {
                    self.leave(id, &card, None);
                }
                return Some(LRESULT(0));
            }
            WM_SETCURSOR if (lparam.0 & 0xffff) == 1 => {
                let hand = match look.hover {
                    Some(CardTarget::Button | CardTarget::Close) => true,
                    // A picture alone opens with a click anywhere on it.
                    Some(CardTarget::Card) => card.content.picture_only || card.content.clickable,
                    _ => false,
                };
                popup::set_pointer(if hand { Pointer::Hand } else { Pointer::Arrow });
                return Some(LRESULT(1));
            }
            WM_MOUSEMOVE => {
                card.popup.track_leave();
                look.hovered = true;
                look.hover = Some(card.layout.at(x, y, true));
            }
            popup::WM_MOUSELEAVE => {
                look.hovered = false;
                look.hover = None;
                if card.expired.get() && card.press.get().is_none() {
                    self.leave(id, &card, None);
                    return Some(LRESULT(0));
                }
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                // SAFETY: plain call on a window of this thread.
                unsafe { SetCapture(card.popup.hwnd) };
                let at = card.layout.at(x, y, look.hovered);
                card.press.set(Some(at));
                if matches!(at, CardTarget::Card | CardTarget::Button) {
                    card.press(gfx, true);
                }
            }
            // Another window took the mouse: its release will not come here.
            WM_CAPTURECHANGED => {
                if matches!(card.press.take(), Some(CardTarget::Card | CardTarget::Button)) {
                    card.press(gfx, false);
                }
            }
            WM_RBUTTONUP if !card.content.menu.is_empty() => {
                card.set_timer(gfx, false);
                let chosen = popup::context_menu(card.popup.hwnd, &card.content.menu, card.content.dark);
                let event = match chosen {
                    Some(0) => Some(Some(ToastEvent::Activated)),
                    Some(1) => Some(Some(ToastEvent::Edit)),
                    Some(2) => Some(Some(ToastEvent::Action)),
                    Some(_) => Some(None),
                    None => None,
                };
                if let Some(event) = event {
                    self.activate(id, &card, event);
                    return Some(LRESULT(0));
                }
                // Dismissed: the card stays while the pointer is on it, then goes.
                card.expired.set(true);
                if !card.popup.has_pointer() {
                    self.leave(id, &card, None);
                    return Some(LRESULT(0));
                }
            }
            WM_LBUTTONUP => {
                // Taken before the release, which reports a loss of the mouse.
                let pressed = card.press.take();
                // SAFETY: plain call.
                let _ = unsafe { ReleaseCapture() };
                let at = card.layout.at(x, y, look.hovered);
                if pressed == Some(at) && at == CardTarget::Close {
                    self.leave(id, &card, None);
                    return Some(LRESULT(0));
                }
                let event = match at {
                    _ if pressed != Some(at) => None,
                    CardTarget::Card => Some(ToastEvent::Activated),
                    CardTarget::Button => Some(ToastEvent::Action),
                    CardTarget::Close | CardTarget::Outside => None,
                };
                if let Some(event) = event {
                    self.activate(id, &card, Some(event));
                    return Some(LRESULT(0));
                }
                if matches!(pressed, Some(CardTarget::Card | CardTarget::Button)) {
                    card.press(gfx, false);
                }
                // Let go away from the card once its time is up.
                if card.expired.get() && !card.popup.has_pointer() {
                    self.leave(id, &card, None);
                    return Some(LRESULT(0));
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
                gfx: Gfx::shared()?,
                current: RefCell::default(),
                count: Cell::new(0),
                trim_due: Cell::new(false),
            }),
        })
    }

    /// No card is on screen.
    pub fn is_empty(&self) -> bool {
        self.shared.current.borrow().is_none()
    }

    /// Shows a card in the bottom-right (or top-right) corner of `work`, the work area of a monitor (physical pixels,
    /// at `dpi`), in the place of the one on screen (whose `done` is not called). `done` learns what the user did
    /// (`None`: the card went by itself or was closed) once the card is gone.
    pub fn show(
        &self,
        content: &ToastContent,
        work: PhysicalRect,
        dpi: u32,
        animate: bool,
        done: impl FnOnce(Option<ToastEvent>) + 'static,
    ) -> Result<()> {
        let shared = &self.shared;
        let gfx = &shared.gfx;
        let layout = CardLayout::new(gfx, content, dpi)?;
        let u = layout.u;
        let (x, y) = corner(work, layout.size, (12.0 * u) as u32, content.top);
        // Never in a capture or a recording: the card is about them, not part of them.
        let popup =
            Popup::new(gfx, PhysicalRect::new(x, y, layout.size.0, layout.size.1), Focus::Never, Capture::Hidden)?;
        let piece = Piece::new(gfx, true)?;
        scene::add(&popup.root, &piece.visual)?;
        let bitmap = |picture: &Option<RgbaImage>| match picture {
            Some(t) => match gfx.rgba_bitmap(t.width, t.height, &t.data) {
                Ok(b) => Some(b),
                Err(e) => {
                    tracing::warn!("notification picture: {e}");
                    None
                }
            },
            None => None,
        };
        let (thumb, app_icon) = (bitmap(&content.thumb), bitmap(&content.app_icon));
        let theme = Theme::new(Look::new(content.dark, vixeeny_platform::system_accent(), animate));
        // The countdown bar, over the card, shrinking towards its left end.
        let bar = Piece::new(gfx, false)?;
        scene::add(&piece.visual, &bar.visual)?;
        let (bx, by, bw, bh) = layout.countdown;
        let bar_size = (bw.round().max(1.0) as u32, bh.round().max(1.0) as u32);
        bar.show(gfx, &true, || Ok(bar_size), |c| paint_countdown(c, &theme, bar_size.0 as f32, bar_size.1 as f32))?;
        bar.move_to(bx.round(), by.round())?;
        let (cx, cy, cw, ch) = layout.card;
        // SAFETY: plain creation and property calls on visuals of the same device.
        let (bar_scale, press_scale) = unsafe {
            let bar_scale = gfx.dcomp.CreateScaleTransform()?;
            bar.visual.SetTransform(&bar_scale)?;
            let press_scale = gfx.dcomp.CreateScaleTransform()?;
            press_scale.SetCenterX2(cx + cw / 2.0)?;
            press_scale.SetCenterY2(cy + ch / 2.0)?;
            piece.visual.SetTransform(&press_scale)?;
            (bar_scale, press_scale)
        };
        let card = Rc::new(Card {
            content: content.clone(),
            layout,
            theme,
            popup,
            piece,
            thumb,
            app_icon,
            _bar: bar,
            bar_scale,
            press_scale,
            look: Cell::new(CardLook { hovered: false, hover: None, held: None }),
            press: Cell::new(None),
            done: RefCell::new(Some(Box::new(done))),
            animate,
            leaving: Cell::new(None),
            expired: Cell::new(false),
        });
        let id = shared.count.get() + 1;
        shared.count.set(id);
        let weak: Weak<Shared> = Rc::downgrade(shared);
        card.popup.on_message(move |msg, wparam, lparam| weak.upgrade()?.message(id, msg, wparam, lparam));
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
                card.piece.visual.SetOffsetX(&scene::ease_out(gfx, slide, 0.0, ENTER)?)?;
                effect.SetOpacity(&scene::ease_out(gfx, 0.0, 1.0, ENTER)?)?;
                gfx.dcomp.Commit()?;
            }
        }
        card.set_timer(gfx, true);
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
        // Above a 48-pixel taskbar.
        assert_eq!(corner(PhysicalRect::new(0, 0, 1920, 1032), (360, 96), 16, false), (1544, 920));
        // A second monitor to the left keeps its own corner; a taskbar on top moves nothing.
        assert_eq!(corner(PhysicalRect::new(-1920, 40, 1920, 1040), (360, 96), 16, false), (-376, 968));
        // At the top, under a taskbar on top.
        assert_eq!(corner(PhysicalRect::new(0, 0, 1920, 1032), (360, 96), 16, true), (1544, 16));
        assert_eq!(corner(PhysicalRect::new(-1920, 40, 1920, 1040), (360, 96), 16, true), (-376, 56));
    }
}
