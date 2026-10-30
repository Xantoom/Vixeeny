// SPDX-License-Identifier: GPL-3.0-or-later
//! The recording widget, drawn natively: a small bar with a blinking red dot, the elapsed time, and the pause / stop
//! buttons, as wide as what it shows. It stays where it was put, held by the side of its corner, and comes in from the
//! screen edge it is on and leaves towards it. When something goes wrong, a warning sign comes out from behind the bar
//! in a box of its own, on the side away from that corner, so the buttons never move. In pause the dot is amber and
//! still and the time frozen. With `auto_hide` it fades after 3 s and comes back under the pointer. It never takes the
//! keyboard and is left out of every screen capture.
//!
//! While recording it draws once a second (the time and the dot), nothing in between.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use vixeeny_platform::PhysicalRect;
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::DirectComposition::{
    IDCompositionEffectGroup, IDCompositionScaleTransform, IDCompositionVisual2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, KillTimer, SetTimer, WM_CAPTURECHANGED, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_SETCURSOR, WM_TIMER,
};
use windows::core::Result;

use crate::gfx::{Canvas, Gfx, rect};
use crate::icons;
use crate::popup::{self, Popup};
use crate::scene::{self, Piece};
use crate::side::Box2;
use crate::theme::{Look, Rgba, Theme};
use crate::window::{Capture, Focus, Pointer};

/// What the buttons ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetEvent {
    TogglePause,
    Stop,
}

/// When the widget is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    Always,
    /// It fades after a few seconds and comes back under the pointer.
    Fade,
    /// Only while something goes wrong (see [`Alert`]).
    Alerts,
}

impl Display {
    /// `always`, `fade` or `alerts` (the config's words); anything else is `Always`.
    pub fn from_name(name: &str) -> Self {
        match name {
            "fade" => Self::Fade,
            "alerts" => Self::Alerts,
            _ => Self::Always,
        }
    }
}

/// `M:SS`, then `H:MM:SS` from an hour on.
pub fn format_elapsed(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    if s < 3600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }
}

/// The time shown: it counts by itself between two updates and freezes while paused.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    base: Duration,
    since: Instant,
    paused: bool,
}

impl Clock {
    pub fn new(now: Instant) -> Self {
        Self { base: Duration::ZERO, since: now, paused: false }
    }

    pub fn set(&mut self, paused: bool, elapsed: Duration, now: Instant) {
        self.base = elapsed;
        self.since = now;
        self.paused = paused;
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        if self.paused { self.base } else { self.base + now.saturating_duration_since(self.since) }
    }
}

// Logical pixels: the bar, the warning sign's box beside it, and room around both for their shadows.
pub const HEIGHT: u32 = 60;
const MARGIN: f32 = 10.0;
const MAX_BAR_W: f32 = 160.0;
const BAR_H: f32 = HEIGHT as f32 - 2.0 * MARGIN;
/// The warning sign's box, square, and its gap to the bar.
const SIGN: f32 = BAR_H;
const SIGN_GAP: f32 = 6.0;
/// The widest window: the widest bar and the warning sign's box.
pub const WIDTH: u32 = (2.0 * MARGIN + MAX_BAR_W + SIGN_GAP + SIGN) as u32;
const BUTTON: f32 = 30.0;
/// From the bar's ends to its buttons.
const INSET: f32 = 5.0;
/// Where the time starts, after the state button.
const TIME_X: f32 = INSET + BUTTON + 6.0;
/// From the time to stop.
const TIME_GAP: f32 = 8.0;
const TIME_SIZE: f32 = 14.0;
const FADE_AFTER_MS: u32 = 3000;
const FADED: f32 = 0.15;
/// Coming in and leaving: how long, how far from the screen edge, from what size.
const RISE: f64 = 0.22;
const LEAVE: f64 = 0.16;
const RISE_BY: f32 = 10.0;
const SMALL: f32 = 0.92;
const FADE: f64 = 0.26;
/// The warning sign coming out from behind the bar, and going back.
const SIGN_IN: f64 = 0.28;
const SIGN_OUT: f64 = 0.18;
/// Width the tooltip's text wraps at.
const TIP_WIDTH: f32 = 300.0;
const TIP_PAD: f32 = 10.0;
/// Room around the tooltip's box for its shadow.
const TIP_SHADOW: f32 = 8.0;
const TIP_DELAY_MS: u32 = 300;

/// What goes wrong with the recording, as the widget shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Alert {
    #[default]
    None,
    /// Something to know (frames lost, the disk filling up): amber.
    Warning,
    /// The recording is about to fail: red.
    Critical,
}

/// A side of the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// What is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetTarget {
    /// The red dot while recording, the pause sign while paused: a click switches.
    State,
    /// The warning sign, while something goes wrong.
    Alert,
    Stop,
    /// The rest of the bar.
    Bar,
    Outside,
}

/// The buttons and the whole of a bar `width` wide, logical pixels from its top-left corner.
fn part(target: WidgetTarget, width: f32) -> Box2 {
    let y = (BAR_H - BUTTON) / 2.0;
    match target {
        WidgetTarget::State => (INSET, y, BUTTON, BUTTON),
        WidgetTarget::Stop => (width - INSET - BUTTON, y, BUTTON, BUTTON),
        _ => (0.0, 0.0, width, BAR_H),
    }
}

