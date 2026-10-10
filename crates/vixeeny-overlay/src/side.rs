// SPDX-License-Identifier: GPL-3.0-or-later
//! The side overlay (plan 5.12): the actions of Vixeeny as a strip of icons on one edge of the
//! screen, drawn natively. The name of the action under the pointer (or the keyboard) shows
//! beside the strip. It slides and fades in, takes the arrow keys, Enter and Escape, and goes
//! away when the user clicks elsewhere, unless it is pinned. Nothing runs while it is still.
//!
//! The pure parts (the entries, the keyboard order, where everything goes, what input does) are
//! plain code the tests drive; [`SidePanel`] puts them on a window.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VIRTUAL_KEY, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    KillTimer, PostMessageW, SetTimer, WA_INACTIVE, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CLOSE,
    WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_SETCURSOR,
    WM_TIMER,
};
use windows::core::Result;

use crate::gfx::{Align, Canvas, Font, Gfx, rect};
use crate::icons;
use crate::popup::{self, Popup};
use crate::scene::{self, Piece};
use crate::theme::{Look, Rgba, Theme};
use crate::window::Pointer;

/// Which edge of the screen the strip sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// `left`, `right`, `top`, `bottom`; anything else is the default, the right edge.
    pub fn from_setting(setting: &str) -> Self {
        match setting {
            "left" => Self::Left,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            _ => Self::Right,
        }
    }

    /// A column (left or right edge) rather than a row.
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// What the user picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Region,
    Window,
    Screen,
    AllMonitors,
    Scrolling,
    RecordToggle,
    ReplayToggle,
    ReplaySave,
    Settings,
}

const HEADER_ID: i32 = 200;

impl Choice {
    const ALL: [Self; 9] = [
        Self::Region,
        Self::Window,
        Self::Screen,
        Self::AllMonitors,
        Self::Scrolling,
        Self::RecordToggle,
        Self::ReplayToggle,
        Self::ReplaySave,
        Self::Settings,
    ];

    const fn id(self) -> i32 {
        self as i32
    }

    fn from_id(id: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }
}

/// Translated texts.
#[derive(Debug, Clone, Default)]
pub struct SideTexts {
    pub image: String,
    pub region: String,
    pub window: String,
    pub screen: String,
    pub all_monitors: String,
    pub scrolling: String,
    pub video: String,
    pub record: String,
    pub stop_recording: String,
    pub replay_start: String,
    pub replay_stop: String,
    pub replay_save: String,
    pub settings: String,
}

/// What the strip needs to know when it opens.
#[derive(Debug, Clone)]
pub struct SideState {
    pub recording: bool,
    pub replay: bool,
    pub dark: bool,
    pub edge: Edge,
    /// `false` when the OS asks for fewer animations: the strip just appears.
    pub animate: bool,
    /// Opens pinned: it stays until closed (it was pinned the last time).
    pub pinned: bool,
}

/// One row of the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: i32,
    pub glyph: i32,
    pub label: String,
    pub enabled: bool,
    pub header: bool,
    pub active: bool,
}

impl Entry {
    fn action(choice: Choice, glyph: i32, label: &str) -> Self {
        Self {
            id: choice.id(),
            glyph,
            label: label.to_owned(),
            enabled: true,
            header: false,
            active: false,
        }
    }

    fn header(n: i32, label: &str) -> Self {
        Self {
            id: HEADER_ID + n,
            glyph: 0,
            label: label.to_uppercase(),
            enabled: false,
            header: true,
            active: false,
        }
    }

    const fn selectable(&self) -> bool {
        self.enabled && !self.header
    }
}

/// The entries, in display order.
pub fn entries(texts: &SideTexts, state: &SideState) -> Vec<Entry> {
    let mut list = vec![
        Entry::header(0, &texts.image),
        Entry::action(Choice::Region, 0, &texts.region),
        Entry::action(Choice::Window, 1, &texts.window),
        Entry::action(Choice::Screen, 2, &texts.screen),
        Entry::action(Choice::AllMonitors, 3, &texts.all_monitors),
        Entry::action(Choice::Scrolling, 4, &texts.scrolling),
        Entry::header(1, &texts.video),
    ];
    let mut record = if state.recording {
        Entry::action(Choice::RecordToggle, 7, &texts.stop_recording)
    } else {
        Entry::action(Choice::RecordToggle, 6, &texts.record)
    };
    record.active = state.recording;
    list.push(record);
    let mut replay = Entry::action(
        Choice::ReplayToggle,
        8,
        if state.replay {
            &texts.replay_stop
        } else {
            &texts.replay_start
        },
    );
    replay.active = state.replay;
    list.push(replay);
    let mut save = Entry::action(Choice::ReplaySave, 9, &texts.replay_save);
    save.enabled = state.replay;
    list.push(save);
    list.push(Entry::action(Choice::Settings, 11, &texts.settings));
    list
}

/// The entry reached by moving `delta` (±1) selectable entries from `current`, wrapping around.
/// `current` is returned when nothing can be selected.
pub fn next_selectable(entries: &[Entry], current: i32, delta: i32) -> i32 {
    let n = entries.len() as i32;
    if n == 0 {
        return current;
    }
    let mut at = current;
    for _ in 0..n {
        at = (at + delta).rem_euclid(n);
        if entries[at as usize].selectable() {
            return at;
        }
    }
    current
}

/// The first entry the keyboard can reach.
pub fn first_selectable(entries: &[Entry]) -> i32 {
    entries
        .iter()
        .position(Entry::selectable)
        .map_or(-1, |i| i as i32)
}

// Layout constants, in logical pixels.
const PAD: f64 = 6.0;
const GAP: f64 = 2.0;
const MARGIN: f64 = 12.0;
const BUTTON: f64 = 40.0;
const SEPARATOR: f64 = 9.0;
const SMALL: f64 = 28.0;
/// Beside the strip, for the name of the action under the pointer.
const TIP_ROOM: f64 = 220.0;
const TIP_HEIGHT: f64 = 40.0;

