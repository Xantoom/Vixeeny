// SPDX-License-Identifier: GPL-3.0-or-later
//! The video editor of the replays, drawn natively: a window of its own that plays a clip and cuts it to the frame.
//!
//! ```text
//! ┌ title: name of the file ─────────────────────────── ✕ ┐
//! │                       the video                       │
//! │ ⏮ ▶ ⏭  0:12.345 / 0:30.000                    🔊 ━━━○ │
//! │ [▮thumbnails, the part kept between its two handles▮] │
//! │ ⚡ 0:08.250                [Save as…]  [   Save   ]    │
//! └───────────────────────────────────────────────────────┘
//! ```
//! The window knows the frames (their times, the key frames), what is kept and where the playhead is; what it cannot do
//! itself (decode a frame, play, write) it asks its host for through [`CutterEvent`]s, on its own thread, between two
//! messages.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use vixeeny_platform::PhysicalRect;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::Graphics::DirectComposition::{IDCompositionSurface, IDCompositionVisual2};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetWindowRect, HTCAPTION, HTCLIENT, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx,
    PM_REMOVE, PeekMessageW, PostMessageW, QS_ALLINPUT, SW_MINIMIZE, ShowWindow, TranslateMessage, WM_CAPTURECHANGED,
    WM_CLOSE, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCHITTEST,
    WM_QUIT, WM_SETCURSOR, WM_SYSKEYDOWN,
};
use windows::core::Result;

use crate::gfx::{Align, Canvas, Gfx, rect};
use crate::icons;
use crate::popup::{self, Popup};
use crate::scene::{self, Piece};
use crate::side::Box2;
use crate::theme::{Look, Rgba, Theme};
use crate::window::Pointer;

// Logical pixels.
const TITLE_H: f32 = 40.0;
const CONTROLS_H: f32 = 48.0;
const TIMELINE_H: f32 = 70.0;
const ACTIONS_H: f32 = 60.0;
/// The bands under the video.
const BOTTOM_H: f32 = CONTROLS_H + TIMELINE_H + ACTIONS_H;
const PAD: f32 = 16.0;
const CLOSE_W: f32 = 46.0;
const STRIP_Y: f32 = CONTROLS_H + 8.0;
const STRIP_H: f32 = 46.0;
const HANDLE_W: f32 = 12.0;
const VOLUME_W: f32 = 110.0;
const SAVE_W: f32 = 130.0;
const SAVE_AS_W: f32 = 170.0;
const BUTTON_H: f32 = 36.0;
/// How near a key frame the start of the part is pulled onto it (Shift: never).
const SNAP: f32 = 8.0;

/// The frames of the clip, as the window needs them.
#[derive(Debug, Clone, PartialEq)]
pub struct Frames {
    /// When each frame is shown, seconds from the first.
    pub times: Vec<f64>,
    /// The key frames, in order.
    pub keys: Vec<usize>,
    /// Frames per second.
    pub rate: f64,
    /// Width over height of the picture.
    pub aspect: f32,
}

impl Frames {
    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    pub fn duration(&self) -> f64 {
        self.times.last().map_or(0.0, |t| t + 1.0 / self.rate.max(1.0))
    }

    /// The frame shown at `t`.
    pub fn at(&self, t: f64) -> usize {
        self.times.partition_point(|&x| x <= t + 1e-9).saturating_sub(1)
    }

    fn time(&self, i: usize) -> f64 {
        self.times.get(i).copied().unwrap_or(0.0)
    }

    /// The key frame at or before `i`.
    pub fn key_before(&self, i: usize) -> usize {
        let n = self.keys.partition_point(|&k| k <= i);
        n.checked_sub(1).map_or(0, |n| self.keys[n])
    }
}

/// What the window says, in the user's language.
#[derive(Debug, Clone, Default)]
pub struct CutterTexts {
    pub title: String,
    pub save: String,
    pub save_as: String,
    pub cancel: String,
    /// Behind the sign of a part written without encoding again.
    pub lossless: String,
    /// Behind the sign of a part encoded again.
    pub reencode: String,
    pub play: String,
    pub pause: String,
    pub previous: String,
    pub next: String,
    pub mute: String,
    pub close: String,
}

/// What the window asks its host for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CutterEvent {
    /// Show this frame (the playhead was moved).
    Seek(usize),
    Play,
    Pause,
    /// What is kept changed: frames `from..=to`.
    Range(usize, usize),
    /// The preview's volume (0 to 1) and whether it is muted.
    Volume(f32, bool),
    Save,
    SaveAs,
    /// Stop the writing in progress.
    Cancel,
    Close,
}

/// Where the pointer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Title,
    Close,
    Video,
    Previous,
    Play,
    Next,
    Mute,
    Volume,
    Strip,
    From,
    To,
    Sign,
    SaveAs,
    Save,
    None,
}

/// The parts of the window, logical pixels: `w`×`h` the whole window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub w: f32,
    pub h: f32,
    /// The band of the video (between the title and the controls).
    pub band: Box2,
    /// Where the picture goes in it, its proportions kept.
    pub video: Box2,
    /// The bottom bands' top.
    pub bottom: f32,
}

impl Layout {
    pub fn new(w: f32, h: f32, aspect: f32) -> Self {
        let band_h = (h - TITLE_H - BOTTOM_H).max(1.0);
        let band = (0.0, TITLE_H, w, band_h);
        let aspect = if aspect > 0.0 { aspect } else { 16.0 / 9.0 };
        let (vw, vh) = if w / band_h > aspect { (band_h * aspect, band_h) } else { (w, w / aspect) };
        Self { w, h, band, video: ((w - vw) / 2.0, TITLE_H + (band_h - vh) / 2.0, vw, vh), bottom: TITLE_H + band_h }
    }

    /// A part of the bottom bands, in window coordinates.
    fn part(&self, target: Target) -> Box2 {
        let b = self.bottom;
        let actions = b + CONTROLS_H + TIMELINE_H;
        let button_y = actions + (ACTIONS_H - BUTTON_H) / 2.0;
        match target {
            Target::Title => (0.0, 0.0, self.w - CLOSE_W, TITLE_H),
            Target::Close => (self.w - CLOSE_W, 0.0, CLOSE_W, TITLE_H),
            Target::Video => self.band,
            Target::Previous => (PAD, b + 8.0, 32.0, 32.0),
            Target::Play => (PAD + 38.0, b + 4.0, 40.0, 40.0),
            Target::Next => (PAD + 84.0, b + 8.0, 32.0, 32.0),
            Target::Mute => (self.w - PAD - VOLUME_W - 40.0, b + 8.0, 32.0, 32.0),
            Target::Volume => (self.w - PAD - VOLUME_W, b + 14.0, VOLUME_W, 20.0),
            Target::Strip | Target::From | Target::To => {
                (PAD + HANDLE_W, b + STRIP_Y, self.w - 2.0 * (PAD + HANDLE_W), STRIP_H)
            }
            Target::Sign => (PAD, actions + 18.0, 24.0, 24.0),
            Target::Save => (self.w - PAD - SAVE_W, button_y, SAVE_W, BUTTON_H),
            Target::SaveAs => (self.w - PAD - SAVE_W - 8.0 - SAVE_AS_W, button_y, SAVE_AS_W, BUTTON_H),
            Target::None => (0.0, 0.0, 0.0, 0.0),
        }
    }