/// Where the bar and the warning sign's box start in the window (logical pixels; the box only while the window has room
/// for it).
fn layout(look: &WidgetLook) -> (f32, Option<f32>) {
    match look.sign_at {
        None => (MARGIN, None),
        Some(Side::Left) => (MARGIN + SIGN + SIGN_GAP, Some(MARGIN)),
        Some(Side::Right) => (MARGIN, Some(MARGIN + look.width as f32 + SIGN_GAP)),
    }
}

/// The left edges of the bar's surface and of the warning sign's, window pixels.
fn surfaces_at(u: f32, look: &WidgetLook) -> (f32, Option<f32>) {
    let (bar, sign) = layout(look);
    (((bar - MARGIN) * u).round(), sign.map(|x| ((x - MARGIN) * u).round()))
}

/// What is at `(x, y)` on the widget `look` shows, window pixels at `u` physical pixels per logical one; the warning
/// sign is there only while something goes wrong.
pub fn widget_at(u: f32, x: f32, y: f32, look: &WidgetLook) -> WidgetTarget {
    let (bar, sign) = layout(look);
    let (lx, ly) = (x / u, y / u - MARGIN);
    let inside = |(bx, by, bw, bh): Box2| lx >= bx && lx < bx + bw && ly >= by && ly < by + bh;
    if let Some(sx) = sign
        && look.alert != Alert::None
        && inside((sx, 0.0, SIGN, BAR_H))
    {
        return WidgetTarget::Alert;
    }
    let width = look.width as f32;
    let on_bar = |(bx, by, bw, bh): Box2| inside((bar + bx, by, bw, bh));
    if !on_bar(part(WidgetTarget::Bar, width)) {
        return WidgetTarget::Outside;
    }
    [WidgetTarget::State, WidgetTarget::Stop].into_iter().find(|t| on_bar(part(*t, width))).unwrap_or(WidgetTarget::Bar)
}

/// `time` laid out as the bar shows it, its digits all as wide.
fn time_text(gfx: &Gfx, u: f32, time: &str) -> Result<crate::gfx::Paragraph> {
    gfx.paragraph_tabular(time, TIME_SIZE * u, true, 1.0e5)
}

/// The width of the bar (logical pixels) for `time`.
pub fn bar_width(gfx: &Gfx, u: f32, time: &str) -> Result<u32> {
    let width = TIME_X + time_text(gfx, u, time)?.width / u + TIME_GAP + BUTTON + INSET;
    Ok(width.ceil().min(MAX_BAR_W) as u32)
}

/// What the widget shows; drawn again only when this changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetLook {
    pub paused: bool,
    pub time: String,
    /// The halo around the dot, on every other second while recording.
    pub pulse: bool,
    pub alert: Alert,
    pub hover: Option<WidgetTarget>,
    pub held: Option<WidgetTarget>,
    /// The bar's width, logical pixels (see [`bar_width`]).
    pub width: u32,
    /// Where the window has room for the warning sign's box (it stays a moment after the sign, while the box goes).
    pub sign_at: Option<Side>,
}

/// Size of the bar's surface.
fn bar_surface(u: f32, look: &WidgetLook) -> (u32, u32) {
    (((look.width as f32 + 2.0 * MARGIN) * u).ceil() as u32, (HEIGHT as f32 * u).ceil() as u32)
}

/// Size of the warning sign's surface.
fn sign_surface(u: f32) -> (u32, u32) {
    (((SIGN + 2.0 * MARGIN) * u).ceil() as u32, (HEIGHT as f32 * u).ceil() as u32)
}

/// Size of the window: the bar's surface and the warning sign's.
pub fn widget_surface(u: f32, look: &WidgetLook) -> (u32, u32) {
    let (bar, sign) = surfaces_at(u, look);
    let (bw, h) = bar_surface(u, look);
    let w = bar as u32 + bw;
    (sign.map_or(w, |x| w.max(x as u32 + sign_surface(u).0)), h)
}

/// The bar, its surface's left edge at `x0`.
pub fn paint_widget(c: &Canvas<'_>, t: &Theme, u: f32, look: &WidgetLook, x0: f32) -> Result<()> {
    let (ox, oy) = (x0 + MARGIN * u, MARGIN * u);
    let width = look.width as f32;
    let bar = rect(ox, oy, width * u, BAR_H * u);
    c.shadow(bar, 10.0 * u, 10.0 * u, 5.0 * u, Rgba::hex(0x000000, 0x55))?;
    c.fill_round(bar, 10.0 * u, t.flyout)?;
    c.stroke_round(bar, 10.0 * u, 1.0, t.stroke_strong)?;
    let at = |target| {
        let (x, y, w, h) = part(target, width);
        (ox + x * u, oy + y * u, w * u, h * u)
    };
    let back = |target: WidgetTarget| -> Result<()> {
        let fill = if look.held == Some(target) {
            t.subtle_pressed
        } else if look.hover == Some(target) {
            t.subtle_hover
        } else {
            return Ok(());
        };
        let (x, y, w, h) = at(target);
        c.fill_round(rect(x, y, w, h), 6.0 * u, fill)
    };

    // Recording: the red dot, with a halo that blinks. Paused: the pause sign, amber. Under the pointer: what a click
    // does.
    back(WidgetTarget::State)?;
    let (sx, sy, sw, sh) = at(WidgetTarget::State);
    let (cx, cy) = (sx + sw / 2.0, sy + sh / 2.0);
    let icon = 16.0 * u;
    if look.hover == Some(WidgetTarget::State) {
        let action = if look.paused { icons::PLAY_FILLED } else { icons::PAUSE_FILLED };
        c.icon(action, cx - icon / 2.0, cy - icon / 2.0, icon, t.text)?;
    } else if look.paused {
        c.icon(icons::PAUSE_FILLED, cx - icon / 2.0, cy - icon / 2.0, icon, t.warning)?;
    } else {
        if look.pulse {
            c.fill_circle(cx, cy, 9.0 * u, t.record.with_alpha(0.25))?;
        }
        c.fill_circle(cx, cy, 6.0 * u, t.record)?;
    }

    // The time, frozen and dimmed while paused.
    let colour = if look.paused { t.text_2 } else { t.text };
    let time = time_text(c.gfx, u, &look.time)?;
    c.text_layout(&time.layout, ox + TIME_X * u, oy + (BAR_H * u - time.height) / 2.0, colour)?;

    back(WidgetTarget::Stop)?;
    let (x, y, w, h) = at(WidgetTarget::Stop);
    c.icon(icons::STOP_FILLED, x + (w - icon) / 2.0, y + (h - icon) / 2.0, icon, t.record)?;
    Ok(())
}