/// The window's size in logical pixels: the strip (the pin and close buttons, a separator, the
/// entries), the room for the label beside it, and the margin it slides in through.
fn logical_size(entries: &[Entry], vertical: bool) -> (f64, f64) {
    let along: f64 = entries
        .iter()
        .map(|e| if e.header { SEPARATOR } else { BUTTON })
        .sum();
    let along = 2.0 * PAD + SMALL + SEPARATOR + along + GAP * (entries.len() + 1) as f64;
    let across = BUTTON + 2.0 * PAD;
    if vertical {
        (across + TIP_ROOM + 2.0 * MARGIN, along + 2.0 * MARGIN)
    } else {
        (along + 2.0 * MARGIN, across + TIP_HEIGHT + 2.0 * MARGIN)
    }
}

/// Where the window goes on `monitor` (`x, y, width, height`, physical pixels; `dpi` 96 = 100 %):
/// flush against `edge`, centred along it. Returns `x, y, width, height` in physical pixels.
pub fn panel_geometry(
    edge: Edge,
    monitor: (i32, i32, u32, u32),
    dpi: u32,
    entries: &[Entry],
) -> (i32, i32, u32, u32) {
    let scale = f64::from(dpi.max(48)) / 96.0;
    let (mx, my, mw, mh) = monitor;
    let (w, h) = logical_size(entries, edge.is_vertical());
    let w = ((w * scale).round() as u32).min(mw);
    let h = ((h * scale).round() as u32).min(mh);
    let centred_x = mx + (mw - w) as i32 / 2;
    let centred_y = my + (mh - h) as i32 / 2;
    let (x, y) = match edge {
        Edge::Left => (mx, centred_y),
        Edge::Right => (mx + (mw - w) as i32, centred_y),
        Edge::Top => (centred_x, my),
        Edge::Bottom => (centred_x, my + (mh - h) as i32),
    };
    (x, y, w, h)
}

/// Room around the strip's and the label's boxes for their shadows, logical pixels.
const SHADOW: f32 = 24.0;
const TIP_SHADOW: f32 = 12.0;
const ENTER: f64 = 0.22;
const EXIT: f64 = 0.15;
const TIP_MOVE: f64 = 0.1;

/// A part of the strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Pin,
    Close,
    /// The line under the pin and close buttons.
    Divider,
    /// `entries[i]`.
    Entry(usize),
}

/// What is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Pin,
    Close,
    Entry(usize),
    /// The strip, between its buttons.
    Strip,
    /// The rest of the window (the room for the label): a click there is a click elsewhere.
    Outside,
}

/// `(x, y, width, height)`.
pub type Box2 = (f32, f32, f32, f32);

fn inside((x, y, w, h): Box2, px: f32, py: f32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// The parts of the strip, logical pixels from its top-left corner, and its size.
pub fn slots(entries: &[Entry], vertical: bool) -> (Vec<(Slot, Box2)>, (f32, f32)) {
    let (pad, gap, button) = (PAD as f32, GAP as f32, BUTTON as f32);
    let (separator, small) = (SEPARATOR as f32, SMALL as f32);
    // `along` runs down a column (vertical) or across a row; `boxed` turns (along, across) boxes
    // into (x, y) ones.
    let boxed = |along: f32, along_len: f32, across: f32, across_len: f32| {
        if vertical {
            (across, along, across_len, along_len)
        } else {
            (along, across, along_len, across_len)
        }
    };
    let mut out = Vec::with_capacity(entries.len() + 3);
    let mut at = pad;
    out.push((Slot::Pin, boxed(at, small, pad, button / 2.0)));
    out.push((
        Slot::Close,
        boxed(at, small, pad + button / 2.0, button / 2.0),
    ));
    // Pin and close share a row of a column (and a column of a row), one above the other.
    if !vertical {
        out[0].1 = (pad, pad, small, button / 2.0);
        out[1].1 = (pad, pad + button / 2.0, small, button / 2.0);
    }
    at += small + gap;
    out.push((Slot::Divider, boxed(at, separator, pad, button)));
    at += separator + gap;
    for (i, e) in entries.iter().enumerate() {
        let len = if e.header { separator } else { button };
        out.push((Slot::Entry(i), boxed(at, len, pad, button)));
        at += len + gap;
    }
    let along = at - gap + pad;
    let across = button + 2.0 * pad;
    let size = if vertical {
        (across, along)
    } else {
        (along, across)
    };
    (out, size)
}

/// Where everything is in the window, in physical pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct StripLayout {
    pub edge: Edge,
    /// Physical pixels per logical one.
    pub u: f32,
    pub window: (f32, f32),
    /// The strip at rest.
    pub strip: Box2,
    /// The parts, from the strip's top-left corner.
    pub slots: Vec<(Slot, Box2)>,
}

impl StripLayout {
    pub fn new(edge: Edge, entries: &[Entry], dpi: u32, window: (u32, u32)) -> Self {
        let u = dpi.max(48) as f32 / 96.0;
        let (logical, (sw, sh)) = slots(entries, edge.is_vertical());
        let (ww, wh) = (window.0 as f32, window.1 as f32);
        let (sw, sh) = (sw * u, sh * u);
        let m = MARGIN as f32 * u;
        let x = if edge == Edge::Right { ww - m - sw } else { m };
        let y = if edge == Edge::Bottom { wh - m - sh } else { m };
        let slots = logical
            .into_iter()
            .map(|(s, (bx, by, bw, bh))| (s, (bx * u, by * u, bw * u, bh * u)))
            .collect();
        Self {
            edge,
            u,
            window: (ww, wh),
            strip: (x, y, sw, sh),
            slots,
        }
    }