    /// The x of time `t` on the strip.
    fn x_of(&self, frames: &Frames, t: f64) -> f32 {
        let (x, _, w, _) = self.part(Target::Strip);
        x + (t / frames.duration().max(1e-6)) as f32 * w
    }

    /// The time at `x` on the strip.
    fn t_of(&self, frames: &Frames, x: f32) -> f64 {
        let (sx, _, w, _) = self.part(Target::Strip);
        f64::from(((x - sx) / w.max(1.0)).clamp(0.0, 1.0)) * frames.duration()
    }

    /// The handles of the part kept: before its first frame and after its last.
    fn handle(&self, frames: &Frames, state: &CutState, target: Target) -> Box2 {
        let (_, y, _, h) = self.part(Target::Strip);
        match target {
            Target::From => (self.x_of(frames, frames.time(state.from)) - HANDLE_W, y - 4.0, HANDLE_W, h + 8.0),
            _ => (self.x_of(frames, frames.time(state.to) + 1.0 / frames.rate.max(1.0)), y - 4.0, HANDLE_W, h + 8.0),
        }
    }

    /// What is at `(x, y)` (logical pixels).
    pub fn target_at(&self, frames: &Frames, state: &CutState, x: f32, y: f32) -> Target {
        let inside = |(bx, by, bw, bh): Box2| x >= bx && x < bx + bw && y >= by && y < by + bh;
        let grow = |(bx, by, bw, bh): Box2| (bx - 4.0, by, bw + 8.0, bh);
        for target in [Target::From, Target::To] {
            if inside(grow(self.handle(frames, state, target))) {
                return target;
            }
        }
        let (sx, sy, sw, sh) = self.part(Target::Strip);
        if inside((sx - HANDLE_W, sy - 6.0, sw + 2.0 * HANDLE_W, sh + 18.0)) {
            return Target::Strip;
        }
        [
            Target::Close,
            Target::Title,
            Target::Previous,
            Target::Play,
            Target::Next,
            Target::Mute,
            Target::Volume,
            Target::Sign,
            Target::SaveAs,
            Target::Save,
            Target::Video,
        ]
        .into_iter()
        .find(|t| inside(self.part(*t)))
        .unwrap_or(Target::None)
    }
}

/// The window's place: centred on `work` (the monitor's free area, physical pixels), as tall as it leaves with a margin
/// above and below, as wide as the picture needs.
pub fn placement(work: PhysicalRect, u: f32, aspect: f32) -> PhysicalRect {
    let (w, h) = (work.width as f32, work.height as f32);
    let margin_y = (h * 0.06).max(24.0 * u);
    let margin_x = (w * 0.04).max(24.0 * u);
    let height = h - 2.0 * margin_y;
    let video_h = height - (TITLE_H + BOTTOM_H) * u;
    let aspect = if aspect > 0.0 { aspect } else { 16.0 / 9.0 };
    let min_w = 760.0 * u;
    let width = (video_h * aspect).clamp(min_w.min(w - 2.0 * margin_x), w - 2.0 * margin_x);
    let (width, height) = (width.round() as u32, height.round() as u32);
    PhysicalRect::new(
        work.x + ((w - width as f32) / 2.0).round() as i32,
        work.y + ((h - height as f32) / 2.0).round() as i32,
        width,
        height,
    )
}

/// What is kept, where the playhead is, and the rest of what the user set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutState {
    pub position: usize,
    /// First and last frame kept.
    pub from: usize,
    pub to: usize,
    pub playing: bool,
    pub volume: f32,
    pub muted: bool,
    /// The share written, while the part is written.
    pub progress: Option<f32>,
    /// The part starts on a key frame: it is written without encoding again.
    pub lossless: bool,
}

/// `m:ss.mmm` (or `h:mm:ss.mmm`).
pub fn format_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    if h > 0 { format!("{h}:{m:02}:{s:02}.{ms:03}") } else { format!("{m}:{s:02}.{ms:03}") }
}

/// What the bottom bands show; drawn again only when it changes.
#[derive(Debug, Clone, PartialEq)]
struct BottomLook {
    state: CutState,
    hover: Target,
    held: Option<Target>,
    thumbs: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct TitleLook {
    hover: Target,
    held: Option<Target>,
}

/// A press being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    Playhead,
    From,
    To,
    Volume,
}

/// What the host does for the window.
pub trait CutterHost {
    fn event(&mut self, cutter: &Cutter, event: CutterEvent);
    /// Called on every turn of the loop; returns how long the loop may sleep (`None`: until a message comes).
    fn tick(&mut self, cutter: &Cutter) -> Option<Duration>;
}

/// Wakes the window's loop from another thread (something is ready for the host).
#[derive(Clone)]
pub struct Waker(isize);

impl Waker {
    pub fn wake(&self) {
        // SAFETY: plain call; a window that is gone makes it fail, nothing else.
        let _ = unsafe { PostMessageW(Some(HWND(self.0 as *mut _)), popup::WM_POPUP, WPARAM(0), LPARAM(0)) };
    }
}

/// A tooltip's surface size and its bubble's (see [`crate::paint::tip_size`]).
type TipSize = ((u32, u32), (f32, f32));

struct Inner {
    gfx: std::rc::Rc<Gfx>,
    theme: Theme,
    u: f32,
    popup: Popup,
    layout: Layout,
    frames: Frames,
    texts: CutterTexts,
    title: Piece<TitleLook>,
    bottom: Piece<BottomLook>,
    tip: Piece<String>,
    /// The size of the tooltip last measured, for its text.
    tip_size: RefCell<Option<(String, TipSize)>>,
    /// The time, a layer of its own (the frame it shows).
    clock: Piece<usize>,
    playhead: Piece<()>,
    video: IDCompositionVisual2,
    video_surface: RefCell<Option<(IDCompositionSurface, (u32, u32))>>,
    frame: RefCell<Option<(ID2D1Bitmap1, (u32, u32))>>,
    thumbs: RefCell<Vec<Option<ID2D1Bitmap1>>>,
    state: Cell<CutState>,
    hover: Cell<Target>,
    press: Cell<Option<Target>>,
    drag: Cell<Option<Drag>>,
    events: RefCell<Vec<CutterEvent>>,
    closed: Cell<bool>,
}

fn shift() -> bool {
    // SAFETY: plain call.
    unsafe { GetKeyState(i32::from(VK_SHIFT.0)) < 0 }
}

fn control() -> bool {
    // SAFETY: plain call.
    unsafe { GetKeyState(i32::from(VK_CONTROL.0)) < 0 }
}