/// The warning sign in its box, the surface's left edge at `x0`: amber, red when critical.
pub fn paint_sign(c: &Canvas<'_>, t: &Theme, u: f32, alert: Alert, x0: f32) -> Result<()> {
    let (ox, oy) = (x0 + MARGIN * u, MARGIN * u);
    let r = rect(ox, oy, SIGN * u, BAR_H * u);
    c.shadow(r, 10.0 * u, 10.0 * u, 5.0 * u, Rgba::hex(0x000000, 0x55))?;
    c.fill_round(r, 10.0 * u, t.flyout)?;
    c.stroke_round(r, 10.0 * u, 1.0, t.stroke_strong)?;
    let tint = if alert == Alert::Critical { t.danger } else { t.warning };
    let icon = 18.0 * u;
    c.icon(icons::WARNING, ox + (SIGN * u - icon) / 2.0, oy + (BAR_H * u - icon) / 2.0, icon, tint)
}

/// The whole window as it shows when nothing moves (tests).
#[doc(hidden)]
pub fn paint_window(c: &Canvas<'_>, t: &Theme, u: f32, look: &WidgetLook) -> Result<()> {
    let (bar, sign) = surfaces_at(u, look);
    if let Some(x) = sign
        && look.alert != Alert::None
    {
        paint_sign(c, t, u, look.alert, x)?;
    }
    paint_widget(c, t, u, look, bar)
}

/// The tooltip of the warning sign: its text laid out, and the size of its window.
pub struct AlertTip {
    pub text: crate::gfx::Paragraph,
    pub size: (u32, u32),
}

impl AlertTip {
    pub fn new(gfx: &Gfx, u: f32, text: &str) -> Result<Self> {
        let text = gfx.paragraph(text, 13.0 * u, false, TIP_WIDTH * u)?;
        let side = 2.0 * (TIP_PAD + TIP_SHADOW) * u;
        let size = ((text.width + side).ceil() as u32, (text.height + side).ceil() as u32);
        Ok(Self { text, size })
    }
}

pub fn paint_alert_tip(c: &Canvas<'_>, t: &Theme, u: f32, tip: &AlertTip) -> Result<()> {
    let m = TIP_SHADOW * u;
    let r = rect(m, m, tip.size.0 as f32 - 2.0 * m, tip.size.1 as f32 - 2.0 * m);
    c.shadow(r, 6.0 * u, 8.0 * u, 2.0 * u, Rgba::hex(0x000000, 0x35))?;
    c.fill_round(r, 6.0 * u, t.flyout)?;
    c.stroke_round(r, 6.0 * u, 1.0, t.stroke_strong)?;
    c.text_layout(&tip.text.layout, m + TIP_PAD * u, m + TIP_PAD * u, t.text)
}

/// Where the tooltip goes: under the widget, or above it when the widget is in the lower half of `work`; its middle
/// under the warning sign at `anchor_x`, whole inside `work`.
pub fn tip_origin(widget: RECT, anchor_x: i32, size: (u32, u32), work: RECT) -> (i32, i32) {
    let (w, h) = (size.0 as i32, size.1 as i32);
    let below = widget.top + widget.bottom < work.top + work.bottom;
    let y = if below { widget.bottom } else { widget.top - h };
    popup::keep_inside((anchor_x - w / 2, y), (w, h), work)
}

const TICK_TIMER: usize = 1;
const FADE_TIMER: usize = 2;
const TIP_TIMER: usize = 3;
/// The widget has left.
const LEAVE_TIMER: usize = 4;
/// The warning sign's box has gone back behind the bar.
const SIGN_TIMER: usize = 5;

/// What follows the widget's leaving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    Close,
    Withdraw,
}

type OnEvent = Box<dyn Fn(WidgetEvent)>;

/// The tooltip on screen: a window of its own beside the bar.
struct TipWindow {
    popup: Popup,
    piece: Piece<String>,
}