    /// What is at `(x, y)` (window pixels).
    pub fn at(&self, x: f32, y: f32) -> Target {
        let (sx, sy, _, _) = self.strip;
        if !inside(self.strip, x, y) {
            return Target::Outside;
        }
        for (slot, (bx, by, bw, bh)) in &self.slots {
            if inside((sx + bx, sy + by, *bw, *bh), x, y) {
                return match *slot {
                    Slot::Pin => Target::Pin,
                    Slot::Close => Target::Close,
                    Slot::Divider => Target::Strip,
                    Slot::Entry(i) => Target::Entry(i),
                };
            }
        }
        Target::Strip
    }

    fn slot(&self, slot: Slot) -> Option<Box2> {
        self.slots.iter().find(|(s, _)| *s == slot).map(|(_, b)| *b)
    }

    /// Where the label of entry `i`, `size` big, goes: beside the strip, level with the entry,
    /// inside the window.
    pub fn tip_origin(&self, i: usize, (tw, th): (f32, f32)) -> (f32, f32) {
        let u = self.u;
        let (sx, sy, sw, sh) = self.strip;
        let (ww, wh) = self.window;
        let (bx, by, bw, bh) = self.slot(Slot::Entry(i)).unwrap_or_default();
        let clamp = |v: f32, max: f32| v.min(max - 4.0 * u).max(4.0 * u);
        if self.edge.is_vertical() {
            let x = if self.edge == Edge::Right {
                sx - tw - 8.0 * u
            } else {
                sx + sw + 8.0 * u
            };
            (x, clamp(sy + by + bh / 2.0 - th / 2.0, wh - th))
        } else {
            let y = if self.edge == Edge::Bottom {
                sy - th - 8.0 * u
            } else {
                sy + sh + 8.0 * u
            };
            (clamp(sx + bx + bw / 2.0 - tw / 2.0, ww - tw), y)
        }
    }

    /// The strip's rest offset and the one it slides in from (window pixels).
    fn travel(&self) -> ((f32, f32), (f32, f32)) {
        let (x, y, w, h) = self.strip;
        let m = MARGIN as f32 * self.u;
        let out = match self.edge {
            Edge::Left => (x - w - m, y),
            Edge::Right => (x + w + m, y),
            Edge::Top => (x, y - h - m),
            Edge::Bottom => (x, y + h + m),
        };
        ((x, y), out)
    }
}

/// What the strip shows; it is drawn again only when this changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripLook {
    pub selected: i32,
    /// The selected entry is being pressed.
    pub pressed: bool,
    pub pinned: bool,
    pub hover: Option<Target>,
    pub held: Option<Target>,
}

/// Keys the strip answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripKey {
    Escape,
    Enter,
    Previous,
    Next,
}

/// The strip's state and what input does to it (no window: the tests drive it directly).
#[derive(Debug, Clone)]
pub struct Strip {
    pub entries: Vec<Entry>,
    pub layout: StripLayout,
    pub selected: i32,
    /// The label beside the strip shows (the pointer or the keyboard moved).
    pub tip_on: bool,
    pub pinned: bool,
    pub hover: Option<Target>,
    pub press: Option<Target>,
    pub chosen: Option<Choice>,
    /// It goes: an action was picked, or it was dismissed.
    pub closing: bool,
}

impl Strip {
    pub fn new(entries: Vec<Entry>, layout: StripLayout, pinned: bool) -> Self {
        Self {
            selected: first_selectable(&entries),
            entries,
            layout,
            tip_on: false,
            pinned,
            hover: None,
            press: None,
            chosen: None,
            closing: false,
        }
    }

    fn selectable(&self, i: usize) -> bool {
        self.entries.get(i).is_some_and(Entry::selectable)
    }

    pub fn pointer_move(&mut self, x: f32, y: f32) {
        let t = self.layout.at(x, y);
        self.hover = Some(t);
        if let Target::Entry(i) = t
            && self.selectable(i)
        {
            self.selected = i as i32;
            self.tip_on = true;
        }
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) {
        self.pointer_move(x, y);
        self.press = Some(self.layout.at(x, y));
    }

    pub fn pointer_up(&mut self, x: f32, y: f32) {
        let t = self.layout.at(x, y);
        if self.press.take() != Some(t) {
            return;
        }
        match t {
            Target::Entry(i) => self.activate(i),
            Target::Pin => self.pinned = !self.pinned,
            Target::Close => self.close(),
            Target::Outside => self.dismiss(),
            Target::Strip => {}
        }
    }

    pub fn pointer_left(&mut self) {
        self.hover = None;
    }

    pub fn key(&mut self, key: StripKey) {
        match key {
            StripKey::Escape => self.close(),
            StripKey::Enter => {
                if let Ok(i) = usize::try_from(self.selected) {
                    self.activate(i);
                }
            }
            StripKey::Previous | StripKey::Next => {
                let delta = if key == StripKey::Next { 1 } else { -1 };
                self.selected = next_selectable(&self.entries, self.selected, delta);
                self.tip_on = true;
            }
        }
    }

    /// A click elsewhere, or the focus went: the strip goes, unless it is pinned.
    pub fn dismiss(&mut self) {
        if !self.pinned {
            self.close();
        }
    }

    fn close(&mut self) {
        self.closing = true;
    }

    fn activate(&mut self, i: usize) {
        // A disabled entry (a replay save without a replay) does nothing.
        if !self.selectable(i) {
            return;
        }
        if let Some(choice) = self.entries.get(i).and_then(|e| Choice::from_id(e.id)) {
            self.chosen = Some(choice);
            self.close();
        }
    }

    pub fn look(&self) -> StripLook {
        let held = self.press.filter(|p| self.hover == Some(*p));
        StripLook {
            selected: self.selected,
            pressed: held == usize::try_from(self.selected).ok().map(Target::Entry),
            pinned: self.pinned,
            hover: self.hover,
            held,
        }
    }