impl Inner {
    fn emit(&self, event: CutterEvent) {
        self.events.borrow_mut().push(event);
    }

    fn set_state(&self, f: impl FnOnce(&mut CutState)) {
        let mut s = self.state.get();
        let before = s;
        f(&mut s);
        let last = self.frames.len().saturating_sub(1);
        s.position = s.position.min(last);
        s.to = s.to.min(last);
        s.from = s.from.min(s.to);
        self.state.set(s);
        if (s.from, s.to) != (before.from, before.to) {
            self.emit(CutterEvent::Range(s.from, s.to));
        }
        if s.position != before.position {
            self.emit(CutterEvent::Seek(s.position));
        }
        if (s.volume, s.muted) != (before.volume, before.muted) {
            self.emit(CutterEvent::Volume(s.volume, s.muted));
        }
    }

    /// Moves the playhead by the user: playing stops.
    fn seek(&self, frame: usize) {
        if self.state.get().playing {
            self.state.set(CutState { playing: false, ..self.state.get() });
            self.emit(CutterEvent::Pause);
        }
        self.set_state(|s| s.position = frame);
    }

    fn step(&self, frames: i64) {
        let p = self.state.get().position as i64 + frames;
        self.seek(p.clamp(0, self.frames.len() as i64 - 1) as usize);
    }

    fn toggle_play(&self) {
        let mut s = self.state.get();
        s.playing = !s.playing;
        // At the end, playing starts again from the beginning of the part.
        if s.playing && s.position >= s.to {
            s.position = s.from;
            self.state.set(s);
            self.emit(CutterEvent::Seek(s.from));
        } else {
            self.state.set(s);
        }
        self.emit(if s.playing { CutterEvent::Play } else { CutterEvent::Pause });
    }

    fn busy(&self) -> bool {
        self.state.get().progress.is_some()
    }

    fn act(&self, target: Target) {
        match target {
            Target::Close => self.emit(CutterEvent::Close),
            Target::Play | Target::Video => self.toggle_play(),
            Target::Previous => self.step(-1),
            Target::Next => self.step(1),
            Target::Mute => self.set_state(|s| s.muted = !s.muted),
            Target::Save if !self.busy() => self.emit(CutterEvent::Save),
            Target::SaveAs if self.busy() => self.emit(CutterEvent::Cancel),
            Target::SaveAs => self.emit(CutterEvent::SaveAs),
            _ => {}
        }
    }

    /// Follows a drag to `x` (logical pixels).
    fn drag_to(&self, drag: Drag, x: f32) {
        let l = &self.layout;
        let f = &self.frames;
        match drag {
            Drag::Playhead => self.seek(f.at(l.t_of(f, x))),
            Drag::From => {
                let mut frame = f.at(l.t_of(f, x));
                if !shift() {
                    let key = f
                        .keys
                        .iter()
                        .copied()
                        .min_by(|a, b| {
                            let da = (l.x_of(f, f.time(*a)) - x).abs();
                            let db = (l.x_of(f, f.time(*b)) - x).abs();
                            da.total_cmp(&db)
                        })
                        .filter(|k| (l.x_of(f, f.time(*k)) - x).abs() <= SNAP);
                    frame = key.unwrap_or(frame);
                }
                self.set_state(|s| s.from = frame.min(s.to));
                self.seek(self.state.get().from);
            }
            Drag::To => {
                // The handle is after the last frame: the frame before it is the last kept.
                let t = l.t_of(f, x) - 1.0 / f.rate.max(1.0) / 2.0;
                let frame = f.at(t.max(0.0));
                self.set_state(|s| s.to = frame.max(s.from));
                self.seek(self.state.get().to);
            }
            Drag::Volume => {
                let (vx, _, vw, _) = l.part(Target::Volume);
                let v = ((x - vx) / vw).clamp(0.0, 1.0);
                self.set_state(|s| {
                    s.volume = v;
                    s.muted = v == 0.0;
                });
            }
        }
    }

    fn key(&self, vk: u16) -> bool {
        let s = self.state.get();
        let second = self.frames.rate.round().max(1.0) as i64;
        match vk {
            // Space
            0x20 => self.toggle_play(),
            // Left, Right
            0x25 => self.step(if shift() { -second } else { -1 }),
            0x27 => self.step(if shift() { second } else { 1 }),
            // Home, End: the start and the end of the part.
            0x24 => self.seek(s.from),
            0x23 => self.seek(s.to),
            // I, O: the part starts or ends at the playhead.
            0x49 => self.set_state(|s| s.from = s.position.min(s.to)),
            0x4f => self.set_state(|s| s.to = s.position.max(s.from)),
            // M: mute.
            0x4d => self.set_state(|s| s.muted = !s.muted),
            // Ctrl+S, Ctrl+Shift+S
            0x53 if control() && !self.busy() => {
                self.emit(if shift() { CutterEvent::SaveAs } else { CutterEvent::Save })
            }
            // Escape: stops the writing, else closes.
            0x1b => self.emit(if self.busy() { CutterEvent::Cancel } else { CutterEvent::Close }),
            _ => return false,
        }
        true
    }

    fn tip_text(&self, target: Target) -> Option<&str> {
        let t = &self.texts;
        Some(match target {
            Target::Close => &t.close,
            Target::Previous => &t.previous,
            Target::Next => &t.next,
            Target::Play if self.state.get().playing => &t.pause,
            Target::Play => &t.play,
            Target::Mute => &t.mute,
            Target::Sign if self.state.get().lossless => &t.lossless,
            Target::Sign => &t.reencode,
            _ => return None,
        })
    }