struct Inner {
    gfx: std::rc::Rc<Gfx>,
    theme: Theme,
    u: f32,
    popup: Popup,
    /// The window at its widest, and where the widget is on the screen (0 left or top, 1 right or bottom).
    room: PhysicalRect,
    anchor: (f32, f32),
    /// Bar and warning sign together, to come in, fade and leave.
    group: IDCompositionVisual2,
    effect: IDCompositionEffectGroup,
    scale: IDCompositionScaleTransform,
    bar: Piece<WidgetLook>,
    sign: Piece<Alert>,
    leaving: Cell<Option<Then>>,
    auto_hide: bool,
    /// Only on screen while something goes wrong.
    alerts_only: bool,
    /// On screen (it is withdrawn while there is no alert, with `alerts_only`).
    shown: Cell<bool>,
    animate: bool,
    clock: Cell<Clock>,
    look: RefCell<WidgetLook>,
    /// The words of the warning sign's tooltip.
    alert_text: RefCell<String>,
    tip: RefCell<Option<TipWindow>>,
    press: Cell<Option<WidgetTarget>>,
    hovered: Cell<bool>,
    /// Faded out (auto-hide).
    faded: Cell<bool>,
    /// The opacity it shows, to fade from.
    opacity: Cell<f32>,
    on_event: RefCell<Option<OnEvent>>,
    closed: Cell<bool>,
}

impl Inner {
    fn refresh(&self) {
        let look = self.look.borrow().clone();
        let u = self.u;
        let (bar_x, _) = surfaces_at(u, &look);
        let theme = &self.theme;
        let drawn = (|| {
            self.bar.show(&self.gfx, &look, || Ok(bar_surface(u, &look)), |c| paint_widget(c, theme, u, &look, 0.0))?;
            self.bar.move_to(bar_x, 0.0)?;
            if look.alert != Alert::None {
                self.sign.show(
                    &self.gfx,
                    &look.alert,
                    || Ok(sign_surface(u)),
                    |c| paint_sign(c, theme, u, look.alert, 0.0),
                )?;
            }
            // SAFETY: plain call.
            unsafe { self.gfx.dcomp.Commit() }
        })();
        if let Err(e) = drawn {
            tracing::warn!("recording widget: {e}");
        }
    }

    /// The side of the bar away from its corner: where the warning sign goes.
    fn sign_side(&self) -> Side {
        if self.anchor.0 > 0.5 { Side::Left } else { Side::Right }
    }

    /// Sizes the bar to what it shows, with room for the warning sign while something goes wrong, and the window with
    /// them: the bar keeps its place by the side of its corner.
    fn fit(&self) {
        let (width, sign_at) = {
            let look = self.look.borrow();
            let width = match bar_width(&self.gfx, self.u, &look.time) {
                Ok(w) => w,
                Err(e) => return tracing::warn!("recording widget: {e}"),
            };
            (width, if look.alert == Alert::None { look.sign_at } else { Some(self.sign_side()) })
        };
        {
            let mut look = self.look.borrow_mut();
            if (look.width, look.sign_at) == (width, sign_at) {
                return;
            }
            (look.width, look.sign_at) = (width, sign_at);
        }
        self.place();
    }

    /// Puts the window where the bar keeps its place.
    fn place(&self) {
        let look = self.look.borrow();
        let u = self.u;
        let (w, h) = widget_surface(u, &look);
        let (bar_x, _) = surfaces_at(u, &look);
        let room = self.room;
        let alone = bar_surface(u, &look).0;
        let x = room.x + ((room.width as f32 - alone as f32) * self.anchor.0).round() as i32 - bar_x as i32;
        self.popup.place(PhysicalRect::new(x, room.y, w, h));
        // The bar's middle, for growing and shrinking.
        let centre = bar_x + (MARGIN + look.width as f32 / 2.0) * u;
        // SAFETY: plain property calls.
        let set =
            unsafe { self.scale.SetCenterX2(centre).and_then(|()| self.scale.SetCenterY2(HEIGHT as f32 * u / 2.0)) };
        if let Err(e) = set {
            tracing::warn!("recording widget: {e}");
        }
    }

    /// Fades the widget to `to`.
    fn fade_to(&self, to: f32) {
        let from = self.opacity.replace(to);
        // SAFETY: plain property calls with an animation of the same device.
        let set = unsafe {
            if self.animate && from != to {
                scene::ease_out(&self.gfx, from, to, FADE).and_then(|a| self.effect.SetOpacity(&a))
            } else {
                self.effect.SetOpacity2(to)
            }
        };
        if let Err(e) = set {
            tracing::warn!("recording widget: {e}");
        }
    }

    /// How far off the widget comes in from and leaves to: towards the screen edge it is on.
    fn rise(&self) -> f32 {
        RISE_BY * self.u * if self.anchor.1 < 0.5 { -1.0 } else { 1.0 }
    }