    /// The label beside the strip, if one shows.
    pub fn tip(&self) -> Option<(usize, &str)> {
        let i = usize::try_from(self.selected).ok()?;
        let e = self.entries.get(i)?;
        (self.tip_on && !e.label.is_empty()).then_some((i, e.label.as_str()))
    }

    pub fn pointer(&self) -> Pointer {
        match self.hover {
            Some(Target::Pin | Target::Close) => Pointer::Hand,
            Some(Target::Entry(i)) if self.selectable(i) => Pointer::Hand,
            _ => Pointer::Arrow,
        }
    }
}

/// The icon of an entry's glyph (see [`entries`]).
fn glyph(kind: i32) -> &'static str {
    match kind {
        0 => icons::SCREENSHOT,
        1 => icons::WINDOW,
        2 => icons::DESKTOP,
        3 => icons::DUAL_SCREEN,
        4 => icons::SCROLL,
        6 => icons::RECORD_FILLED,
        7 => icons::STOP_FILLED,
        8 => icons::HISTORY,
        9 => icons::SAVE,
        _ => icons::SETTINGS,
    }
}

/// Size of the strip's surface: the strip and room for its shadow.
pub fn strip_surface(layout: &StripLayout) -> (u32, u32) {
    let m = 2.0 * SHADOW * layout.u;
    (
        (layout.strip.2 + m).ceil() as u32,
        (layout.strip.3 + m).ceil() as u32,
    )
}

/// The strip, its top-left corner `SHADOW` logical pixels into the surface.
pub fn paint_strip(
    c: &Canvas<'_>,
    t: &Theme,
    layout: &StripLayout,
    entries: &[Entry],
    look: &StripLook,
) -> Result<()> {
    let u = layout.u;
    let o = SHADOW * u;
    let (_, _, sw, sh) = layout.strip;
    let strip = rect(o, o, sw, sh);
    c.shadow(strip, 8.0 * u, 14.0 * u, 6.0 * u, Rgba::hex(0x000000, 0x45))?;
    c.fill_round(strip, 8.0 * u, t.flyout.with_alpha(0xeb as f32 / 255.0))?;
    c.stroke_round(strip, 8.0 * u, 1.0, t.stroke_strong)?;
    let vertical = layout.edge.is_vertical();
    // A hairline across a separator box.
    let divider = |(x, y, w, h): Box2| {
        if vertical {
            c.fill_rect(
                rect(x + 8.0 * u, y + (h - 1.0) / 2.0, w - 16.0 * u, 1.0),
                t.divider,
            )
        } else {
            c.fill_rect(
                rect(x + (w - 1.0) / 2.0, y + 8.0 * u, 1.0, h - 16.0 * u),
                t.divider,
            )
        }
    };
    for (slot, (bx, by, bw, bh)) in &layout.slots {
        let b = (o + bx, o + by, *bw, *bh);
        match *slot {
            Slot::Divider => divider(b)?,
            Slot::Pin | Slot::Close => {
                let target = if *slot == Slot::Pin {
                    Target::Pin
                } else {
                    Target::Close
                };
                let active = *slot == Slot::Pin && look.pinned;
                let fill = if active {
                    Some(t.accent_soft)
                } else if look.held == Some(target) {
                    Some(t.subtle_pressed)
                } else if look.hover == Some(target) {
                    Some(t.subtle_hover)
                } else {
                    None
                };
                if let Some(fill) = fill {
                    c.fill_round(rect(b.0, b.1, b.2, b.3), 6.0 * u, fill)?;
                }
                let icon = match (*slot, look.pinned) {
                    (Slot::Pin, true) => icons::PIN_FILLED,
                    (Slot::Pin, false) => icons::PIN,
                    _ => icons::DISMISS,
                };
                let tint = if active { t.accent } else { t.text_2 };
                let size = 14.0 * u;
                c.icon(
                    icon,
                    b.0 + (b.2 - size) / 2.0,
                    b.1 + (b.3 - size) / 2.0,
                    size,
                    tint,
                )?;
            }
            Slot::Entry(i) => {
                let Some(e) = entries.get(i) else {
                    continue;
                };
                if e.header {
                    divider(b)?;
                    continue;
                }
                if look.selected == i as i32 && e.selectable() {
                    let fill = if look.pressed {
                        t.subtle_pressed
                    } else {
                        t.selected
                    };
                    c.fill_round(rect(b.0, b.1, b.2, b.3), 10.0 * u, fill)?;
                }
                let tint = if e.glyph == 6 || e.glyph == 7 {
                    t.record
                } else if e.active {
                    t.accent
                } else {
                    t.text
                };
                let tint = if e.enabled {
                    tint
                } else {
                    tint.with_alpha(tint.0[3] * 0.38)
                };
                let size = 20.0 * u;
                c.icon(
                    glyph(e.glyph),
                    b.0 + (b.2 - size) / 2.0,
                    b.1 + (b.3 - size) / 2.0,
                    size,
                    tint,
                )?;
                // A dot under the actions that are on (recording, replay buffer).
                if e.active {
                    let dot = if e.glyph == 7 { t.record } else { t.accent };
                    c.fill_circle(b.0 + b.2 / 2.0, b.1 + b.3 - 5.0 * u, 2.0 * u, dot)?;
                }
            }
        }
    }
    Ok(())
}

/// The size of the label's box for `text`.
pub fn tip_bubble(gfx: &Gfx, u: f32, text: &str) -> Result<(f32, f32)> {
    let (w, _) = gfx.measure(text, 13.0 * u, Font::Ui)?;
    Ok(((w + 20.0 * u).ceil(), (30.0 * u).ceil()))
}