    fn message(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let u = self.u;
        let (px, py) = popup::point(lparam);
        let (x, y) = (px / u, py / u);
        match msg {
            WM_NCHITTEST => {
                let mut r = RECT::default();
                // SAFETY: a window of this thread; a valid out-pointer.
                let _ = unsafe { GetWindowRect(self.popup.hwnd, &mut r) };
                let (lx, ly) = ((px - r.left as f32) / u, (py - r.top as f32) / u);
                let state = self.state.get();
                let hit = self.layout.target_at(&self.frames, &state, lx, ly);
                return Some(LRESULT(if hit == Target::Title { HTCAPTION as isize } else { HTCLIENT as isize }));
            }
            WM_CLOSE => {
                self.emit(CutterEvent::Close);
                return Some(LRESULT(0));
            }
            WM_SETCURSOR if (lparam.0 & 0xffff) == 1 => {
                let pointer = match self.hover.get() {
                    Target::From | Target::To => Pointer::Zone(vixeeny_editor::CursorHint::ResizeEw),
                    Target::None | Target::Title | Target::Sign => Pointer::Arrow,
                    _ => Pointer::Hand,
                };
                popup::set_pointer(pointer);
                return Some(LRESULT(1));
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if !self.key(wparam.0 as u16) {
                    return None;
                }
            }
            WM_MOUSEMOVE => {
                self.popup.track_leave();
                match self.drag.get() {
                    Some(drag) => self.drag_to(drag, x),
                    None => {
                        let state = self.state.get();
                        self.hover.set(self.layout.target_at(&self.frames, &state, x, y));
                    }
                }
            }
            popup::WM_MOUSELEAVE => {
                if self.drag.get().is_none() {
                    self.hover.set(Target::None);
                }
            }
            WM_MOUSEWHEEL => {
                let delta = (wparam.0 >> 16) as u16 as i16;
                let second = self.frames.rate.round().max(1.0) as i64;
                let step = if shift() { second } else { 1 };
                self.step(if delta > 0 { -step } else { step });
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let state = self.state.get();
                let at = self.layout.target_at(&self.frames, &state, x, y);
                self.press.set(Some(at));
                let drag = match at {
                    Target::Strip => Some(Drag::Playhead),
                    Target::From if !self.busy() => Some(Drag::From),
                    Target::To if !self.busy() => Some(Drag::To),
                    Target::Volume => Some(Drag::Volume),
                    _ => None,
                };
                if let Some(drag) = drag {
                    // SAFETY: plain call on a window of this thread.
                    unsafe { SetCapture(self.popup.hwnd) };
                    self.drag.set(Some(drag));
                    self.drag_to(drag, x);
                }
            }
            WM_CAPTURECHANGED => {
                self.drag.set(None);
            }
            WM_LBUTTONUP => {
                let pressed = self.press.take();
                if self.drag.take().is_some() {
                    // SAFETY: plain call.
                    let _ = unsafe { ReleaseCapture() };
                } else {
                    let state = self.state.get();
                    let at = self.layout.target_at(&self.frames, &state, x, y);
                    if pressed == Some(at) {
                        self.act(at);
                    }
                }
            }
            popup::WM_POPUP => {}
            _ => return None,
        }
        self.refresh();
        Some(LRESULT(0))
    }

    fn refresh(&self) {
        if let Err(e) = self.draw() {
            tracing::warn!("video editor: {e}");
        }
    }

    fn draw(&self) -> Result<()> {
        let u = self.u;
        let layout = self.layout;
        let hover = self.hover.get();
        let held = self.press.get().filter(|p| *p == hover);
        let w = (layout.w * u).round() as u32;
        let title = TitleLook { hover, held };
        self.title.show(
            &self.gfx,
            &title,
            || Ok((w, (TITLE_H * u).ceil() as u32)),
            |c| paint_title(c, &self.theme, u, &layout, &self.texts.title, &title),
        )?;
        let state = self.state.get();
        // The playhead and the time are layers of their own: playing moves the one and draws the other again, not the
        // whole bands.
        let look = BottomLook {
            state: CutState { position: 0, ..state },
            hover,
            held,
            thumbs: self.thumbs.borrow().iter().filter(|t| t.is_some()).count(),
        };
        let thumbs = self.thumbs.borrow();
        self.bottom.show(
            &self.gfx,
            &look,
            || Ok((w, (BOTTOM_H * u).ceil() as u32)),
            |c| paint_bottom(c, &self.theme, u, &layout, &self.frames, &self.texts, &look, &thumbs),
        )?;
        drop(thumbs);
        self.clock.show(
            &self.gfx,
            &state.position,
            || Ok(clock_size(u)),
            |c| paint_clock(c, &self.theme, u, &self.frames, state.position, (0.0, 0.0)),
        )?;
        let (cx, cy) = clock_at(&layout);
        self.clock.move_to((cx * u).round(), (cy * u).round())?;
        self.playhead.show(
            &self.gfx,
            &(),
            || Ok(playhead_size(u)),
            |c| paint_playhead(c, &self.theme, u, (0.0, 0.0)),
        )?;
        let (px, py) = playhead_at(&layout, &self.frames, state.position);
        self.playhead.move_to((px * u).round(), (py * u).round())?;
        // The tooltip of what is under the pointer, above it.
        match self.tip_text(hover).filter(|_| self.drag.get().is_none()) {
            Some(text) => {
                let text = text.to_owned();
                // Measured once per text, not on every move of the pointer.
                let cached = self.tip_size.borrow().as_ref().filter(|(t, _)| *t == text).map(|(_, m)| *m);
                let (size, bubble) = match cached {
                    Some(m) => m,
                    None => {
                        let m = crate::paint::tip_size(&self.gfx, u, &text)?;
                        *self.tip_size.borrow_mut() = Some((text.clone(), m));
                        m
                    }
                };
                self.tip.show(
                    &self.gfx,
                    &text,
                    || Ok(size),
                    |c| crate::paint::tip(c, &self.theme, u, &text, bubble),
                )?;
                let (bx, by, bw, _) = layout.part(hover);
                let m = crate::paint::TIP_MARGIN * u;
                let tx =
                    ((bx + bw / 2.0) * u - size.0 as f32 / 2.0).clamp(0.0, (layout.w * u - size.0 as f32).max(0.0));
                let ty = by * u - bubble.1 - m - 4.0 * u;
                self.tip.move_to(tx.round(), ty.round())?;
            }
            None => self.tip.hide()?,
        }
        // SAFETY: plain call.
        unsafe { self.gfx.dcomp.Commit() }
    }

    /// Draws `bgra` (`w`×`h`) in the video band.
    fn show_frame(&self, w: u32, h: u32, bgra: &[u8]) -> Result<()> {
        let u = self.u;
        let (bx, by, bw, bh) = self.layout.band;
        let (sw, sh) = ((bw * u).round() as u32, (bh * u).round() as u32);
        let mut slot = self.video_surface.borrow_mut();
        if slot.as_ref().is_none_or(|(_, s)| *s != (sw, sh)) {
            let surface = self.gfx.opaque_surface(sw, sh)?;
            // SAFETY: plain property call.
            unsafe { self.video.SetContent(&surface)? };
            scene::set_offset(&self.video, (bx * u).round(), (by * u).round())?;
            *slot = Some((surface, (sw, sh)));
        }
        let mut frame = self.frame.borrow_mut();
        let reuse = frame.as_ref().is_some_and(|(_, s)| *s == (w, h));
        if reuse {
            if let Some((bitmap, _)) = frame.as_ref() {
                Gfx::update_bgra(bitmap, w, h, bgra)?;
            }
        } else {
            *frame = Some((self.gfx.bgra_bitmap(w, h, bgra)?, (w, h)));
        }
        let (Some((surface, _)), Some((bitmap, _))) = (slot.as_ref(), frame.as_ref()) else {
            return Ok(());
        };
        let (vx, vy, vw, vh) = self.layout.video;
        let target = rect((vx - bx) * u, (vy - by) * u, vw * u, vh * u);
        let deep = self.theme.bg_deep;
        self.gfx.draw(surface, sw, sh, |c| {
            c.fill_rect(rect(0.0, 0.0, sw as f32, sh as f32), deep)?;
            c.picture(bitmap, target)
        })?;
        // SAFETY: plain call.
        unsafe { self.gfx.dcomp.Commit() }
    }
}