    /// Puts the window on screen, coming in from its screen edge.
    fn appear(&self) -> Result<()> {
        if self.leaving.take().is_some() {
            self.timer(LEAVE_TIMER, None);
            self.shown.set(false);
        }
        if self.shown.replace(true) {
            return Ok(());
        }
        let gfx = &self.gfx;
        let rise = self.rise();
        self.opacity.set(1.0);
        self.faded.set(false);
        let motion = self.animate;
        // SAFETY: plain property calls.
        unsafe {
            self.group.SetOffsetY2(if motion { rise } else { 0.0 })?;
            self.effect.SetOpacity2(if motion { 0.0 } else { 1.0 })?;
            let size = if motion { SMALL } else { 1.0 };
            self.scale.SetScaleX2(size)?;
            self.scale.SetScaleY2(size)?;
        }
        self.refresh();
        self.popup.show(gfx)?;
        if motion {
            // SAFETY: plain property calls with animations of the same device.
            unsafe {
                self.group.SetOffsetY(&scene::ease_out(gfx, rise, 0.0, RISE)?)?;
                self.effect.SetOpacity(&scene::ease_out(gfx, 0.0, 1.0, RISE)?)?;
                let grow = scene::ease_out(gfx, SMALL, 1.0, RISE)?;
                self.scale.SetScaleX(&grow)?;
                self.scale.SetScaleY(&grow)?;
                gfx.dcomp.Commit()?;
            }
        }
        if self.auto_hide {
            self.timer(FADE_TIMER, Some(FADE_AFTER_MS));
        }
        Ok(())
    }

    /// Takes the widget off the screen, towards its screen edge, then `then`.
    fn leave(&self, then: Then) {
        if !self.shown.get() {
            return self.finish(then);
        }
        self.hide_tip();
        self.timer(FADE_TIMER, None);
        if !self.animate {
            return self.finish(then);
        }
        self.leaving.set(Some(then));
        let gfx = &self.gfx;
        let from = self.opacity.replace(0.0);
        // SAFETY: plain property calls with animations of the same device.
        let set = (|| unsafe {
            self.group.SetOffsetY(&scene::ease_in(gfx, 0.0, self.rise(), LEAVE)?)?;
            self.effect.SetOpacity(&scene::ease_in(gfx, from, 0.0, LEAVE)?)?;
            let shrink = scene::ease_in(gfx, 1.0, SMALL, LEAVE)?;
            self.scale.SetScaleX(&shrink)?;
            self.scale.SetScaleY(&shrink)?;
            gfx.dcomp.Commit()
        })();
        if let Err(e) = set {
            tracing::warn!("recording widget: {e}");
            self.leaving.set(None);
            return self.finish(then);
        }
        self.timer(LEAVE_TIMER, Some((LEAVE * 1000.0) as u32 + 20));
    }

    fn finish(&self, then: Then) {
        match then {
            Then::Close => self.closed.set(true),
            Then::Withdraw => {
                if self.shown.replace(false) {
                    self.hide_tip();
                    self.popup.withdraw();
                }
            }
        }
    }

    /// Brings the warning sign's box out from behind the bar.
    fn sign_in(&self) {
        self.timer(SIGN_TIMER, None);
        self.fit();
        self.refresh();
        let look = self.look.borrow().clone();
        let (Some(side), (_, Some(x))) = (look.sign_at, surfaces_at(self.u, &look)) else {
            return;
        };
        let behind = x + (SIGN + SIGN_GAP) * self.u * if side == Side::Left { 1.0 } else { -1.0 };
        let gfx = &self.gfx;
        let moved = (|| {
            self.sign.move_to(x, 0.0)?;
            let Some(effect) = &self.sign.effect else {
                return Ok(());
            };
            // SAFETY: plain property calls with animations of the same device.
            unsafe {
                if self.animate {
                    self.sign.visual.SetOffsetX(&scene::ease_out(gfx, behind, x, SIGN_IN)?)?;
                    effect.SetOpacity(&scene::ease_out(gfx, 0.0, 1.0, SIGN_IN)?)?;
                } else {
                    effect.SetOpacity2(1.0)?;
                }
                gfx.dcomp.Commit()
            }
        })();
        if let Err(e) = moved {
            tracing::warn!("recording widget: {e}");
        }
    }

    /// Sends the warning sign's box back behind the bar; the window narrows once it is there.
    fn sign_out(&self) {
        let look = self.look.borrow().clone();
        let (Some(side), (_, Some(x))) = (look.sign_at, surfaces_at(self.u, &look)) else {
            return;
        };
        if !self.animate {
            return self.sign_gone();
        }
        let behind = x + (SIGN + SIGN_GAP) * self.u * if side == Side::Left { 1.0 } else { -1.0 };
        let gfx = &self.gfx;
        let moved = (|| {
            let Some(effect) = &self.sign.effect else {
                return Ok(());
            };
            // SAFETY: plain property calls with animations of the same device.
            unsafe {
                self.sign.visual.SetOffsetX(&scene::ease_in(gfx, x, behind, SIGN_OUT)?)?;
                effect.SetOpacity(&scene::ease_in(gfx, 1.0, 0.0, SIGN_OUT)?)?;
                gfx.dcomp.Commit()
            }
        })();
        if let Err(e) = moved {
            tracing::warn!("recording widget: {e}");
        }
        self.timer(SIGN_TIMER, Some((SIGN_OUT * 1000.0) as u32 + 20));
    }

    /// The warning sign's box is gone: the window narrows to the bar.
    fn sign_gone(&self) {
        self.timer(SIGN_TIMER, None);
        if self.look.borrow().alert != Alert::None {
            return;
        }
        self.look.borrow_mut().sign_at = None;
        if let Err(e) = self.sign.hide() {
            tracing::warn!("recording widget: {e}");
        }
        self.place();
        self.refresh();
    }