fn tip_surface(u: f32, bubble: (f32, f32)) -> (u32, u32) {
    let m = 2.0 * TIP_SHADOW * u;
    ((bubble.0 + m).ceil() as u32, (bubble.1 + m).ceil() as u32)
}

/// The label, its box `TIP_SHADOW` logical pixels into the surface.
pub fn paint_tip(c: &Canvas<'_>, t: &Theme, u: f32, text: &str, bubble: (f32, f32)) -> Result<()> {
    let m = TIP_SHADOW * u;
    let r = rect(m, m, bubble.0, bubble.1);
    c.shadow(r, 6.0 * u, 8.0 * u, 2.0 * u, Rgba::hex(0x000000, 0x35))?;
    c.fill_round(r, 6.0 * u, t.flyout)?;
    c.stroke_round(r, 6.0 * u, 1.0, t.stroke_strong)?;
    c.text(text, r, 13.0 * u, t.text, Align::Centre)
}

const ENTER_TIMER: usize = 1;
const EXIT_TIMER: usize = 2;
/// [`WM_POPUP`] parameter: a click elsewhere.
const DISMISS: usize = 1;

struct Inner {
    gfx: Gfx,
    theme: Theme,
    popup: Popup,
    strip: Piece<StripLook>,
    tip: Piece<String>,
    model: RefCell<Strip>,
    animate: bool,
    /// The slide-in is over: the label may show.
    entered: Cell<bool>,
    exiting: Cell<bool>,
    done: Cell<bool>,
}

impl Inner {
    fn refresh(&self) {
        if let Err(e) = self.update() {
            tracing::warn!("side strip: {e}");
        }
    }

    fn update(&self) -> Result<()> {
        let m = self.model.borrow();
        let look = m.look();
        let gfx = &self.gfx;
        let t = &self.theme;
        self.strip.show(
            gfx,
            &look,
            || Ok(strip_surface(&m.layout)),
            |c| paint_strip(c, t, &m.layout, &m.entries, &look),
        )?;
        let u = m.layout.u;
        let tip = m
            .tip()
            .filter(|_| self.entered.get() && !self.exiting.get());
        match tip {
            Some((i, text)) => {
                let bubble = tip_bubble(gfx, u, text)?;
                let text = text.to_owned();
                self.tip.show(
                    gfx,
                    &text,
                    || Ok(tip_surface(u, bubble)),
                    |c| paint_tip(c, t, u, &text, bubble),
                )?;
                let (x, y) = m.layout.tip_origin(i, bubble);
                let m = TIP_SHADOW * u;
                let (x, y) = ((x - m).round(), (y - m).round());
                let before = self.tip.at.get();
                if self.tip.move_to(x, y)? {
                    self.tip.appear(gfx, 0.0, TIP_MOVE, self.animate)?;
                } else if let Some((x0, y0)) = before
                    && self.animate
                    && (x0, y0) != (x, y)
                {
                    // SAFETY: plain property calls with animations of the same device.
                    unsafe {
                        self.tip
                            .visual
                            .SetOffsetX(&scene::ease_out(gfx, x0, x, TIP_MOVE)?)?;
                        self.tip
                            .visual
                            .SetOffsetY(&scene::ease_out(gfx, y0, y, TIP_MOVE)?)?;
                    }
                }
            }
            None => self.tip.hide()?,
        }
        // SAFETY: plain call.
        unsafe { gfx.dcomp.Commit() }
    }

    /// Slides the strip from `from` to `to` (window pixels, its rest offset and its outside
    /// one), fading it `fade.0` → `fade.1`.
    fn slide(&self, from: (f32, f32), to: (f32, f32), fade: (f32, f32), enter: bool) -> Result<()> {
        let gfx = &self.gfx;
        let o = SHADOW * self.model.borrow().layout.u;
        let v = &self.strip.visual;
        let Some(effect) = &self.strip.effect else {
            return Ok(());
        };
        let ease = |a: f32, b: f32| {
            if enter {
                scene::ease_out(gfx, a, b, ENTER)
            } else {
                scene::ease_in(gfx, a, b, EXIT)
            }
        };
        // SAFETY: plain property calls with animations of the same device.
        unsafe {
            if self.animate {
                v.SetOffsetX(&ease(from.0 - o, to.0 - o)?)?;
                v.SetOffsetY(&ease(from.1 - o, to.1 - o)?)?;
                effect.SetOpacity(&ease(fade.0, fade.1)?)?;
            } else {
                v.SetOffsetX2(to.0 - o)?;
                v.SetOffsetY2(to.1 - o)?;
                effect.SetOpacity2(fade.1)?;
            }
        }
        Ok(())
    }

    /// Puts the strip at `at` (window pixels) with `opacity`, without motion.
    fn park(&self, at: (f32, f32), opacity: f32) -> Result<()> {
        let o = SHADOW * self.model.borrow().layout.u;
        scene::set_offset(&self.strip.visual, at.0 - o, at.1 - o)?;
        if let Some(effect) = &self.strip.effect {
            // SAFETY: plain property call.
            unsafe { effect.SetOpacity2(opacity)? };
        }
        Ok(())
    }

    /// After input: starts the exit once the strip goes, and shows the new state.
    fn after_input(&self) {
        let closing = self.model.borrow().closing;
        if closing && !self.exiting.replace(true) {
            let (rest, out) = self.model.borrow().layout.travel();
            if let Err(e) = self.slide(rest, out, (1.0, 0.0), false) {
                tracing::warn!("side strip: {e}");
            }
            if self.animate {
                // A little longer than the animation, so its last frame is drawn.
                // SAFETY: plain timer call on a window of this thread.
                unsafe {
                    SetTimer(
                        Some(self.popup.hwnd),
                        EXIT_TIMER,
                        (EXIT * 1000.0) as u32 + 30,
                        None,
                    )
                };
            } else {
                self.done.set(true);
            }
        }
        self.refresh();
    }