fn paint_title(c: &Canvas<'_>, t: &Theme, u: f32, l: &Layout, name: &str, look: &TitleLook) -> Result<()> {
    c.fill_rect(rect(0.0, 0.0, l.w * u, TITLE_H * u), t.flyout)?;
    let icon = 18.0 * u;
    c.icon(icons::CUT, 14.0 * u, (TITLE_H * u - icon) / 2.0, icon, t.accent)?;
    let text = c.gfx.paragraph_lines(name, 13.0 * u, false, (l.w - 140.0) * u, 1)?;
    c.text_layout(&text.layout, 42.0 * u, (TITLE_H * u - text.height) / 2.0, t.text)?;
    let (x, y, w, h) = l.part(Target::Close);
    let (back, fore) = match (look.held, look.hover) {
        (Some(Target::Close), _) => (Some(Rgba::hex(0xc42b1c, 0xcc)), Rgba::hex(0xffffff, 255)),
        (_, Target::Close) => (Some(Rgba::hex(0xc42b1c, 255)), Rgba::hex(0xffffff, 255)),
        _ => (None, t.text),
    };
    if let Some(back) = back {
        c.fill_rect(rect(x * u, y * u, w * u, h * u), back)?;
    }
    let icon = 16.0 * u;
    c.icon(icons::DISMISS, (x + w / 2.0) * u - icon / 2.0, (y + h / 2.0) * u - icon / 2.0, icon, fore)
}

/// The size of the time's surface (physical pixels): room for `h:mm:ss.mmm / h:mm:ss.mmm`.
fn clock_size(u: f32) -> (u32, u32) {
    ((300.0 * u).ceil() as u32, (CONTROLS_H * u).ceil() as u32)
}

/// Where the time is drawn, window logical pixels.
fn clock_at(l: &Layout) -> (f32, f32) {
    (PAD + 128.0, l.bottom)
}

/// The time of the playhead and the length of the clip, at `(x, y)` (pixels of the canvas): a layer of its own, drawn
/// again on every frame played (the rest of the bands is not).
fn paint_clock(c: &Canvas<'_>, t: &Theme, u: f32, frames: &Frames, position: usize, (x, y): (f32, f32)) -> Result<()> {
    let now = format_time(frames.time(position));
    let total = format_time(frames.duration());
    let time = c.gfx.paragraph(&now, 14.0 * u, true, 1.0e5)?;
    let rest = c.gfx.paragraph(&format!(" / {total}"), 14.0 * u, false, 1.0e5)?;
    let ty = y + (CONTROLS_H * u - time.height) / 2.0;
    c.text_layout(&time.layout, x, ty, t.text)?;
    c.text_layout(&rest.layout, x + time.width, ty, t.text_2)
}

/// The playhead's surface (physical pixels): its line over the strip and its knob above.
fn playhead_size(u: f32) -> (u32, u32) {
    ((PLAYHEAD_W * u).ceil() as u32, ((STRIP_H + 17.0) * u).ceil() as u32)
}

const PLAYHEAD_W: f32 = 12.0;

/// Where the playhead's surface goes for frame `position`, window logical pixels.
fn playhead_at(l: &Layout, frames: &Frames, position: usize) -> (f32, f32) {
    let x = l.x_of(frames, frames.time(position));
    (x - PLAYHEAD_W / 2.0, l.bottom + STRIP_Y - 11.0)
}

/// The playhead, its surface's top left at `(x, y)` (pixels of the canvas): drawn once, then only moved.
fn paint_playhead(c: &Canvas<'_>, t: &Theme, u: f32, (x, y): (f32, f32)) -> Result<()> {
    let cx = x + PLAYHEAD_W / 2.0 * u;
    c.fill_rect(rect(cx - 1.0 * u, y + 5.0 * u, 2.0 * u, (STRIP_H + 12.0) * u), t.text)?;
    c.fill_circle(cx, y + 5.0 * u, 5.0 * u, t.text)
}