    /// Back to full opacity; with auto-hide, it fades again a while later.
    fn wake(&self) {
        if self.faded.replace(false) {
            self.fade_to(1.0);
        }
        if self.auto_hide && !self.hovered.get() {
            self.timer(FADE_TIMER, Some(FADE_AFTER_MS));
        }
    }

    fn timer(&self, id: usize, on: Option<u32>) {
        // SAFETY: plain timer calls on a window of this thread.
        unsafe {
            match on {
                Some(ms) => {
                    SetTimer(Some(self.popup.hwnd), id, ms, None);
                }
                None => {
                    let _ = KillTimer(Some(self.popup.hwnd), id);
                }
            }
        }
    }

    /// Shows the time now and waits for the next second of it.
    fn tick(&self) {
        let clock = self.clock.get();
        let elapsed = clock.elapsed(Instant::now());
        {
            let mut look = self.look.borrow_mut();
            look.time = format_elapsed(elapsed);
            look.paused = clock.paused;
        }
        self.fit();
        if clock.paused {
            self.timer(TICK_TIMER, None);
        } else {
            let to_next = 1000 - (elapsed.as_millis() % 1000) as u32;
            self.timer(TICK_TIMER, Some(to_next + 5));
        }
    }

    /// Shows the tooltip of the warning sign beside the bar.
    fn show_tip(&self) {
        let text = self.alert_text.borrow().clone();
        if text.is_empty() {
            return self.hide_tip();
        }
        let shown = self.tip.borrow().as_ref().and_then(|t| t.piece.look.borrow().clone());
        if shown.as_deref() == Some(text.as_str()) {
            return;
        }
        self.hide_tip();
        if let Err(e) = self.open_tip(&text) {
            tracing::warn!("recording widget tooltip: {e}");
        }
    }

    fn open_tip(&self, text: &str) -> Result<()> {
        let u = self.u;
        let tip = AlertTip::new(&self.gfx, u, text)?;
        let mut bar = RECT::default();
        // SAFETY: a valid out-pointer, a window of this thread.
        unsafe { GetWindowRect(self.popup.hwnd, &mut bar)? };
        let (_, sign) = layout(&self.look.borrow());
        let anchor = bar.left + ((sign.unwrap_or(MARGIN) + SIGN / 2.0) * u) as i32;
        let centre = POINT { x: i32::midpoint(bar.left, bar.right), y: i32::midpoint(bar.top, bar.bottom) };
        let work = popup::work_area_at(centre).unwrap_or(bar);
        let (x, y) = tip_origin(bar, anchor, tip.size, work);
        // In the video as the bar is.
        let popup = Popup::new(&self.gfx, PhysicalRect::new(x, y, tip.size.0, tip.size.1), Focus::Never, Capture::Own)?;
        let piece = Piece::new(&self.gfx, false)?;
        scene::add(&popup.root, &piece.visual)?;
        piece.move_to(0.0, 0.0)?;
        piece.show(&self.gfx, &text.to_owned(), || Ok(tip.size), |c| paint_alert_tip(c, &self.theme, u, &tip))?;
        // SAFETY: plain call.
        unsafe { self.gfx.dcomp.Commit()? };
        popup.on_message(|msg, _, _| popup::passive(msg));
        popup.show(&self.gfx)?;
        *self.tip.borrow_mut() = Some(TipWindow { popup, piece });
        Ok(())
    }

    fn hide_tip(&self) {
        self.timer(TIP_TIMER, None);
        if let Some(tip) = self.tip.borrow_mut().take() {
            tip.popup.hide();
        }
    }