    fn message(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_TIMER => {
                // SAFETY: plain timer call on a window of this thread.
                let _ = unsafe { KillTimer(Some(self.popup.hwnd), wparam.0) };
                match wparam.0 {
                    ENTER_TIMER => {
                        self.entered.set(true);
                        self.refresh();
                    }
                    EXIT_TIMER => self.done.set(true),
                    _ => {}
                }
                return Some(LRESULT(0));
            }
            WM_SETCURSOR if (lparam.0 & 0xffff) == 1 => {
                // HTCLIENT: the shape follows what is under the pointer.
                popup::set_pointer(self.model.borrow().pointer());
                return Some(LRESULT(1));
            }
            _ => {}
        }
        if self.exiting.get() {
            return None;
        }
        let (x, y) = popup::point(lparam);
        match msg {
            WM_MOUSEMOVE => {
                self.popup.track_leave();
                self.model.borrow_mut().pointer_move(x, y);
                popup::set_pointer(self.model.borrow().pointer());
            }
            popup::WM_MOUSELEAVE => self.model.borrow_mut().pointer_left(),
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                // SAFETY: plain call on a window of this thread.
                unsafe { SetCapture(self.popup.hwnd) };
                self.model.borrow_mut().pointer_down(x, y);
            }
            // Another window took the mouse: its release will not come here.
            WM_CAPTURECHANGED => self.model.borrow_mut().press = None,
            WM_LBUTTONUP => {
                // Before the release, which reports a loss of the mouse.
                self.model.borrow_mut().pointer_up(x, y);
                // SAFETY: plain call.
                let _ = unsafe { ReleaseCapture() };
            }
            WM_KEYDOWN => {
                let shift = key_down(VK_SHIFT);
                let key = match VIRTUAL_KEY(wparam.0 as u16) {
                    VK_ESCAPE => StripKey::Escape,
                    VK_RETURN | VK_SPACE => StripKey::Enter,
                    VK_UP | VK_LEFT => StripKey::Previous,
                    VK_TAB if shift => StripKey::Previous,
                    VK_DOWN | VK_RIGHT | VK_TAB => StripKey::Next,
                    _ => return None,
                };
                self.model.borrow_mut().key(key);
            }
            // The user went to another window.
            WM_ACTIVATE if (wparam.0 & 0xffff) as u32 == WA_INACTIVE => {
                self.model.borrow_mut().dismiss();
                self.after_input();
                return None;
            }
            popup::WM_POPUP if wparam.0 == DISMISS => self.model.borrow_mut().dismiss(),
            WM_CLOSE => self.model.borrow_mut().key(StripKey::Escape),
            _ => return None,
        }
        self.after_input();
        Some(LRESULT(0))
    }
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    // SAFETY: plain call.
    unsafe { GetKeyState(i32::from(vk.0)) < 0 }
}

/// The strip on screen: a window against the edge of a monitor.
pub struct SidePanel {
    inner: Rc<Inner>,
}

impl SidePanel {
    /// The strip for `state`, in a window over `geometry` (physical pixels, see
    /// [`panel_geometry`]) of a monitor at `dpi`.
    pub fn new(
        texts: &SideTexts,
        state: &SideState,
        geometry: (i32, i32, u32, u32),
        dpi: u32,
    ) -> Result<Self> {
        let gfx = Gfx::new()?;
        let theme = Theme::new(Look {
            dark: state.dark,
            animations: state.animate,
            ..Look::default()
        });
        let entries = entries(texts, state);
        let layout = StripLayout::new(state.edge, &entries, dpi, (geometry.2, geometry.3));
        let popup = Popup::new(&gfx, geometry, true, false)?;
        let strip = Piece::new(&gfx, true)?;
        scene::add(&popup.root, &strip.visual)?;
        let tip = Piece::new(&gfx, true)?;
        scene::add(&popup.root, &tip.visual)?;
        let inner = Rc::new(Inner {
            gfx,
            theme,
            popup,
            strip,
            tip,
            model: RefCell::new(Strip::new(entries, layout, state.pinned)),
            animate: state.animate,
            entered: Cell::new(!state.animate),
            exiting: Cell::new(false),
            done: Cell::new(false),
        });
        let weak: Weak<Inner> = Rc::downgrade(&inner);
        inner
            .popup
            .on_message(move |msg, wparam, lparam| weak.upgrade()?.message(msg, wparam, lparam));
        Ok(Self { inner })
    }

    /// The entries, as shown.
    pub fn entries(&self) -> Vec<Entry> {
        self.inner.model.borrow().entries.clone()
    }

    /// The user pinned the strip: it comes back after the action it was closed for.
    pub fn pinned(&self) -> bool {
        self.inner.model.borrow().pinned
    }