#[expect(clippy::too_many_arguments, reason = "the painting state of one frame, read once")]
fn paint_bottom(
    c: &Canvas<'_>,
    t: &Theme,
    u: f32,
    l: &Layout,
    frames: &Frames,
    texts: &CutterTexts,
    look: &BottomLook,
    thumbs: &[Option<ID2D1Bitmap1>],
) -> Result<()> {
    let s = look.state;
    let top = l.bottom;
    // Window logical box → this surface's pixels.
    let at = |(x, y, w, h): Box2| rect(x * u, (y - top) * u, w * u, h * u);
    c.fill_rect(rect(0.0, 0.0, l.w * u, BOTTOM_H * u), t.flyout)?;
    c.fill_rect(rect(0.0, 0.0, l.w * u, 1.0), t.divider)?;
    let back = |target: Target, round: f32| -> Result<()> {
        let fill = if look.held == Some(target) {
            t.subtle_pressed
        } else if look.hover == target {
            t.subtle_hover
        } else {
            return Ok(());
        };
        c.fill_round(at(l.part(target)), round * u, fill)
    };
    let icon_in = |data: &'static str, target: Target, size: f32, colour: Rgba| -> Result<()> {
        let (x, y, w, h) = l.part(target);
        let i = size * u;
        c.icon(data, (x + w / 2.0) * u - i / 2.0, (y + h / 2.0 - top) * u - i / 2.0, i, colour)
    };

    // ---- controls: frame by frame, play, the time; the volume on the right
    back(Target::Previous, 6.0)?;
    icon_in(icons::PREVIOUS, Target::Previous, 18.0, t.text)?;
    back(Target::Next, 6.0)?;
    icon_in(icons::NEXT, Target::Next, 18.0, t.text)?;
    let play = at(l.part(Target::Play));
    let fill = if look.hover == Target::Play { t.accent.brighter(0.08) } else { t.accent };
    c.fill_circle(
        f32::midpoint(play.left, play.right),
        f32::midpoint(play.top, play.bottom),
        (play.right - play.left) / 2.0,
        fill,
    )?;
    icon_in(
        if s.playing { icons::PAUSE_FILLED } else { icons::PLAY_FILLED },
        Target::Play,
        18.0,
        Rgba::hex(0xffffff, 255),
    )?;

    back(Target::Mute, 6.0)?;
    let silent = s.muted || s.volume == 0.0;
    icon_in(
        if silent { icons::SPEAKER_MUTE } else { icons::SPEAKER },
        Target::Mute,
        18.0,
        if silent { t.text_2 } else { t.text },
    )?;
    let track = at(l.part(Target::Volume));
    let cy = f32::midpoint(track.top, track.bottom);
    let level = if s.muted { 0.0 } else { s.volume };
    let kx = track.left + (track.right - track.left) * level;
    c.fill_round(rect(track.left, cy - 2.0 * u, track.right - track.left, 4.0 * u), 2.0 * u, t.control_stroke)?;
    c.fill_round(rect(track.left, cy - 2.0 * u, kx - track.left, 4.0 * u), 2.0 * u, t.accent)?;
    c.fill_circle(kx, cy, 7.0 * u, t.flyout)?;
    c.stroke_circle(kx, cy, 7.0 * u, 1.0, t.stroke_strong)?;
    c.fill_circle(kx, cy, 4.0 * u, t.accent)?;

    // ---- the timeline: thumbnails, the part kept between its handles, the key frames
    let strip = at(l.part(Target::Strip));
    let (sw, sh) = (strip.right - strip.left, strip.bottom - strip.top);
    c.fill_round(strip, 6.0 * u, t.bg_deep)?;
    c.push_round_clip(strip, 6.0 * u)?;
    if !thumbs.is_empty() {
        let slot = sw / thumbs.len() as f32;
        for (i, thumb) in thumbs.iter().enumerate() {
            if let Some(bitmap) = thumb {
                let x = strip.left + slot * i as f32;
                c.picture(bitmap, rect(x, strip.top, slot.ceil() + 1.0, sh))?;
            }
        }
    }
    let x_from = l.x_of(frames, frames.time(s.from)) * u;
    let x_to = l.x_of(frames, frames.time(s.to) + 1.0 / frames.rate.max(1.0)) * u;
    let veil = Rgba::hex(0x000000, 0x99);
    c.fill_rect(rect(strip.left, strip.top, x_from - strip.left, sh), veil)?;
    c.fill_rect(rect(x_to, strip.top, strip.right - x_to, sh), veil)?;
    c.pop_layer();
    // The part kept: framed in the accent colour, its handles outside it.
    let edge = 2.0 * u;
    c.fill_rect(rect(x_from, strip.top - edge, x_to - x_from, edge), t.accent)?;
    c.fill_rect(rect(x_from, strip.bottom, x_to - x_from, edge), t.accent)?;
    for target in [Target::From, Target::To] {
        let (hx, hy, hw, hh) = l.handle(frames, &s, target);
        let r = rect(hx * u, (hy - top) * u, hw * u, hh * u);
        let colour = if look.hover == target || look.held == Some(target) { t.accent.brighter(0.1) } else { t.accent };
        c.fill_round(r, 4.0 * u, colour)?;
        let gx = f32::midpoint(r.left, r.right);
        let gy = f32::midpoint(r.top, r.bottom);
        c.fill_rect(rect(gx - 0.75 * u, gy - 7.0 * u, 1.5 * u, 14.0 * u), Rgba::hex(0xffffff, 0xcc))?;
    }
    // Key frames under the strip, where they can be told apart.
    let ticks_y = strip.bottom + 6.0 * u;
    let spacing = sw / frames.keys.len().max(1) as f32;
    if spacing >= 4.0 * u {
        for &k in &frames.keys {
            let x = l.x_of(frames, frames.time(k)) * u;
            let colour = if k == s.from { t.accent } else { t.text_3 };
            c.fill_rect(rect(x - 0.5 * u, ticks_y, 1.0 * u, 4.0 * u), colour)?;
        }
    }

    // ---- the actions: the length kept and how it is written; save as, save
    let sign = at(l.part(Target::Sign));
    let i = 20.0 * u;
    let (data, colour) = if s.lossless { (icons::FLASH, t.accent) } else { (icons::SYNC, t.text_2) };
    c.icon(
        data,
        f32::midpoint(sign.left, sign.right) - i / 2.0,
        f32::midpoint(sign.top, sign.bottom) - i / 2.0,
        i,
        colour,
    )?;
    let kept = format_time(frames.time(s.to) - frames.time(s.from) + 1.0 / frames.rate.max(1.0));
    match s.progress {
        Some(p) => {
            let (bx, _, _, _) = l.part(Target::SaveAs);
            let w = (bx - PAD - 48.0 - 16.0).max(0.0) * u;
            let r = rect(sign.right + 12.0 * u, f32::midpoint(sign.top, sign.bottom) - 3.0 * u, w, 6.0 * u);
            c.fill_round(r, 3.0 * u, t.control_stroke)?;
            c.fill_round(rect(r.left, r.top, (r.right - r.left) * p.clamp(0.0, 1.0), 6.0 * u), 3.0 * u, t.accent)?;
        }
        None => {
            let text = c.gfx.paragraph(&kept, 14.0 * u, true, 1.0e5)?;
            c.text_layout(
                &text.layout,
                sign.right + 10.0 * u,
                f32::midpoint(sign.top, sign.bottom) - text.height / 2.0,
                t.text,
            )?;
        }
    }
    let busy = s.progress.is_some();
    // Save as (Cancel while writing): a quiet button.
    let r = at(l.part(Target::SaveAs));
    let fill = match (look.held, look.hover) {
        (Some(Target::SaveAs), _) => t.control_pressed,
        (_, Target::SaveAs) => t.control_hover,
        _ => t.control,
    };
    c.fill_round(r, 6.0 * u, fill)?;
    c.stroke_round(r, 6.0 * u, 1.0, t.control_stroke)?;
    let label = if busy { &texts.cancel } else { &texts.save_as };
    c.text(label, r, 14.0 * u, t.text, Align::Centre)?;
    // Save: the accent button, off while writing.
    let r = at(l.part(Target::Save));
    let fill = if busy {
        t.control
    } else if look.held == Some(Target::Save) {
        t.accent.darker(0.08)
    } else if look.hover == Target::Save {
        t.accent.brighter(0.08)
    } else {
        t.accent
    };
    c.fill_round(r, 6.0 * u, fill)?;
    let colour = if busy { t.text_off } else { Rgba::hex(0xffffff, 255) };
    c.text(&texts.save, r, 14.0 * u, colour, Align::Centre)
}

/// The editor on screen.
pub struct Cutter {
    inner: Rc<Inner>,
}