    fn message(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        if let Some(r) = popup::passive(msg) {
            return Some(r);
        }
        let (x, y) = popup::point(lparam);
        match msg {
            WM_TIMER => match wparam.0 {
                TICK_TIMER => {
                    {
                        let mut look = self.look.borrow_mut();
                        look.pulse = !look.pulse;
                    }
                    self.tick();
                }
                LEAVE_TIMER => {
                    self.timer(LEAVE_TIMER, None);
                    if let Some(then) = self.leaving.take() {
                        self.finish(then);
                    }
                    return Some(LRESULT(0));
                }
                SIGN_TIMER => {
                    self.sign_gone();
                    return Some(LRESULT(0));
                }
                FADE_TIMER => {
                    self.timer(FADE_TIMER, None);
                    if !self.hovered.get() {
                        self.faded.set(true);
                        self.fade_to(FADED);
                    }
                }
                TIP_TIMER => {
                    self.timer(TIP_TIMER, None);
                    if self.look.borrow().hover == Some(WidgetTarget::Alert) {
                        self.show_tip();
                    }
                    return Some(LRESULT(0));
                }
                _ => return None,
            },
            WM_SETCURSOR if (lparam.0 & 0xffff) == 1 => {
                let pointer = match self.look.borrow().hover {
                    Some(WidgetTarget::State | WidgetTarget::Stop) => Pointer::Hand,
                    _ => Pointer::Arrow,
                };
                popup::set_pointer(pointer);
                return Some(LRESULT(1));
            }
            WM_MOUSEMOVE => {
                self.popup.track_leave();
                if !self.hovered.replace(true) {
                    self.timer(FADE_TIMER, None);
                    if self.faded.replace(false) {
                        self.fade_to(1.0);
                    }
                }
                let hover = widget_at(self.u, x, y, &self.look.borrow());
                let before = self.look.borrow_mut().hover.replace(hover);
                if before != Some(hover) {
                    if hover == WidgetTarget::Alert {
                        self.timer(TIP_TIMER, Some(TIP_DELAY_MS));
                    } else {
                        self.hide_tip();
                    }
                }
            }
            popup::WM_MOUSELEAVE => {
                self.hovered.set(false);
                self.look.borrow_mut().hover = None;
                self.hide_tip();
                if self.auto_hide {
                    self.timer(FADE_TIMER, Some(FADE_AFTER_MS));
                }
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                // SAFETY: plain call on a window of this thread.
                unsafe { SetCapture(self.popup.hwnd) };
                self.hide_tip();
                let at = widget_at(self.u, x, y, &self.look.borrow());
                self.press.set(Some(at));
            }
            // Another window took the mouse: its release will not come here.
            WM_CAPTURECHANGED => {
                self.press.set(None);
            }
            WM_LBUTTONUP => {
                // Taken before the release, which reports a loss of the mouse.
                let pressed = self.press.take();
                // SAFETY: plain call.
                let _ = unsafe { ReleaseCapture() };
                let at = widget_at(self.u, x, y, &self.look.borrow());
                if pressed == Some(at) {
                    let event = match at {
                        WidgetTarget::State => Some(WidgetEvent::TogglePause),
                        WidgetTarget::Stop => Some(WidgetEvent::Stop),
                        _ => None,
                    };
                    if let Some(event) = event
                        && let Some(f) = self.on_event.borrow().as_ref()
                    {
                        f(event);
                    }
                }
            }
            _ => return None,
        }
        {
            let mut look = self.look.borrow_mut();
            look.held = self.press.get().filter(|p| look.hover == Some(*p));
        }
        self.refresh();
        Some(LRESULT(0))
    }
}

/// The widget on screen.
pub struct Widget {
    inner: Rc<Inner>,
}

impl Widget {
    /// The widget within `geometry` (physical pixels, its widest window) on a monitor at `dpi`, at `anchor` on the
    /// screen (fractions across and down: 0 left or top, 1 right or bottom). A narrower bar keeps to that side.
    pub fn new(geometry: PhysicalRect, anchor: (f32, f32), dpi: u32, look: Look, display: Display) -> Result<Self> {
        let gfx = Gfx::shared()?;
        let u = dpi.max(48) as f32 / 96.0;
        // Out of the video unless the settings want Vixeeny's windows in it; never stealing the keyboard.
        let popup = Popup::new(&gfx, geometry, Focus::Never, Capture::Own)?;
        let group = scene::visual(&gfx)?;
        // SAFETY: plain creation and property calls on objects of this device.
        let (effect, scale) = unsafe {
            let effect = gfx.dcomp.CreateEffectGroup()?;
            group.SetEffect(&effect)?;
            let scale = gfx.dcomp.CreateScaleTransform()?;
            group.SetTransform(&scale)?;
            (effect, scale)
        };
        scene::add(&popup.root, &group)?;
        // The warning sign under the bar, to come out from behind it.
        let sign = Piece::new(&gfx, true)?;
        scene::add(&group, &sign.visual)?;
        let bar = Piece::new(&gfx, false)?;
        scene::add(&group, &bar.visual)?;
        let inner = Rc::new(Inner {
            theme: Theme::new(look),
            gfx,
            u,
            popup,
            room: geometry,
            anchor: (anchor.0.clamp(0.0, 1.0), anchor.1.clamp(0.0, 1.0)),
            group,
            effect,
            scale,
            bar,
            sign,
            leaving: Cell::new(None),
            auto_hide: display == Display::Fade,
            alerts_only: display == Display::Alerts,
            shown: Cell::new(false),
            animate: look.animations,
            clock: Cell::new(Clock::new(Instant::now())),
            look: RefCell::new(WidgetLook {
                paused: false,
                time: format_elapsed(Duration::ZERO),
                pulse: true,
                alert: Alert::None,
                hover: None,
                held: None,
                width: MAX_BAR_W as u32,
                sign_at: None,
            }),
            alert_text: RefCell::default(),
            tip: RefCell::default(),
            press: Cell::new(None),
            hovered: Cell::new(false),
            faded: Cell::new(false),
            opacity: Cell::new(1.0),
            on_event: RefCell::new(None),
            closed: Cell::new(false),
        });
        let weak: Weak<Inner> = Rc::downgrade(&inner);
        inner.popup.on_message(move |msg, wparam, lparam| weak.upgrade()?.message(msg, wparam, lparam));
        inner.fit();
        inner.place();
        Ok(Self { inner })
    }

    pub fn on_event(&self, f: impl Fn(WidgetEvent) + 'static) {
        *self.inner.on_event.borrow_mut() = Some(Box::new(f));
    }

    /// The recorder's state: paused or not, and the time recorded so far.
    pub fn set_state(&self, paused: bool, elapsed: Duration) {
        let inner = &self.inner;
        let mut clock = inner.clock.get();
        clock.set(paused, elapsed, Instant::now());
        inner.clock.set(clock);
        inner.tick();
        inner.refresh();
    }