    /// What a click elsewhere does: closes the strip (unless it is pinned) on the next turn of
    /// the message loop. Safe to call from a hook.
    pub fn dismisser(&self) -> impl Fn() + 'static {
        let hwnd = self.inner.popup.hwnd.0 as isize;
        move || {
            // SAFETY: plain call; a window that is gone makes it fail, nothing else.
            let _ = unsafe {
                PostMessageW(
                    Some(HWND(hwnd as *mut _)),
                    popup::WM_POPUP,
                    WPARAM(DISMISS),
                    LPARAM(0),
                )
            };
        }
    }

    /// Shows the strip, slides it in, and runs until something is picked, Escape is pressed, or
    /// the user goes elsewhere. `dress` gets the native handle once the window is on screen.
    /// Once the animation is over nothing runs: the loop sleeps until the next input.
    pub fn run_with(&self, dress: impl FnOnce(u64)) -> Result<Option<Choice>> {
        let inner = &self.inner;
        let (rest, out) = inner.model.borrow().layout.travel();
        // Drawn where the slide starts, transparent: it appears with the slide.
        inner.park(out, if inner.animate { 0.0 } else { 1.0 })?;
        inner.update()?;
        inner.popup.show(&inner.gfx)?;
        inner.slide(out, rest, (0.0, 1.0), true)?;
        // SAFETY: plain calls on the device and a window of this thread.
        unsafe {
            inner.gfx.dcomp.Commit()?;
            if inner.animate {
                SetTimer(
                    Some(inner.popup.hwnd),
                    ENTER_TIMER,
                    (ENTER * 1000.0) as u32,
                    None,
                );
            }
        }
        dress(inner.popup.hwnd.0 as u64);
        popup::pump_while(|| !inner.done.get());
        inner.popup.hide();
        Ok(inner.model.borrow().chosen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts() -> SideTexts {
        SideTexts {
            image: "Screenshot".into(),
            region: "Region".into(),
            window: "Window".into(),
            screen: "Screen".into(),
            all_monitors: "All screens".into(),
            scrolling: "Scrolling capture".into(),
            video: "Video".into(),
            record: "Record".into(),
            stop_recording: "Stop recording".into(),
            replay_start: "Start replay buffer".into(),
            replay_stop: "Stop replay buffer".into(),
            replay_save: "Save replay".into(),
            settings: "Settings".into(),
        }
    }

    pub(crate) fn state() -> SideState {
        SideState {
            recording: false,
            replay: false,
            dark: true,
            edge: Edge::Right,
            animate: false,
            pinned: false,
        }
    }

    #[test]
    fn edges_parse_with_the_right_edge_as_default() {
        assert_eq!(Edge::from_setting("left"), Edge::Left);
        assert_eq!(Edge::from_setting("top"), Edge::Top);
        assert_eq!(Edge::from_setting("bottom"), Edge::Bottom);
        assert_eq!(Edge::from_setting("right"), Edge::Right);
        assert_eq!(Edge::from_setting("diagonal"), Edge::Right);
        assert!(Edge::Left.is_vertical() && !Edge::Bottom.is_vertical());
    }

    #[test]
    fn entries_follow_the_state() {
        let mut s = state();
        let list = entries(&texts(), &s);
        let find =
            |list: &[Entry], c: Choice| list.iter().find(|e| e.id == c.id()).unwrap().clone();
        assert_eq!(find(&list, Choice::RecordToggle).label, "Record");
        assert!(
            !find(&list, Choice::ReplaySave).enabled,
            "no replay, nothing to save"
        );
        s.recording = true;
        s.replay = true;
        let list = entries(&texts(), &s);
        let record = find(&list, Choice::RecordToggle);
        assert_eq!(
            (record.label.as_str(), record.active),
            ("Stop recording", true)
        );
        assert_eq!(
            find(&list, Choice::ReplayToggle).label,
            "Stop replay buffer"
        );
        assert!(find(&list, Choice::ReplaySave).enabled);
    }

    #[test]
    fn the_keyboard_skips_titles_and_disabled_entries_and_wraps() {
        let list = entries(&texts(), &state());
        let first = first_selectable(&list);
        assert_eq!(list[first as usize].id, Choice::Region.id());
        // Down from the last entry wraps to the first; up from the first wraps to the last.
        let last = list.len() as i32 - 1;
        assert_eq!(next_selectable(&list, last, 1), first);
        assert_eq!(next_selectable(&list, first, -1), last);
        // Down from the scrolling capture jumps over the "Video" title to Record.
        let scrolling = list
            .iter()
            .position(|e| e.id == Choice::Scrolling.id())
            .unwrap() as i32;
        let record = list
            .iter()
            .position(|e| e.id == Choice::RecordToggle.id())
            .unwrap() as i32;
        assert_eq!(next_selectable(&list, scrolling, 1), record);
        // The replay save is disabled while there is no replay: Replay → Settings.
        let replay = list
            .iter()
            .position(|e| e.id == Choice::ReplayToggle.id())
            .unwrap() as i32;
        let settings = list
            .iter()
            .position(|e| e.id == Choice::Settings.id())
            .unwrap() as i32;
        assert_eq!(next_selectable(&list, replay, 1), settings);
        assert_eq!(next_selectable(&[], 0, 1), 0);
    }

    #[test]
    fn the_window_hugs_its_edge_and_is_centred_along_it() {
        let list = entries(&texts(), &state());
        let monitor = (0, 0, 2560, 1440);
        let (x, y, w, h) = panel_geometry(Edge::Right, monitor, 96, &list);
        assert_eq!(x + w as i32, 2560);
        assert_eq!(y, (1440 - h as i32) / 2);
        assert_eq!(panel_geometry(Edge::Left, monitor, 96, &list).0, 0);
        let (x, y, w, h) = panel_geometry(Edge::Bottom, monitor, 96, &list);
        assert_eq!(y + h as i32, 1440);
        assert_eq!(x, (2560 - w as i32) / 2);
        assert_eq!(panel_geometry(Edge::Top, monitor, 96, &list).1, 0);
        // A second monitor to the right, at 200 %: everything doubles and moves with it.
        let second = (2560, 0, 3840, 2160);
        let (x1, _, w1, h1) = panel_geometry(Edge::Right, second, 192, &list);
        let (_, _, w0, h0) = panel_geometry(Edge::Right, second, 96, &list);
        assert_eq!(x1 + w1 as i32, 2560 + 3840);
        assert_eq!((w1, h1), (w0 * 2, h0 * 2));
        // Never bigger than the monitor.
        let tiny = (0, 0, 300, 400);
        let (_, _, w, h) = panel_geometry(Edge::Right, tiny, 96, &list);
        assert!(w <= 300 && h <= 400);
    }
    fn strip_at_100(edge: Edge, pinned: bool) -> Strip {
        let list = entries(&texts(), &state());
        let (_, _, w, h) = panel_geometry(edge, (0, 0, 2560, 1440), 96, &list);
        let layout = StripLayout::new(edge, &list, 96, (w, h));
        Strip::new(list, layout, pinned)
    }

    fn click(s: &mut Strip, x: f32, y: f32) {
        s.pointer_move(x, y);
        s.pointer_down(x, y);
        s.pointer_up(x, y);
    }

    #[test]
    fn the_strip_fits_its_window_and_its_parts_fit_the_strip() {
        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            let s = strip_at_100(edge, false);
            let (x, y, w, h) = s.layout.strip;
            let (ww, wh) = s.layout.window;
            assert!(
                x >= 0.0 && y >= 0.0 && x + w <= ww && y + h <= wh,
                "{edge:?}"
            );
            // Flush with the margin on its own edge.
            match edge {
                Edge::Right => assert_eq!(x + w, ww - 12.0),
                Edge::Bottom => assert_eq!(y + h, wh - 12.0),
                _ => assert_eq!((x.min(y)), 12.0),
            }
            for (slot, (bx, by, bw, bh)) in &s.layout.slots {
                assert!(*bx >= 0.0 && *by >= 0.0, "{slot:?}");
                assert!(
                    bx + bw <= w + 0.01 && by + bh <= h + 0.01,
                    "{edge:?} {slot:?}"
                );
            }
        }
    }

    #[test]
    fn the_keyboard_picks_and_escape_closes() {
        // Down, Down, Enter: the third entry (Screen), not the titles.
        let mut s = strip_at_100(Edge::Right, false);
        assert_eq!(s.entries[s.selected as usize].id, Choice::Region.id());
        s.key(StripKey::Next);
        s.key(StripKey::Next);
        assert!(s.tip_on, "the keyboard shows the label");
        s.key(StripKey::Enter);
        assert_eq!(s.chosen, Some(Choice::Screen));
        assert!(s.closing);
        // Escape picks nothing, even pinned.
        let mut s = strip_at_100(Edge::Right, true);
        s.key(StripKey::Escape);
        assert_eq!((s.chosen, s.closing), (None, true));
    }

    #[test]
    fn clicks_pick_pin_and_dismiss() {
        // The strip: 12 px from the window's edges, 6 px padding, the pin and close row (28), a
        // separator (9), then the entries (separators of 9, buttons of 40), 2 px apart.
        let mut s = strip_at_100(Edge::Right, false);
        let (ww, _) = s.layout.window;
        let x = ww - 12.0 - 26.0;
        let first = 12.0 + 6.0 + 28.0 + 2.0 + 9.0 + 2.0;
        click(&mut s, x, first + 4.0); // the separator before the screenshots
        assert_eq!((s.chosen, s.closing), (None, false));
        // separator, 5 buttons, separator, each followed by a gap → the record button
        let record_y = first + 9.0 + 2.0 + 5.0 * 42.0 + 9.0 + 2.0 + 20.0;
        click(&mut s, x, record_y);
        assert_eq!(s.chosen, Some(Choice::RecordToggle));

        // The pin keeps it open when the user clicks elsewhere; the close button still closes.
        let mut s = strip_at_100(Edge::Right, false);
        let pin_y = 12.0 + 6.0 + 14.0;
        click(&mut s, ww - 12.0 - 6.0 - 40.0 + 10.0, pin_y);
        assert!(s.pinned);
        click(&mut s, 10.0, 10.0);
        assert!(!s.closing, "pinned: a click elsewhere does nothing");
        s.dismiss();
        assert!(!s.closing, "pinned: losing the focus does nothing");
        click(&mut s, ww - 12.0 - 6.0 - 10.0, pin_y);
        assert_eq!((s.chosen, s.closing), (None, true));
        // Unpinned, a click beside the strip closes it.
        let mut s = strip_at_100(Edge::Right, false);
        click(&mut s, 10.0, 10.0);
        assert!(s.closing);
    }

    #[test]
    fn a_press_that_slides_off_its_button_does_nothing_and_disabled_entries_stay_off() {
        let mut s = strip_at_100(Edge::Right, false);
        let (ww, _) = s.layout.window;
        let x = ww - 12.0 - 26.0;
        let first = 12.0 + 6.0 + 28.0 + 2.0 + 9.0 + 2.0;
        let region_y = first + 9.0 + 2.0 + 20.0;
        s.pointer_down(x, region_y);
        s.pointer_up(x, region_y + 42.0);
        assert_eq!(s.chosen, None);
        // No replay buffer: "Save replay" is disabled, the pointer does not select it.
        let save = s
            .entries
            .iter()
            .position(|e| e.id == Choice::ReplaySave.id())
            .unwrap();
        let (_, (_, by, _, bh)) = s
            .layout
            .slots
            .iter()
            .find(|(slot, _)| *slot == Slot::Entry(save))
            .copied()
            .unwrap();
        let y = s.layout.strip.1 + by + bh / 2.0;
        let before = s.selected;
        click(&mut s, x, y);
        assert_eq!((s.selected, s.chosen), (before, None));
    }

    #[test]
    fn the_label_sits_beside_the_strip_inside_the_window() {
        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            let s = strip_at_100(edge, false);
            let (sx, sy, sw, sh) = s.layout.strip;
            let (ww, wh) = s.layout.window;
            for i in 0..s.entries.len() {
                let (x, y) = s.layout.tip_origin(i, (120.0, 30.0));
                assert!(
                    x >= 0.0 && y >= 0.0 && x + 120.0 <= ww && y + 30.0 <= wh,
                    "{edge:?}"
                );
                let apart = x + 120.0 <= sx || x >= sx + sw || y + 30.0 <= sy || y >= sy + sh;
                assert!(apart, "{edge:?}: the label covers the strip");
            }
        }
    }
}