impl Cutter {
    /// The window over `geometry` (physical pixels; see [`placement`]) on a monitor at `dpi`.
    pub fn new(geometry: PhysicalRect, dpi: u32, look: Look, frames: Frames, texts: CutterTexts) -> Result<Self> {
        let gfx = Gfx::shared()?;
        let u = dpi.max(48) as f32 / 96.0;
        let popup = Popup::new_window(&gfx, geometry, &texts.title)?;
        let layout = Layout::new(geometry.width as f32 / u, geometry.height as f32 / u, frames.aspect);
        let video = scene::visual(&gfx)?;
        let title = Piece::new(&gfx, false)?;
        let bottom = Piece::new(&gfx, false)?;
        let tip = Piece::new(&gfx, false)?;
        let clock = Piece::new(&gfx, false)?;
        let playhead = Piece::new(&gfx, false)?;
        for v in [&video, &title.visual, &bottom.visual, &playhead.visual, &clock.visual, &tip.visual] {
            scene::add(&popup.root, v)?;
        }
        title.move_to(0.0, 0.0)?;
        bottom.move_to(0.0, (layout.bottom * u).round())?;
        let last = frames.len().saturating_sub(1);
        let inner = Rc::new(Inner {
            theme: Theme::new(look),
            gfx,
            u,
            popup,
            layout,
            texts,
            title,
            bottom,
            tip,
            tip_size: RefCell::default(),
            clock,
            playhead,
            video,
            video_surface: RefCell::default(),
            frame: RefCell::default(),
            thumbs: RefCell::default(),
            state: Cell::new(CutState {
                position: 0,
                from: 0,
                to: last,
                playing: false,
                volume: 1.0,
                muted: false,
                progress: None,
                lossless: true,
            }),
            frames,
            hover: Cell::new(Target::None),
            press: Cell::new(None),
            drag: Cell::new(None),
            events: RefCell::default(),
            closed: Cell::new(false),
        });
        let weak: Weak<Inner> = Rc::downgrade(&inner);
        inner.popup.on_message(move |msg, wparam, lparam| weak.upgrade()?.message(msg, wparam, lparam));
        Ok(Self { inner })
    }

    /// The size the frames are best given at: the picture's place, physical pixels.
    pub fn video_size(&self) -> (u32, u32) {
        let (_, _, w, h) = self.inner.layout.video;
        let u = self.inner.u;
        (((w * u).round() as u32) & !1, ((h * u).round() as u32) & !1)
    }

    /// How many thumbnails the strip shows, and their size (physical pixels).
    pub fn thumbnail_slots(&self) -> (usize, (u32, u32)) {
        let (_, _, w, h) = self.inner.layout.part(Target::Strip);
        let u = self.inner.u;
        let tw = h * self.inner.frames.aspect.max(0.1);
        let n = (w / tw).ceil().max(1.0) as usize;
        (n, (((w / n as f32) * u).ceil() as u32, (h * u).ceil() as u32))
    }

    /// The frame thumbnail `slot` should show.
    pub fn thumbnail_frame(&self, slot: usize) -> usize {
        let (n, _) = self.thumbnail_slots();
        let f = &self.inner.frames;
        let t = (slot as f64 + 0.5) / n as f64 * f.duration();
        f.key_before(f.at(t))
    }

    pub fn set_thumbnail(&self, slot: usize, w: u32, h: u32, bgra: &[u8]) {
        let (n, _) = self.thumbnail_slots();
        match self.inner.gfx.bgra_bitmap(w, h, bgra) {
            Ok(bitmap) => {
                let mut thumbs = self.inner.thumbs.borrow_mut();
                thumbs.resize(n, None);
                if let Some(t) = thumbs.get_mut(slot) {
                    *t = Some(bitmap);
                }
            }
            Err(e) => tracing::warn!("video editor: {e}"),
        }
        self.inner.refresh();
    }

    /// Shows a frame: `w`×`h` BGRA pixels, rows packed.
    pub fn show_frame(&self, w: u32, h: u32, bgra: &[u8]) {
        if let Err(e) = self.inner.show_frame(w, h, bgra) {
            tracing::warn!("video editor: {e}");
        }
    }

    pub fn state(&self) -> CutState {
        self.inner.state.get()
    }

    /// The playhead moved by itself (playing): no [`CutterEvent::Seek`].
    pub fn set_position(&self, frame: usize) {
        let mut s = self.inner.state.get();
        s.position = frame.min(self.inner.frames.len().saturating_sub(1));
        self.inner.state.set(s);
        self.inner.refresh();
    }

    pub fn set_playing(&self, playing: bool) {
        let mut s = self.inner.state.get();
        s.playing = playing;
        self.inner.state.set(s);
        self.inner.refresh();
    }

    pub fn set_lossless(&self, lossless: bool) {
        let mut s = self.inner.state.get();
        s.lossless = lossless;
        self.inner.state.set(s);
        self.inner.refresh();
    }

    /// The share written (`None`: not writing).
    pub fn set_progress(&self, progress: Option<f32>) {
        let mut s = self.inner.state.get();
        s.progress = progress;
        self.inner.state.set(s);
        self.inner.refresh();
    }

    pub fn waker(&self) -> Waker {
        Waker(self.inner.popup.hwnd.0 as isize)
    }

    /// The window, for a dialog to belong to.
    pub fn hwnd(&self) -> isize {
        self.inner.popup.hwnd.0 as isize
    }

    pub fn minimise(&self) {
        // SAFETY: plain call on a window of this thread.
        let _ = unsafe { ShowWindow(self.inner.popup.hwnd, SW_MINIMIZE) };
    }

    /// Ends [`Cutter::run`].
    pub fn close(&self) {
        self.inner.closed.set(true);
    }