    /// What goes wrong (`Alert::None`: nothing) and the tooltip of the warning sign. A new or worse problem brings a
    /// faded widget back.
    pub fn set_alert(&self, alert: Alert, text: &str) {
        let inner = &self.inner;
        let before = std::mem::replace(&mut inner.look.borrow_mut().alert, alert);
        *inner.alert_text.borrow_mut() = text.to_owned();
        if before == Alert::None && alert != Alert::None {
            inner.sign_in();
        } else if before != Alert::None && alert == Alert::None {
            inner.sign_out();
        }
        if inner.alerts_only && !inner.closed.get() {
            if alert == Alert::None {
                inner.leave(Then::Withdraw);
            } else if let Err(e) = inner.appear() {
                tracing::warn!("recording widget: {e}");
            }
        }
        if alert > before {
            inner.wake();
        }
        if alert == Alert::None {
            inner.hide_tip();
        } else if inner.tip.borrow().is_some() {
            inner.show_tip();
        }
        inner.refresh();
    }

    /// Shows the bar, rising into place (with [`Display::Alerts`], once something goes wrong), and runs until
    /// [`Widget::close`].
    pub fn run(&self) -> Result<()> {
        let inner = &self.inner;
        inner.tick();
        if !inner.alerts_only || inner.look.borrow().alert != Alert::None {
            inner.appear()?;
        }
        popup::pump_while(|| !inner.closed.get());
        inner.hide_tip();
        inner.popup.hide();
        Ok(())
    }

    /// Takes the widget off the screen and ends [`Widget::run`] (from this thread, e.g. from a mailbox).
    pub fn close(&self) {
        self.inner.leave(Then::Close);
    }

    /// The look the bar has now (tests).
    #[doc(hidden)]
    pub fn look(&self) -> WidgetLook {
        self.inner.look.borrow().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_counts_until_paused_then_freezes() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut c = Clock::new(t0);
        c.set(false, s(10), t0);
        assert_eq!(c.elapsed(t0 + s(3)), s(13));
        c.set(true, s(13), t0 + s(3));
        assert_eq!(c.elapsed(t0 + s(60)), s(13));
        c.set(false, s(13), t0 + s(60));
        assert_eq!(c.elapsed(t0 + s(62)), s(15));
    }

    #[test]
    fn elapsed_is_short_under_an_hour() {
        assert_eq!(format_elapsed(Duration::ZERO), "0:00");
        assert_eq!(format_elapsed(Duration::from_millis(59_999)), "0:59");
        assert_eq!(format_elapsed(Duration::from_secs(3_599)), "59:59");
        assert_eq!(format_elapsed(Duration::from_secs(3_725)), "1:02:05");
        assert_eq!(format_elapsed(Duration::from_secs(100 * 3600)), "100:00:00");
    }

    #[test]
    fn the_buttons_are_where_they_are_drawn() {
        // The bar is 10 px inside the window: the state at its left end, stop at its right end, the warning sign just
        // before stop; the rest is the bar.
        let look = |alert, sign_at| WidgetLook {
            paused: false,
            time: "0:05".into(),
            pulse: false,
            alert,
            hover: None,
            held: None,
            width: 150,
            sign_at,
        };
        let calm = look(Alert::None, None);
        for u in [1.0, 1.5] {
            assert_eq!(widget_at(u, 30.0 * u, 30.0 * u, &calm), WidgetTarget::State);
            assert_eq!(widget_at(u, 140.0 * u, 30.0 * u, &calm), WidgetTarget::Stop);
            assert_eq!(widget_at(u, 70.0 * u, 30.0 * u, &calm), WidgetTarget::Bar);
            assert_eq!(widget_at(u, 3.0 * u, 3.0 * u, &calm), WidgetTarget::Outside);
            assert_eq!(widget_at(u, 175.0 * u, 30.0 * u, &calm), WidgetTarget::Outside);
            // The warning sign on the right: the bar where it was, the sign after it.
            let right = look(Alert::Warning, Some(Side::Right));
            assert_eq!(widget_at(u, 140.0 * u, 30.0 * u, &right), WidgetTarget::Stop);
            assert_eq!(widget_at(u, 185.0 * u, 30.0 * u, &right), WidgetTarget::Alert);
            // On the left: the bar after it.
            let left = look(Alert::Critical, Some(Side::Left));
            assert_eq!(widget_at(u, 30.0 * u, 30.0 * u, &left), WidgetTarget::Alert);
            assert_eq!(widget_at(u, 76.0 * u, 30.0 * u, &left), WidgetTarget::State);
            assert_eq!(widget_at(u, 186.0 * u, 30.0 * u, &left), WidgetTarget::Stop);
            // Going away: no longer there to point at.
            assert_eq!(widget_at(u, 30.0 * u, 30.0 * u, &look(Alert::None, Some(Side::Left))), WidgetTarget::Outside);
        }
    }

    #[test]
    fn the_tooltip_goes_beside_the_bar_on_the_free_side() {
        let work = RECT { left: 0, top: 0, right: 1920, bottom: 1032 };
        let bar = |top| RECT { left: 10, top, right: 226, bottom: top + 60 };
        // Top-left corner: under the bar, kept on screen on the left.
        assert_eq!(tip_origin(bar(6), 160, (200, 50), work), (60, 66));
        assert_eq!(tip_origin(bar(6), 40, (200, 50), work), (0, 66));
        // Lower half: above it.
        assert_eq!(tip_origin(bar(966), 160, (200, 50), work), (60, 916));
    }
}