    /// Shows the window and runs until [`Cutter::close`], handing the events to `host` and asking it what to do on
    /// every turn.
    pub fn run(&self, host: &mut dyn CutterHost) -> Result<()> {
        let inner = &self.inner;
        inner.refresh();
        inner.popup.show(&inner.gfx)?;
        let mut msg = MSG::default();
        while !inner.closed.get() {
            let events: Vec<CutterEvent> = inner.events.borrow_mut().drain(..).collect();
            for event in events {
                host.event(self, event);
            }
            if inner.closed.get() {
                break;
            }
            let wait = host.tick(self);
            if !inner.events.borrow().is_empty() {
                continue;
            }
            let ms = wait.map_or(u32::MAX, |d| d.as_millis().min(u128::from(u32::MAX - 1)) as u32);
            // SAFETY: no handles; waits for input or the timeout.
            unsafe {
                MsgWaitForMultipleObjectsEx(None, ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
            // SAFETY: `msg` is valid for the calls; messages of this thread.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        inner.closed.set(true);
                        break;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        inner.popup.hide();
        Ok(())
    }

    /// The look of the bottom bands (tests).
    #[doc(hidden)]
    pub fn layout(&self) -> Layout {
        self.inner.layout
    }

    /// Where the pointer is on the screen, for the tests.
    #[doc(hidden)]
    pub fn origin(&self) -> POINT {
        let mut r = RECT::default();
        // SAFETY: a valid out-pointer.
        let _ = unsafe { GetWindowRect(self.inner.popup.hwnd, &mut r) };
        POINT { x: r.left, y: r.top }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(n: usize, rate: f64, key_every: usize) -> Frames {
        Frames {
            times: (0..n).map(|i| i as f64 / rate).collect(),
            keys: (0..n).step_by(key_every).collect(),
            rate,
            aspect: 16.0 / 9.0,
        }
    }

    fn state(n: usize) -> CutState {
        CutState {
            position: 0,
            from: 0,
            to: n - 1,
            playing: false,
            volume: 1.0,
            muted: false,
            progress: None,
            lossless: true,
        }
    }

    #[test]
    fn times_read_as_minutes_seconds_and_milliseconds() {
        assert_eq!(format_time(0.0), "0:00.000");
        assert_eq!(format_time(12.3456), "0:12.346");
        assert_eq!(format_time(75.5), "1:15.500");
        assert_eq!(format_time(3725.0), "1:02:05.000");
    }

    #[test]
    fn frames_are_found_by_time_and_key_frames_before_them() {
        let f = frames(90, 30.0, 30);
        assert_eq!(f.at(0.0), 0);
        assert_eq!(f.at(1.0), 30);
        assert_eq!(f.at(1.0 - 1e-6), 29);
        assert_eq!(f.at(99.0), 89);
        assert_eq!(f.key_before(29), 0);
        assert_eq!(f.key_before(30), 30);
        assert_eq!(f.key_before(89), 60);
        assert!((f.duration() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn the_picture_keeps_its_proportions_in_its_band() {
        let l = Layout::new(1000.0, 800.0, 16.0 / 9.0);
        let (x, y, w, h) = l.video;
        assert!((w / h - 16.0 / 9.0).abs() < 1e-3);
        assert!(y >= TITLE_H && y + h <= l.bottom + 1e-3);
        assert!(x >= 0.0 && x + w <= 1000.0);
        assert!((l.bottom - (800.0 - BOTTOM_H)).abs() < 1e-3);
    }

    #[test]
    fn the_window_leaves_a_margin_above_and_below_and_is_centred() {
        let work = PhysicalRect::new(0, 0, 2560, 1400);
        let PhysicalRect { x, y, width: w, height: h } = placement(work, 1.0, 16.0 / 9.0);
        assert!(y > 0 && y + h as i32 <= 1400);
        assert!((y - (1400 - (y + h as i32))).abs() <= 1, "the same margin above and below");
        assert!((x - (2560 - (x + w as i32))).abs() <= 1, "centred");
        // As wide as the picture needs at that height.
        let video_h = h as f32 - TITLE_H - BOTTOM_H;
        assert!((w as f32 - video_h * 16.0 / 9.0).abs() <= 1.0);
        // A narrow screen: never wider than it.
        let narrow = placement(PhysicalRect::new(0, 0, 1000, 1400), 1.0, 16.0 / 9.0);
        assert!(narrow.width <= 1000);
    }

    #[test]
    fn the_handles_and_buttons_are_where_they_are_drawn() {
        let l = Layout::new(1200.0, 800.0, 16.0 / 9.0);
        let f = frames(300, 60.0, 60);
        let mut s = state(300);
        s.from = 60;
        s.to = 179;
        let centre = |(x, y, w, h): Box2| (x + w / 2.0, y + h / 2.0);
        let (x, y) = centre(l.handle(&f, &s, Target::From));
        assert_eq!(l.target_at(&f, &s, x, y), Target::From);
        let (x, y) = centre(l.handle(&f, &s, Target::To));
        assert_eq!(l.target_at(&f, &s, x, y), Target::To);
        for target in [
            Target::Close,
            Target::Play,
            Target::Previous,
            Target::Next,
            Target::Mute,
            Target::Volume,
            Target::Save,
            Target::SaveAs,
        ] {
            let (x, y) = centre(l.part(target));
            assert_eq!(l.target_at(&f, &s, x, y), target);
        }
        // The middle of the strip moves the playhead; the title drags the window.
        let (sx, sy, sw, sh) = l.part(Target::Strip);
        assert_eq!(l.target_at(&f, &s, sx + sw / 2.0, sy + sh / 2.0), Target::Strip);
        assert_eq!(l.target_at(&f, &s, 300.0, 20.0), Target::Title);
    }

    #[test]
    fn the_editor_draws_in_both_themes() {
        let frames = frames(1800, 60.0, 60);
        let texts = CutterTexts {
            title: "Replay 2026-10-15 21-04-11.mp4".into(),
            save: "Enregistrer".into(),
            save_as: "Enregistrer sous…".into(),
            cancel: "Annuler".into(),
            ..CutterTexts::default()
        };
        let gfx = Gfx::new().expect("gfx");
        let u = 1.25;
        let l = Layout::new(1100.0, 760.0, frames.aspect);
        let (w, h) = ((l.w * u).round() as u32, (l.h * u).round() as u32);
        // A picture: a gradient, as a game would show.
        let (pw, ph) = (320_u32, 180_u32);
        let picture: Vec<u8> = (0..ph)
            .flat_map(|y| (0..pw).flat_map(move |x| [(x * 255 / pw) as u8, (y * 255 / ph) as u8, 120, 255]))
            .collect();
        for (name, dark, progress, lossless) in [("dark", true, None, true), ("light", false, Some(0.4), false)] {
            let t = Theme::new(Look { dark, ..Look::default() });
            let bitmap = gfx.bgra_bitmap(pw, ph, &picture).expect("bitmap");
            let thumbs: Vec<Option<ID2D1Bitmap1>> = (0..12).map(|_| Some(bitmap.clone())).collect();
            let mut state = state(frames.len());
            state.from = 240;
            state.to = 1380;
            state.position = 700;
            state.progress = progress;
            state.lossless = lossless;
            let look = BottomLook { state, hover: Target::Save, held: None, thumbs: 12 };
            let title = TitleLook { hover: Target::None, held: None };
            let th = (TITLE_H * u).ceil() as u32;
            let bh = (BOTTOM_H * u).ceil() as u32;
            let vh = h - th - bh;
            let top = gfx.render(w, th, |c| paint_title(c, &t, u, &l, &texts.title, &title)).expect("title");
            let (vx, vy, vw, vhh) = l.video;
            let video = gfx
                .render(w, vh, |c| {
                    c.fill_rect(rect(0.0, 0.0, w as f32, vh as f32), t.bg_deep)?;
                    c.picture(&bitmap, rect(vx * u, (vy - TITLE_H) * u, vw * u, vhh * u))
                })
                .expect("video");
            let bottom = gfx
                .render(w, bh, |c| {
                    paint_bottom(c, &t, u, &l, &frames, &texts, &look, &thumbs)?;
                    let (cx, cy) = clock_at(&l);
                    paint_clock(c, &t, u, &frames, state.position, (cx * u, (cy - l.bottom) * u))?;
                    let (px, py) = playhead_at(&l, &frames, state.position);
                    paint_playhead(c, &t, u, (px * u, (py - l.bottom) * u))
                })
                .expect("bottom");
            let whole: Vec<u8> = [top, video, bottom].concat();
            assert_eq!(whole.len(), (w * h * 4) as usize);
            crate::tests::save(&format!("cutter-{name}"), w, h, &whole);
        }
    }

    #[test]
    fn the_strip_maps_time_to_x_and_back() {
        let l = Layout::new(1200.0, 800.0, 16.0 / 9.0);
        let f = frames(600, 60.0, 60);
        for frame in [0, 1, 299, 599] {
            let x = l.x_of(&f, f.time(frame) + 0.5 / 60.0);
            assert_eq!(f.at(l.t_of(&f, x)), frame);
        }
    }
}
