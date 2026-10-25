// SPDX-License-Identifier: GPL-3.0-or-later
//! The video editor of the replays, drawn natively: a window of its own that plays a clip and cuts it to the frame.
//!
//! ```text
//! ┌ ✂ name of the file ──────────────────────────────── – ✕ ┐
//! │ ╭───────────────────── the video ───────────────────────╮ │
//! │ ╰───────────────────────────────────────────────────────╯ │
//! │ ╭ ⏮ ▶ ⏭  0:12.345 / 0:30.000     ⚡ 0:08.250   🔊 ━━━○ ╮ │
//! │ │ [▮thumbnails, the part kept between its two handles▮] │ │
//! │ ╰───────────────────────────────────────────────────────╯ │
//! │ ╭ [◉ Discord ⌄][H.264 ⌄][720p ⌄][30 fps ⌄] [Save as][Save]╮ │
//! │ ╰───────────────────────────────────────────────────────╯ │
//! └───────────────────────────────────────────────────────────┘
//! ```
//! The window knows the frames (their times, the key frames), what is kept and where the playhead is; what it cannot do
//! itself (decode a frame, play, write) it asks its host for through [`CutterEvent`]s, on its own thread, between two
//! messages. The export row picks what the copy is made for: the clip as it is, a network (which sets the codec, the
//! size and the frames per second, and may hold the length kept), or values of the user's own.

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
    WM_CHAR, WM_CLOSE, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NCHITTEST, WM_QUIT, WM_SETCURSOR, WM_SYSKEYDOWN,
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
const TIMELINE_H: f32 = 64.0;
/// The player's card: the controls over the timeline.
const PLAYER_H: f32 = CONTROLS_H + TIMELINE_H;
const EXPORT_H: f32 = 56.0;
/// Between the video and the card under it.
const GAP: f32 = 10.0;
const PAD: f32 = 16.0;
/// From a card's edge to what it holds.
const INSET: f32 = 12.0;
/// The bands under the video: their card and the margin under it.
const BOTTOM_H: f32 = GAP + PLAYER_H + EXPORT_H + PAD;
const CARD_RADIUS: f32 = 8.0;
const CAPTION_W: f32 = 46.0;
/// The strip's top, from the player card's.
const STRIP_Y: f32 = CONTROLS_H;
const STRIP_H: f32 = 44.0;
const HANDLE_W: f32 = 12.0;
const VOLUME_W: f32 = 110.0;
/// The room of the length kept, its sign before it.
const KEPT_W: f32 = 220.0;
const SAVE_W: f32 = 130.0;
const SAVE_AS_W: f32 = 170.0;
const BUTTON_H: f32 = 36.0;
const PRESET_W: f32 = 230.0;
const CODEC_W: f32 = 104.0;
const HEIGHT_W: f32 = 104.0;
const FPS_W: f32 = 96.0;
/// A row of a list, its search field, and the room around them.
const ITEM_H: f32 = 36.0;
const SEARCH_H: f32 = 44.0;
const MENU_PAD: f32 = 4.0;
const MENU_GAP: f32 = 6.0;
const PRESET_MENU_W: f32 = 330.0;
/// Room around a list for its shadow.
const SHADOW: f32 = 16.0;
/// How near a key frame the start of the part is pulled onto it (Shift: never).
const SNAP: f32 = 8.0;
/// The narrowest window: the export row in one line.
const MIN_W: f32 = 940.0;

/// What a copy is made as: the clip as it is.
pub const ORIGINAL: &str = "original";
/// Values of the user's own.
pub const CUSTOM: &str = "custom";

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
        self.times.last().map_or(0.0, |t| t + self.frame())
    }

    /// The frame shown at `t`.
    pub fn at(&self, t: f64) -> usize {
        self.times.partition_point(|&x| x <= t + 1e-9).saturating_sub(1)
    }

    /// The first frame shown at `t` or after.
    fn first_at(&self, t: f64) -> usize {
        self.times.partition_point(|&x| x < t - 1e-9).min(self.len().saturating_sub(1))
    }

    fn time(&self, i: usize) -> f64 {
        self.times.get(i).copied().unwrap_or(0.0)
    }

    /// One frame, in seconds.
    fn frame(&self) -> f64 {
        1.0 / self.rate.max(1.0)
    }

    /// The length of frames `from..=to`, the last one included.
    fn length(&self, from: usize, to: usize) -> f64 {
        self.time(to) + self.frame() - self.time(from)
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
    pub minimise: String,
    pub close: String,
    /// The export's values set by hand.
    pub custom: String,
    /// The placeholder of the networks' search field.
    pub search: String,
    /// A search that finds nothing.
    pub no_match: String,
    /// Before the longest part a network takes.
    pub max: String,
}

/// A value of a list: what is stored, and what is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

/// What a copy can be made for: the clip as it is ([`ORIGINAL`]) or a network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub id: String,
    pub name: String,
    /// The network's mark ([`crate::brands::brand`]); empty for the clip as it is.
    pub brand: String,
    /// Other names it is searched by (`twitter`), lower case.
    pub keywords: String,
    /// Its limits and how it is written, muted beside the name.
    pub detail: String,
    /// What it sets: a codec of [`ExportOptions::codecs`], a height and a frame rate of their lists.
    pub codec: String,
    pub height: u32,
    pub fps: u32,
}

/// What the export row offers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExportOptions {
    /// The clip as it is first, then the networks, in the order they are listed.
    pub presets: Vec<Preset>,
    pub codecs: Vec<Choice>,
    /// The heights offered, the clip's first, then lower ones.
    pub heights: Vec<u32>,
    /// The same for the frames per second.
    pub fps: Vec<u32>,
}

/// What the copy is made as: a preset's id ([`ORIGINAL`], [`CUSTOM`] or a network's) and the values it set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExportChoice {
    pub preset: String,
    pub codec: String,
    pub height: u32,
    pub fps: u32,
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
    /// What the copy is made as changed ([`Cutter::export_choice`]).
    Export,
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
    Minimise,
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
    Preset,
    Codec,
    Height,
    Fps,
    SaveAs,
    Save,
    /// The search field of an open list.
    Search,
    /// A row of an open list (its place among those it shows, the first one at 0, scrolled or not).
    Item(usize),
    /// The rest of an open list.
    Menu,
    None,
}

/// The lists of the export row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Preset,
    Codec,
    Height,
    Fps,
}

impl MenuKind {
    const fn anchor(self) -> Target {
        match self {
            Self::Preset => Target::Preset,
            Self::Codec => Target::Codec,
            Self::Height => Target::Height,
            Self::Fps => Target::Fps,
        }
    }

    const fn of(target: Target) -> Option<Self> {
        match target {
            Target::Preset => Some(Self::Preset),
            Target::Codec => Some(Self::Codec),
            Target::Height => Some(Self::Height),
            Target::Fps => Some(Self::Fps),
            _ => None,
        }
    }
}

/// What goes before a label.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Mark {
    None,
    Glyph(&'static str),
    /// A network's ([`crate::brands::brand`]).
    Brand(String),
}

/// A row of a list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    key: String,
    label: String,
    detail: String,
    mark: Mark,
    /// Rows of one group (a network's plans) go together; a hairline parts two groups.
    group: String,
}

/// A list open, and where it is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MenuState {
    kind: MenuKind,
    search: String,
    /// The row under the pointer or the arrows.
    hover: Option<usize>,
    /// The first row shown.
    scroll: usize,
}

/// A list's box (window logical pixels), the rows it shows at once, and the height of its search field.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MenuBox {
    r: Box2,
    rows: usize,
    search: f32,
}

/// The parts of the window, logical pixels: `w`×`h` the whole window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub w: f32,
    pub h: f32,
    /// The video's card (between the title and the player).
    pub band: Box2,
    /// Where the picture goes in it, its proportions kept.
    pub video: Box2,
    /// The bottom bands' top.
    pub bottom: f32,
}

impl Layout {
    pub fn new(w: f32, h: f32, aspect: f32) -> Self {
        let band_h = (h - TITLE_H - BOTTOM_H).max(1.0);
        let band_w = (w - 2.0 * PAD).max(1.0);
        let band = (PAD, TITLE_H, band_w, band_h);
        let aspect = if aspect > 0.0 { aspect } else { 16.0 / 9.0 };
        let (vw, vh) = if band_w / band_h > aspect { (band_h * aspect, band_h) } else { (band_w, band_w / aspect) };
        Self {
            w,
            h,
            band,
            video: (PAD + (band_w - vw) / 2.0, TITLE_H + (band_h - vh) / 2.0, vw, vh),
            bottom: TITLE_H + band_h,
        }
    }

    /// The player card's top.
    fn player(&self) -> f32 {
        self.bottom + GAP
    }

    /// The export row's top, under a hairline in the same card.
    fn export(&self) -> f32 {
        self.player() + PLAYER_H
    }

    /// A part of the window, in window coordinates.
    fn part(&self, target: Target) -> Box2 {
        let card = self.player();
        let left = PAD + INSET;
        let right = self.w - PAD - INSET;
        let button_y = self.export() + (EXPORT_H - BUTTON_H) / 2.0;
        let mute_x = right - VOLUME_W - 46.0;
        let after = |x: f32, w: f32| x + w + 8.0;
        let codec_x = after(left, PRESET_W);
        let height_x = after(codec_x, CODEC_W);
        let fps_x = after(height_x, HEIGHT_W);
        match target {
            Target::Title => (0.0, 0.0, self.w - 2.0 * CAPTION_W, TITLE_H),
            Target::Minimise => (self.w - 2.0 * CAPTION_W, 0.0, CAPTION_W, TITLE_H),
            Target::Close => (self.w - CAPTION_W, 0.0, CAPTION_W, TITLE_H),
            Target::Video => self.band,
            Target::Previous => (left - 4.0, card + 8.0, 32.0, 32.0),
            Target::Play => (left + 32.0, card + 8.0, 32.0, 32.0),
            Target::Next => (left + 68.0, card + 8.0, 32.0, 32.0),
            Target::Mute => (mute_x, card + 8.0, 32.0, 32.0),
            Target::Volume => (right - 6.0 - VOLUME_W, card + 14.0, VOLUME_W, 20.0),
            Target::Sign => (mute_x - 16.0 - KEPT_W, card + 12.0, 24.0, 24.0),
            Target::Strip | Target::From | Target::To => {
                (left + HANDLE_W, card + STRIP_Y, right - left - 2.0 * HANDLE_W, STRIP_H)
            }
            Target::Preset => (left, button_y, PRESET_W, BUTTON_H),
            Target::Codec => (codec_x, button_y, CODEC_W, BUTTON_H),
            Target::Height => (height_x, button_y, HEIGHT_W, BUTTON_H),
            Target::Fps => (fps_x, button_y, FPS_W, BUTTON_H),
            Target::Save => (right - SAVE_W, button_y, SAVE_W, BUTTON_H),
            Target::SaveAs => (right - SAVE_W - 8.0 - SAVE_AS_W, button_y, SAVE_AS_W, BUTTON_H),
            Target::Search | Target::Item(_) | Target::Menu | Target::None => (0.0, 0.0, 0.0, 0.0),
        }
    }

    /// The box of the list of `kind` holding `n` rows: over its button, as high as the video leaves.
    fn menu(&self, kind: MenuKind, n: usize) -> MenuBox {
        let (ax, ay, aw, _) = self.part(kind.anchor());
        let search = if kind == MenuKind::Preset { SEARCH_H } else { 0.0 };
        let w = if kind == MenuKind::Preset { PRESET_MENU_W.max(aw) } else { aw.max(120.0) };
        let room = ay - MENU_GAP - TITLE_H - 8.0 - search - 2.0 * MENU_PAD;
        let rows = n.max(1).min((room / ITEM_H).floor().max(1.0) as usize);
        let h = search + rows as f32 * ITEM_H + 2.0 * MENU_PAD;
        let x = ax.min(self.w - PAD - w).max(PAD);
        MenuBox { r: (x, ay - MENU_GAP - h, w, h), rows, search }
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
            _ => (self.x_of(frames, frames.time(state.to) + frames.frame()), y - 4.0, HANDLE_W, h + 8.0),
        }
    }

    /// What is at `(x, y)` (logical pixels), lists aside.
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
            Target::Minimise,
            Target::Title,
            Target::Previous,
            Target::Play,
            Target::Next,
            Target::Mute,
            Target::Volume,
            Target::Sign,
            Target::Preset,
            Target::Codec,
            Target::Height,
            Target::Fps,
            Target::SaveAs,
            Target::Save,
            Target::Video,
        ]
        .into_iter()
        .find(|t| inside(self.part(*t)))
        .unwrap_or(Target::None)
    }
}

/// What is at `(x, y)` in the list `m` showing `n` rows from `scroll`; `None`: outside it.
fn menu_target(m: &MenuBox, n: usize, scroll: usize, x: f32, y: f32) -> Option<Target> {
    let (mx, my, mw, mh) = m.r;
    if x < mx || x >= mx + mw || y < my || y >= my + mh {
        return None;
    }
    let top = my + MENU_PAD;
    if y < top + m.search {
        return Some(Target::Search);
    }
    let row = ((y - top - m.search) / ITEM_H).floor().max(0.0) as usize;
    Some(if row < m.rows && scroll + row < n { Target::Item(scroll + row) } else { Target::Menu })
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
    let min_w = MIN_W * u;
    let width = (video_h * aspect + 2.0 * PAD * u).clamp(min_w.min(w - 2.0 * margin_x), w - 2.0 * margin_x);
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

/// `m:ss` (or `h:mm:ss`), whole seconds down: a limit worked out is no more precise.
fn format_seconds(t: f64) -> String {
    let s = t.max(0.0).floor() as u64;
    let (h, m, s) = (s / 3_600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Lower case, without accents: what a search compares.
fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .collect()
}

/// How well `name` (and its `keywords`) answers `query` (folded): its start first, then a word's start, then anywhere;
/// `None`: not at all.
fn rank(name: &str, keywords: &str, query: &str) -> Option<u8> {
    let name = fold(name);
    if name.starts_with(query) {
        return Some(0);
    }
    let words = format!("{name} {keywords}");
    if words.split_whitespace().any(|w| w.starts_with(query)) {
        return Some(1);
    }
    words.contains(query).then_some(2)
}

/// The rows of the list of presets: the clip as it is, the values set by hand when they are chosen, then the networks;
/// those a search finds, best first.
fn preset_items(options: &ExportOptions, choice: &ExportChoice, custom: &str, search: &str) -> Vec<Item> {
    let item = |p: &Preset| Item {
        key: p.id.clone(),
        label: p.name.clone(),
        detail: p.detail.clone(),
        mark: if p.brand.is_empty() { Mark::Glyph(icons::VIDEO) } else { Mark::Brand(p.brand.clone()) },
        group: p.brand.clone(),
    };
    let mut rows: Vec<(Item, &str)> = Vec::new();
    let mut presets = options.presets.iter();
    if let Some(first) = presets.next() {
        rows.push((item(first), ""));
    }
    if choice.preset == CUSTOM {
        let detail = format!("{}p{} · {}", choice.height, choice.fps, codec_label(options, &choice.codec));
        let mark = Mark::Glyph(icons::SETTINGS);
        rows.push((Item { key: CUSTOM.into(), label: custom.into(), detail, mark, group: String::new() }, ""));
    }
    rows.extend(presets.map(|p| (item(p), p.keywords.as_str())));
    let query = fold(search.trim());
    if query.is_empty() {
        return rows.into_iter().map(|(i, _)| i).collect();
    }
    let mut found: Vec<(u8, Item)> =
        rows.into_iter().filter_map(|(i, k)| rank(&i.label, k, &query).map(|r| (r, i))).collect();
    found.sort_by_key(|(r, _)| *r);
    found.into_iter().map(|(_, i)| i).collect()
}

fn codec_label(options: &ExportOptions, id: &str) -> String {
    options.codecs.iter().find(|c| c.id == id).map_or_else(|| id.to_uppercase(), |c| c.label.clone())
}

/// What the export row shows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExportLook {
    mark: Mark,
    preset: String,
    codec: String,
    height: u32,
    fps: u32,
}

/// What the bottom bands show; drawn again only when it changes.
#[derive(Debug, Clone, PartialEq)]
struct BottomLook {
    state: CutState,
    hover: Target,
    held: Option<Target>,
    thumbs: usize,
    /// `None`: nothing to choose (no options given).
    export: Option<ExportLook>,
    menu: Option<MenuKind>,
    /// The longest part kept, in seconds.
    limit: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
struct TitleLook {
    hover: Target,
    held: Option<Target>,
}

/// What an open list shows.
#[derive(Debug, Clone, PartialEq)]
struct MenuLook {
    w: f32,
    h: f32,
    /// The search field's height (0: none), what is typed in it, and what it says empty.
    search_h: f32,
    search: String,
    placeholder: String,
    no_match: String,
    /// The rows shown, the first one's place, and how many there are.
    items: Vec<Item>,
    first: usize,
    total: usize,
    selected: Option<usize>,
    hover: Option<usize>,
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
    list: Piece<MenuLook>,
    video: IDCompositionVisual2,
    video_surface: RefCell<Option<(IDCompositionSurface, (u32, u32))>>,
    frame: RefCell<Option<(ID2D1Bitmap1, (u32, u32))>>,
    thumbs: RefCell<Vec<Option<ID2D1Bitmap1>>>,
    state: Cell<CutState>,
    options: RefCell<ExportOptions>,
    choice: RefCell<ExportChoice>,
    /// The longest part the choice takes, in seconds.
    limit: Cell<Option<f64>>,
    menu: RefCell<Option<MenuState>>,
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

/// Holds `s` to `limit` seconds: the end that moved stops there, else the start comes nearer the end.
fn hold(frames: &Frames, s: &mut CutState, before: &CutState, limit: Option<f64>) {
    let Some(limit) = limit else {
        return;
    };
    if frames.length(s.from, s.to) <= limit + 1e-6 {
        return;
    }
    if s.to != before.to && s.from == before.from {
        s.to = frames.at(frames.time(s.from) + limit - frames.frame() + 1e-6).max(s.from);
    } else {
        s.from = frames.first_at(frames.time(s.to) + frames.frame() - limit).min(s.to);
    }
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
        hold(&self.frames, &mut s, &before, self.limit.get());
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

    /// The rows of the list `menu`.
    fn items(&self, menu: &MenuState) -> Vec<Item> {
        let options = self.options.borrow();
        let plain = |key: String, label: String| Item {
            key,
            label,
            detail: String::new(),
            mark: Mark::None,
            group: String::new(),
        };
        match menu.kind {
            MenuKind::Preset => preset_items(&options, &self.choice.borrow(), &self.texts.custom, &menu.search),
            MenuKind::Codec => options.codecs.iter().map(|c| plain(c.id.clone(), c.label.clone())).collect(),
            MenuKind::Height => options.heights.iter().map(|h| plain(h.to_string(), format!("{h}p"))).collect(),
            MenuKind::Fps => options.fps.iter().map(|f| plain(f.to_string(), format!("{f} fps"))).collect(),
        }
    }

    /// The key of the row of `kind` chosen now.
    fn chosen(&self, kind: MenuKind) -> String {
        let c = self.choice.borrow();
        match kind {
            MenuKind::Preset => c.preset.clone(),
            MenuKind::Codec => c.codec.clone(),
            MenuKind::Height => c.height.to_string(),
            MenuKind::Fps => c.fps.to_string(),
        }
    }

    fn open_menu(&self, kind: MenuKind) {
        let mut menu = MenuState { kind, search: String::new(), hover: None, scroll: 0 };
        let items = self.items(&menu);
        let chosen = self.chosen(kind);
        menu.hover = items.iter().position(|i| i.key == chosen);
        let rows = self.layout.menu(kind, items.len()).rows;
        if let Some(h) = menu.hover {
            menu.scroll = (h + 1).saturating_sub(rows);
        }
        *self.menu.borrow_mut() = Some(menu);
    }

    fn close_menu(&self) {
        *self.menu.borrow_mut() = None;
    }

    /// Moves the row of the open list the keyboard is on by `by`, and the list with it.
    fn move_hover(&self, by: i64) {
        let Some(mut menu) = self.menu.borrow().clone() else {
            return;
        };
        let n = self.items(&menu).len();
        if n == 0 {
            return;
        }
        let h = menu.hover.map_or(if by > 0 { -1 } else { n as i64 }, |h| h as i64) + by;
        let h = h.clamp(0, n as i64 - 1) as usize;
        menu.hover = Some(h);
        let rows = self.layout.menu(menu.kind, n).rows;
        if h < menu.scroll {
            menu.scroll = h;
        } else if h >= menu.scroll + rows {
            menu.scroll = h + 1 - rows;
        }
        *self.menu.borrow_mut() = Some(menu);
    }

    /// Picks row `i` of the open list.
    fn pick(&self, i: usize) {
        let Some(menu) = self.menu.borrow().clone() else {
            return;
        };
        let items = self.items(&menu);
        self.close_menu();
        let Some(item) = items.get(i) else {
            return;
        };
        let before = self.choice.borrow().clone();
        let mut c = before.clone();
        let options = self.options.borrow();
        match menu.kind {
            MenuKind::Preset => {
                if let Some(p) = options.presets.iter().find(|p| p.id == item.key) {
                    c = ExportChoice { preset: p.id.clone(), codec: p.codec.clone(), height: p.height, fps: p.fps };
                }
            }
            MenuKind::Codec => c.codec.clone_from(&item.key),
            MenuKind::Height => c.height = item.key.parse().unwrap_or(c.height),
            MenuKind::Fps => c.fps = item.key.parse().unwrap_or(c.fps),
        }
        if menu.kind != MenuKind::Preset && c != before {
            // A value set by hand: the preset no longer says what the copy is.
            let still = options
                .presets
                .iter()
                .find(|p| p.id == c.preset)
                .is_some_and(|p| (p.codec.as_str(), p.height, p.fps) == (c.codec.as_str(), c.height, c.fps));
            if !still {
                c.preset = CUSTOM.into();
            }
        }
        drop(options);
        if c != before {
            *self.choice.borrow_mut() = c;
            self.emit(CutterEvent::Export);
        }
    }

    fn act(&self, target: Target) {
        match target {
            Target::Close => self.emit(CutterEvent::Close),
            Target::Minimise => {
                // SAFETY: plain call on a window of this thread.
                let _ = unsafe { ShowWindow(self.popup.hwnd, SW_MINIMIZE) };
            }
            Target::Play | Target::Video => self.toggle_play(),
            Target::Previous => self.step(-1),
            Target::Next => self.step(1),
            Target::Mute => self.set_state(|s| s.muted = !s.muted),
            Target::Preset | Target::Codec | Target::Height | Target::Fps if !self.busy() => {
                if let Some(kind) = MenuKind::of(target) {
                    self.open_menu(kind);
                }
            }
            Target::Item(i) => self.pick(i),
            Target::Save if !self.busy() => self.emit(CutterEvent::Save),
            Target::SaveAs if self.busy() => self.emit(CutterEvent::Cancel),
            Target::SaveAs => self.emit(CutterEvent::SaveAs),
            _ => {}
        }
    }

    /// The open list's box, its rows' count and where it is scrolled to.
    fn menu_box(&self) -> Option<(MenuBox, usize, usize)> {
        let menu = self.menu.borrow().clone()?;
        let n = self.items(&menu).len();
        Some((self.layout.menu(menu.kind, n), n, menu.scroll))
    }

    /// What is at `(x, y)` (logical pixels): an open list first.
    fn target_at(&self, x: f32, y: f32) -> Target {
        if let Some((m, n, scroll)) = self.menu_box()
            && let Some(t) = menu_target(&m, n, scroll, x, y)
        {
            return t;
        }
        self.layout.target_at(&self.frames, &self.state.get(), x, y)
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
                let t = l.t_of(f, x) - f.frame() / 2.0;
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

    /// A key while a list is open: every one goes to it (the characters come as `WM_CHAR`).
    fn menu_key(&self, vk: u16) {
        let rows = self.menu_box().map_or(1, |(m, _, _)| m.rows) as i64;
        match vk {
            // Escape
            0x1b => self.close_menu(),
            // Up, Down, Page Up, Page Down
            0x26 => self.move_hover(-1),
            0x28 => self.move_hover(1),
            0x21 => self.move_hover(-rows),
            0x22 => self.move_hover(rows),
            // Enter
            0x0d => {
                let hover = self.menu.borrow().as_ref().and_then(|m| m.hover);
                match hover {
                    Some(i) => self.pick(i),
                    None => self.close_menu(),
                }
            }
            // Backspace
            0x08 => self.type_in(None),
            _ => {}
        }
    }

    /// Types `c` in the open list's search field (`None`: the last character is removed).
    fn type_in(&self, c: Option<char>) {
        let mut slot = self.menu.borrow_mut();
        let Some(menu) = slot.as_mut().filter(|m| m.kind == MenuKind::Preset) else {
            return;
        };
        match c {
            Some(c) => menu.search.push(c),
            None => {
                menu.search.pop();
            }
        }
        menu.scroll = 0;
        menu.hover = Some(0);
    }

    fn key(&self, vk: u16) -> bool {
        if self.menu.borrow().is_some() {
            self.menu_key(vk);
            return true;
        }
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
            Target::Minimise => &t.minimise,
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
                let hit = self.target_at(lx, ly);
                return Some(LRESULT(if hit == Target::Title { HTCAPTION as isize } else { HTCLIENT as isize }));
            }
            WM_CLOSE => {
                self.emit(CutterEvent::Close);
                return Some(LRESULT(0));
            }
            WM_SETCURSOR if (lparam.0 & 0xffff) == 1 => {
                let pointer = match self.hover.get() {
                    Target::From | Target::To => Pointer::Zone(vixeeny_editor::CursorHint::ResizeEw),
                    Target::None | Target::Title | Target::Sign | Target::Search | Target::Menu => Pointer::Arrow,
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
            WM_CHAR => {
                if self.menu.borrow().is_none() {
                    return None;
                }
                if let Some(c) = char::from_u32(wparam.0 as u32).filter(|c| !c.is_control()) {
                    self.type_in(Some(c));
                }
            }
            WM_MOUSEMOVE => {
                self.popup.track_leave();
                match self.drag.get() {
                    Some(drag) => self.drag_to(drag, x),
                    None => {
                        let at = self.target_at(x, y);
                        self.hover.set(at);
                        if let (Target::Item(i), Some(menu)) = (at, self.menu.borrow_mut().as_mut()) {
                            menu.hover = Some(i);
                        }
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
                if let Some((m, n, _)) = self.menu_box() {
                    if let Some(menu) = self.menu.borrow_mut().as_mut() {
                        let most = n.saturating_sub(m.rows);
                        menu.scroll =
                            if delta > 0 { menu.scroll.saturating_sub(3) } else { (menu.scroll + 3).min(most) };
                    }
                } else {
                    let second = self.frames.rate.round().max(1.0) as i64;
                    let step = if shift() { second } else { 1 };
                    self.step(if delta > 0 { -step } else { step });
                }
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let at = self.target_at(x, y);
                if self.menu.borrow().is_some() && !matches!(at, Target::Item(_) | Target::Search | Target::Menu) {
                    // A press beside an open list closes it, and does nothing else.
                    self.close_menu();
                    self.press.set(None);
                    self.refresh();
                    return Some(LRESULT(0));
                }
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
                    let at = self.target_at(x, y);
                    if pressed == Some(at) {
                        self.act(at);
                    }
                    self.hover.set(self.target_at(x, y));
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

    /// What the export row shows; `None` when there is nothing to choose.
    fn export_look(&self) -> Option<ExportLook> {
        let options = self.options.borrow();
        if options.presets.is_empty() {
            return None;
        }
        let c = self.choice.borrow();
        let (mark, preset) = match options.presets.iter().find(|p| p.id == c.preset) {
            Some(p) if p.brand.is_empty() => (Mark::Glyph(icons::VIDEO), p.name.clone()),
            Some(p) => (Mark::Brand(p.brand.clone()), p.name.clone()),
            None => (Mark::Glyph(icons::SETTINGS), self.texts.custom.clone()),
        };
        Some(ExportLook { mark, preset, codec: codec_label(&options, &c.codec), height: c.height, fps: c.fps })
    }

    /// What the open list shows, and where its surface goes (window logical pixels).
    fn menu_look(&self) -> Option<(MenuLook, (f32, f32))> {
        let menu = self.menu.borrow().clone()?;
        let items = self.items(&menu);
        let m = self.layout.menu(menu.kind, items.len());
        let chosen = self.chosen(menu.kind);
        let look = MenuLook {
            w: m.r.2,
            h: m.r.3,
            search_h: m.search,
            search: menu.search.clone(),
            placeholder: self.texts.search.clone(),
            no_match: self.texts.no_match.clone(),
            selected: items.iter().position(|i| i.key == chosen),
            first: menu.scroll,
            total: items.len(),
            items: items.into_iter().skip(menu.scroll).take(m.rows).collect(),
            hover: menu.hover,
        };
        Some((look, (m.r.0 - SHADOW, m.r.1 - SHADOW)))
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
            export: self.export_look(),
            menu: self.menu.borrow().as_ref().map(|m| m.kind),
            limit: self.limit.get(),
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
        match self.menu_look() {
            Some((look, (mx, my))) => {
                let size = (((look.w + 2.0 * SHADOW) * u).ceil() as u32, ((look.h + 2.0 * SHADOW) * u).ceil() as u32);
                self.list.show(&self.gfx, &look, || Ok(size), |c| paint_menu(c, &self.theme, u, &look))?;
                self.list.move_to((mx * u).round(), (my * u).round())?;
            }
            None => self.list.hide()?,
        }
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
                let (bx, by, bw, bh) = layout.part(hover);
                let m = crate::paint::TIP_MARGIN * u;
                let tx =
                    ((bx + bw / 2.0) * u - size.0 as f32 / 2.0).clamp(0.0, (layout.w * u - size.0 as f32).max(0.0));
                // Under the title's buttons, above the others.
                let ty = if by < TITLE_H { (by + bh) * u - m + 4.0 * u } else { by * u - bubble.1 - m - 4.0 * u };
                self.tip.move_to(tx.round(), ty.round())?;
            }
            None => self.tip.hide()?,
        }
        // SAFETY: plain call.
        unsafe { self.gfx.dcomp.Commit() }
    }

    /// Draws `bgra` (`w`×`h`) in the video's card.
    fn show_frame(&self, w: u32, h: u32, bgra: &[u8]) -> Result<()> {
        let mut frame = self.frame.borrow_mut();
        let reuse = frame.as_ref().is_some_and(|(_, s)| *s == (w, h));
        if reuse {
            if let Some((bitmap, _)) = frame.as_ref() {
                Gfx::update_bgra(bitmap, w, h, bgra)?;
            }
        } else {
            *frame = Some((self.gfx.bgra_bitmap(w, h, bgra)?, (w, h)));
        }
        drop(frame);
        self.paint_band()
    }

    /// The band of the video: its card, and the frame shown last in it.
    fn paint_band(&self) -> Result<()> {
        let u = self.u;
        let (_, by, _, bh) = self.layout.band;
        let (sw, sh) = ((self.layout.w * u).round() as u32, (bh * u).round() as u32);
        let mut slot = self.video_surface.borrow_mut();
        if slot.as_ref().is_none_or(|(_, s)| *s != (sw, sh)) {
            let surface = self.gfx.opaque_surface(sw, sh)?;
            // SAFETY: plain property call.
            unsafe { self.video.SetContent(&surface)? };
            scene::set_offset(&self.video, 0.0, (by * u).round())?;
            *slot = Some((surface, (sw, sh)));
        }
        let Some((surface, _)) = slot.as_ref() else {
            return Ok(());
        };
        let frame = self.frame.borrow();
        let bitmap = frame.as_ref().map(|(b, _)| b);
        self.gfx
            .draw(surface, sw, sh, |c| paint_video(c, &self.theme, u, &self.layout, bitmap, (sw as f32, sh as f32)))?;
        // SAFETY: plain call.
        unsafe { self.gfx.dcomp.Commit() }
    }
}

/// The video in its band: the picture alone, its corners rounded, on the window's background.
fn paint_video(
    c: &Canvas<'_>,
    t: &Theme,
    u: f32,
    l: &Layout,
    bitmap: Option<&ID2D1Bitmap1>,
    (w, h): (f32, f32),
) -> Result<()> {
    c.fill_rect(rect(0.0, 0.0, w, h), t.bg_deep)?;
    let Some(bitmap) = bitmap else {
        return Ok(());
    };
    let (vx, vy, vw, vh) = l.video;
    let picture = rect(vx * u, (vy - l.band.1) * u, vw * u, vh * u);
    c.push_round_clip(picture, CARD_RADIUS * u)?;
    c.picture(bitmap, picture)?;
    c.pop_layer();
    Ok(())
}

/// Draws `mark` `size` pixels wide at `(x, y)`.
fn paint_mark(c: &Canvas<'_>, t: &Theme, mark: &Mark, x: f32, y: f32, size: f32) -> Result<()> {
    match mark {
        Mark::None => Ok(()),
        Mark::Glyph(data) => c.icon(data, x, y, size, t.text),
        Mark::Brand(network) => match crate::brands::brand(network) {
            Some(b) => c.icon(b.path, x, y, size, b.colour.unwrap_or(t.text)),
            None => Ok(()),
        },
    }
}

fn paint_title(c: &Canvas<'_>, t: &Theme, u: f32, l: &Layout, name: &str, look: &TitleLook) -> Result<()> {
    c.fill_rect(rect(0.0, 0.0, l.w * u, TITLE_H * u), t.bg_deep)?;
    let icon = 18.0 * u;
    c.icon(icons::CUT, (PAD + 2.0) * u, (TITLE_H * u - icon) / 2.0, icon, t.text_2)?;
    let text = c.gfx.paragraph_lines(name, 13.0 * u, false, (l.w - 2.0 * CAPTION_W - 70.0) * u, 1)?;
    c.text_layout(&text.layout, (PAD + 30.0) * u, (TITLE_H * u - text.height) / 2.0, t.text)?;
    // Minimise: a quiet square; close: red under the pointer, as Windows draws them.
    let (x, y, w, h) = l.part(Target::Minimise);
    let back = match (look.held, look.hover) {
        (Some(Target::Minimise), _) => Some(t.subtle_pressed),
        (_, Target::Minimise) => Some(t.subtle_hover),
        _ => None,
    };
    if let Some(back) = back {
        c.fill_rect(rect(x * u, y * u, w * u, h * u), back)?;
    }
    let icon = 16.0 * u;
    c.icon(icons::MINIMIZE, (x + w / 2.0) * u - icon / 2.0, (y + h / 2.0) * u - icon / 2.0, icon, t.text)?;
    let (x, y, w, h) = l.part(Target::Close);
    let (back, fore) = match (look.held, look.hover) {
        (Some(Target::Close), _) => (Some(Rgba::hex(0xc42b1c, 0xcc)), Rgba::hex(0xffffff, 255)),
        (_, Target::Close) => (Some(Rgba::hex(0xc42b1c, 255)), Rgba::hex(0xffffff, 255)),
        _ => (None, t.text),
    };
    if let Some(back) = back {
        c.fill_rect(rect(x * u, y * u, w * u, h * u), back)?;
    }
    c.icon(icons::DISMISS, (x + w / 2.0) * u - icon / 2.0, (y + h / 2.0) * u - icon / 2.0, icon, fore)
}

/// The size of the time's surface (physical pixels): room for `h:mm:ss.mmm / h:mm:ss.mmm`.
fn clock_size(u: f32) -> (u32, u32) {
    ((300.0 * u).ceil() as u32, (CONTROLS_H * u).ceil() as u32)
}

/// Where the time is drawn, window logical pixels.
fn clock_at(l: &Layout) -> (f32, f32) {
    (PAD + INSET + 112.0, l.player())
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
    (x - PLAYHEAD_W / 2.0, l.player() + STRIP_Y - 11.0)
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
    c.fill_rect(rect(0.0, 0.0, l.w * u, BOTTOM_H * u), t.bg_deep)?;
    // One card, a hairline between the player and the export.
    let card = at((PAD, l.player(), l.w - 2.0 * PAD, PLAYER_H + EXPORT_H));
    c.fill_round(card, CARD_RADIUS * u, t.flyout)?;
    c.stroke_round(card, CARD_RADIUS * u, 1.0, t.stroke_strong)?;
    let line = at((PAD + INSET, l.export(), l.w - 2.0 * (PAD + INSET), 0.0));
    c.fill_rect(rect(line.left, line.top, line.right - line.left, 1.0), t.divider)?;
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

    // ---- controls: frame by frame, play, the time; the length kept and the volume on the right
    back(Target::Previous, 6.0)?;
    icon_in(icons::PREVIOUS, Target::Previous, 18.0, t.text)?;
    back(Target::Next, 6.0)?;
    icon_in(icons::NEXT, Target::Next, 18.0, t.text)?;
    back(Target::Play, 6.0)?;
    icon_in(if s.playing { icons::PAUSE_FILLED } else { icons::PLAY_FILLED }, Target::Play, 18.0, t.text)?;

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
    c.fill_round(rect(track.left, cy - 2.0 * u, kx - track.left, 4.0 * u), 2.0 * u, t.text_2)?;
    c.fill_circle(kx, cy, 7.0 * u, t.flyout)?;
    c.stroke_circle(kx, cy, 7.0 * u, 1.0, t.stroke_strong)?;
    c.fill_circle(kx, cy, 4.0 * u, t.text)?;

    // The length kept, how it is written, and the longest a network takes.
    let sign = at(l.part(Target::Sign));
    let i = 20.0 * u;
    let data = if s.lossless { icons::FLASH } else { icons::SYNC };
    let mid = f32::midpoint(sign.top, sign.bottom);
    c.icon(data, f32::midpoint(sign.left, sign.right) - i / 2.0, mid - i / 2.0, i, t.text_2)?;
    let kept = c.gfx.paragraph(&format_time(frames.length(s.from, s.to)), 14.0 * u, true, 1.0e5)?;
    let kx = sign.right + 8.0 * u;
    c.text_layout(&kept.layout, kx, mid - kept.height / 2.0, t.text)?;
    if let Some(limit) = look.limit {
        let max = c.gfx.paragraph(&format!("{} {}", texts.max, format_seconds(limit)), 13.0 * u, false, 1.0e5)?;
        c.text_layout(&max.layout, kx + kept.width + 8.0 * u, mid - max.height / 2.0, t.text_3)?;
    }

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
    let x_to = l.x_of(frames, frames.time(s.to) + frames.frame()) * u;
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
            let colour = if k == s.from { t.text_2 } else { t.text_3.with_alpha(0.5) };
            c.fill_rect(rect(x - 0.5 * u, ticks_y, 1.0 * u, 4.0 * u), colour)?;
        }
    }

    // ---- the export: what the copy is made as (the bar of the writing in its place), save as, save
    let busy = s.progress.is_some();
    match (s.progress, &look.export) {
        (Some(p), _) => {
            let from = at(l.part(Target::Preset));
            let until = at(l.part(Target::SaveAs));
            let mid = f32::midpoint(from.top, from.bottom);
            let r = rect(from.left, mid - 3.0 * u, (until.left - 16.0 * u - from.left).max(0.0), 6.0 * u);
            c.fill_round(r, 3.0 * u, t.control_stroke)?;
            c.fill_round(rect(r.left, r.top, (r.right - r.left) * p.clamp(0.0, 1.0), 6.0 * u), 3.0 * u, t.accent)?;
        }
        (None, Some(export)) => {
            // The preset as a button; the values it sets quieter, framed only under the pointer.
            let dropdown = |target: Target, mark: &Mark, label: &str| -> Result<()> {
                let r = at(l.part(target));
                let open = look.menu.map(MenuKind::anchor) == Some(target);
                let main = target == Target::Preset;
                let fill = if look.held == Some(target) || open {
                    Some(t.control_pressed)
                } else if look.hover == target {
                    Some(t.control_hover)
                } else {
                    main.then_some(t.control)
                };
                if let Some(fill) = fill {
                    c.fill_round(r, 6.0 * u, fill)?;
                    c.stroke_round(r, 6.0 * u, 1.0, t.control_stroke)?;
                }
                let mid = f32::midpoint(r.top, r.bottom);
                let mut x = r.left + 12.0 * u;
                if *mark != Mark::None {
                    paint_mark(c, t, mark, x, mid - 9.0 * u, 18.0 * u)?;
                    x += 28.0 * u;
                }
                c.text(
                    label,
                    rect(x, r.top, r.right - 30.0 * u - x, r.bottom - r.top),
                    14.0 * u,
                    t.text,
                    Align::Leading,
                )?;
                let chevron = 12.0 * u;
                c.icon(icons::CHEVRON_DOWN, r.right - 12.0 * u - chevron, mid - chevron / 2.0, chevron, t.text_2)
            };
            dropdown(Target::Preset, &export.mark, &export.preset)?;
            dropdown(Target::Codec, &Mark::None, &export.codec)?;
            dropdown(Target::Height, &Mark::None, &format!("{}p", export.height))?;
            dropdown(Target::Fps, &Mark::None, &format!("{} fps", export.fps))?;
        }
        (None, None) => {}
    }
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

/// An open list, its surface's room for the shadow around it: the search field, then the rows, the chosen one marked as
/// Windows 11 marks it.
fn paint_menu(c: &Canvas<'_>, t: &Theme, u: f32, look: &MenuLook) -> Result<()> {
    let m = SHADOW * u;
    let r = rect(m, m, look.w * u, look.h * u);
    c.shadow(r, 8.0 * u, 16.0 * u, 4.0 * u, Rgba::hex(0x000000, 0x50))?;
    c.fill_round(r, 8.0 * u, t.flyout)?;
    c.stroke_round(r, 8.0 * u, 1.0, t.stroke_strong)?;
    let mut y = r.top + MENU_PAD * u;
    if look.search_h > 0.0 {
        let field = rect(r.left + 6.0 * u, y + 2.0 * u, (look.w - 12.0) * u, 34.0 * u);
        c.fill_round(field, 4.0 * u, t.control)?;
        c.stroke_round(field, 4.0 * u, 1.0, t.control_stroke)?;
        let mid = f32::midpoint(field.top, field.bottom);
        let i = 16.0 * u;
        c.icon(icons::SEARCH, field.left + 10.0 * u, mid - i / 2.0, i, t.text_2)?;
        let tx = field.left + 34.0 * u;
        if look.search.is_empty() {
            c.text(
                &look.placeholder,
                rect(tx + 3.0 * u, field.top, field.right - tx - 3.0 * u, field.bottom - field.top),
                14.0 * u,
                t.text_3,
                Align::Leading,
            )?;
            c.fill_rect(rect(tx, mid - 8.0 * u, 1.0 * u, 16.0 * u), t.text)?;
        } else {
            let typed = c.gfx.paragraph(&look.search, 14.0 * u, false, 1.0e5)?;
            c.text_layout(&typed.layout, tx, mid - typed.height / 2.0, t.text)?;
            c.fill_rect(rect(tx + typed.width + 2.0 * u, mid - 8.0 * u, 1.0 * u, 16.0 * u), t.text)?;
        }
        y += look.search_h * u;
        c.fill_rect(rect(r.left, y - 4.0 * u, r.right - r.left, 1.0), t.divider)?;
    }
    if look.items.is_empty() {
        let row = rect(r.left + 16.0 * u, y, (look.w - 32.0) * u, ITEM_H * u);
        return c.text(&look.no_match, row, 13.0 * u, t.text_3, Align::Leading);
    }
    for (k, item) in look.items.iter().enumerate() {
        let i = look.first + k;
        let row = rect(r.left + 4.0 * u, y, (look.w - 8.0) * u, ITEM_H * u);
        let mid = f32::midpoint(row.top, row.bottom);
        if k > 0 && look.items[k - 1].group != item.group {
            c.fill_rect(rect(row.left + 8.0 * u, row.top, row.right - row.left - 16.0 * u, 1.0), t.divider)?;
        }
        if look.hover == Some(i) {
            c.fill_round(row, 4.0 * u, t.subtle_hover)?;
        } else if look.selected == Some(i) {
            c.fill_round(row, 4.0 * u, t.selected)?;
        }
        if look.selected == Some(i) {
            c.fill_round(rect(row.left, mid - 8.0 * u, 3.0 * u, 16.0 * u), 1.5 * u, t.accent)?;
        }
        let mut x = row.left + 12.0 * u;
        if item.mark != Mark::None {
            paint_mark(c, t, &item.mark, x, mid - 9.0 * u, 18.0 * u)?;
            x += 30.0 * u;
        }
        let mut right = row.right - 12.0 * u;
        if !item.detail.is_empty() {
            let detail = c.gfx.paragraph(&item.detail, 12.0 * u, false, 1.0e5)?;
            right -= detail.width;
            c.text_layout(&detail.layout, right, mid - detail.height / 2.0, t.text_3)?;
            right -= 12.0 * u;
        }
        c.text(
            &item.label,
            rect(x, row.top, (right - x).max(0.0), row.bottom - row.top),
            14.0 * u,
            t.text,
            Align::Leading,
        )?;
        y += ITEM_H * u;
    }
    // More rows than shown: where those shown are.
    if look.total > look.items.len() {
        let track = (look.h - 2.0 * MENU_PAD - look.search_h) * u;
        let top = r.top + (MENU_PAD + look.search_h) * u;
        let h = track * look.items.len() as f32 / look.total as f32;
        let y = top + track * look.first as f32 / look.total as f32;
        c.fill_round(rect(r.right - 6.0 * u, y + 2.0 * u, 3.0 * u, (h - 4.0 * u).max(8.0 * u)), 1.5 * u, t.text_3)?;
    }
    Ok(())
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
        let list = Piece::new(&gfx, false)?;
        for v in [&video, &title.visual, &bottom.visual, &playhead.visual, &clock.visual, &list.visual, &tip.visual] {
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
            list,
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
            options: RefCell::default(),
            choice: RefCell::default(),
            limit: Cell::new(None),
            menu: RefCell::default(),
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

    /// What the export row offers, and what is chosen in it.
    pub fn set_export(&self, options: ExportOptions, choice: ExportChoice) {
        *self.inner.options.borrow_mut() = options;
        *self.inner.choice.borrow_mut() = choice;
        self.inner.refresh();
    }

    pub fn export_choice(&self) -> ExportChoice {
        self.inner.choice.borrow().clone()
    }

    /// The longest part kept, in seconds (`None`: the whole clip): a longer one is shortened from its start, and the
    /// handles stop there.
    pub fn set_limit(&self, limit: Option<f64>) {
        self.inner.limit.set(limit);
        self.inner.set_state(|_| {});
        // The playhead goes into the part kept when it is left outside.
        let s = self.inner.state.get();
        if s.position < s.from || s.position > s.to {
            self.inner.seek(s.from);
        }
        self.inner.refresh();
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
        if progress.is_some() {
            self.inner.close_menu();
        }
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
        self.inner.act(Target::Minimise);
    }

    /// Ends [`Cutter::run`].
    pub fn close(&self) {
        self.inner.closed.set(true);
    }

    /// Shows the window and runs until [`Cutter::close`], handing the events to `host` and asking it what to do on
    /// every turn.
    pub fn run(&self, host: &mut dyn CutterHost) -> Result<()> {
        let inner = &self.inner;
        if inner.video_surface.borrow().is_none() {
            inner.paint_band()?;
        }
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

    fn preset(id: &str, name: &str, brand: &str, keywords: &str, detail: &str, height: u32, fps: u32) -> Preset {
        Preset {
            id: id.into(),
            name: name.into(),
            brand: brand.into(),
            keywords: keywords.into(),
            detail: detail.into(),
            codec: "h264".into(),
            height,
            fps,
        }
    }

    fn options() -> ExportOptions {
        ExportOptions {
            presets: vec![
                preset(ORIGINAL, "Original", "", "", "1440p60", 1440, 60),
                preset("discord", "Discord", "discord", "", "20 Mo · 720p30", 720, 30),
                preset("discord_nitro_basic", "Discord Nitro Basic", "discord", "", "50 Mo · 720p60", 720, 60),
                preset("discord_nitro", "Discord Nitro", "discord", "", "1 Go · 1080p60", 1080, 60),
                preset("x", "X", "x", "twitter", "2 min 20 s · 720p60", 720, 60),
                preset("x_premium", "X Premium", "x", "twitter", "1080p60", 1080, 60),
                preset("youtube", "YouTube", "youtube", "", "1080p60", 1080, 60),
                preset("tiktok", "TikTok", "tiktok", "", "10 min · 1080p60", 1080, 60),
                preset("bluesky", "Bluesky", "bluesky", "", "10 min · 300 Mo · 720p60", 720, 60),
                preset("reddit", "Reddit", "reddit", "", "15 min · 1 Go · 1080p60", 1080, 60),
                preset("instagram", "Instagram", "instagram", "", "3 min · 1080p30", 1080, 30),
                preset("threads", "Threads", "threads", "", "5 min · 1 Go · 1080p30", 1080, 30),
                preset("telegram", "Telegram", "telegram", "", "2 Go · 1080p60", 1080, 60),
            ],
            codecs: vec![
                Choice { id: "h264".into(), label: "H.264".into() },
                Choice { id: "hevc".into(), label: "HEVC".into() },
                Choice { id: "av1".into(), label: "AV1".into() },
            ],
            heights: vec![1440, 1080, 720, 480],
            fps: vec![60, 30],
        }
    }

    fn keys(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.key.as_str()).collect()
    }

    #[test]
    fn times_read_as_minutes_seconds_and_milliseconds() {
        assert_eq!(format_time(0.0), "0:00.000");
        assert_eq!(format_time(12.3456), "0:12.346");
        assert_eq!(format_time(75.5), "1:15.500");
        assert_eq!(format_time(3725.0), "1:02:05.000");
        assert_eq!(format_seconds(46.9), "0:46");
        assert_eq!(format_seconds(7_200.0), "2:00:00");
    }

    #[test]
    fn frames_are_found_by_time_and_key_frames_before_them() {
        let f = frames(90, 30.0, 30);
        assert_eq!(f.at(0.0), 0);
        assert_eq!(f.at(1.0), 30);
        assert_eq!(f.at(1.0 - 1e-6), 29);
        assert_eq!(f.at(99.0), 89);
        assert_eq!(f.first_at(1.0), 30);
        assert_eq!(f.first_at(1.0 - 1e-3), 30);
        assert_eq!(f.key_before(29), 0);
        assert_eq!(f.key_before(30), 30);
        assert_eq!(f.key_before(89), 60);
        assert!((f.duration() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn the_picture_keeps_its_proportions_in_its_card() {
        let l = Layout::new(1000.0, 800.0, 16.0 / 9.0);
        let (x, y, w, h) = l.video;
        assert!((w / h - 16.0 / 9.0).abs() < 1e-3);
        assert!(y >= TITLE_H && y + h <= l.bottom + 1e-3);
        assert!(x >= PAD && x + w <= 1000.0 - PAD + 1e-3);
        assert!((l.bottom - (800.0 - BOTTOM_H)).abs() < 1e-3);
    }

    #[test]
    fn the_window_leaves_a_margin_above_and_below_and_is_centred() {
        let work = PhysicalRect::new(0, 0, 2560, 1400);
        let PhysicalRect { x, y, width: w, height: h } = placement(work, 1.0, 16.0 / 9.0);
        assert!(y > 0 && y + h as i32 <= 1400);
        assert!((y - (1400 - (y + h as i32))).abs() <= 1, "the same margin above and below");
        assert!((x - (2560 - (x + w as i32))).abs() <= 1, "centred");
        // As wide as the picture needs at that height, with the margins of its card.
        let video_h = h as f32 - TITLE_H - BOTTOM_H;
        assert!((w as f32 - video_h * 16.0 / 9.0 - 2.0 * PAD).abs() <= 1.0);
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
            Target::Minimise,
            Target::Close,
            Target::Play,
            Target::Previous,
            Target::Next,
            Target::Mute,
            Target::Volume,
            Target::Preset,
            Target::Codec,
            Target::Height,
            Target::Fps,
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
        // The export row fits the narrowest window: nothing over the buttons.
        let l = Layout::new(MIN_W, 800.0, 16.0 / 9.0);
        let (fx, _, fw, _) = l.part(Target::Fps);
        assert!(fx + fw + 8.0 <= l.part(Target::SaveAs).0);
        let (cx, _, cw, _) = l.part(Target::Strip);
        assert!(cx > PAD && cx + cw < MIN_W - PAD);
        assert!(clock_at(&l).0 + 300.0 <= l.part(Target::Sign).0 + 1.0);
    }

    #[test]
    fn a_list_opens_over_its_button_and_its_rows_are_found() {
        let l = Layout::new(1200.0, 800.0, 16.0 / 9.0);
        let m = l.menu(MenuKind::Preset, 13);
        let (ax, ay, _, _) = l.part(Target::Preset);
        let (mx, my, mw, mh) = m.r;
        assert!((my + mh + MENU_GAP - ay).abs() < 1e-3, "just above its button");
        assert!(mx >= PAD && mx + mw <= 1200.0 - PAD && (mx - ax).abs() < 1e-3);
        assert!(my >= TITLE_H);
        assert_eq!(m.rows, 13);
        assert_eq!(menu_target(&m, 13, 0, mx + 20.0, my + 10.0), Some(Target::Search));
        let row = |k: f32| my + MENU_PAD + SEARCH_H + ITEM_H * k + ITEM_H / 2.0;
        assert_eq!(menu_target(&m, 13, 0, mx + 20.0, row(0.0)), Some(Target::Item(0)));
        assert_eq!(menu_target(&m, 13, 2, mx + 20.0, row(3.0)), Some(Target::Item(5)));
        assert_eq!(menu_target(&m, 13, 0, mx - 1.0, row(0.0)), None);
        // A short window: fewer rows, scrolled.
        let short = Layout::new(1200.0, 520.0, 16.0 / 9.0).menu(MenuKind::Preset, 13);
        assert!(short.rows < 13 && short.r.1 >= TITLE_H);
        // A list without search: as wide as its button at least.
        let codec = l.menu(MenuKind::Codec, 3);
        assert_eq!((codec.rows, codec.search), (3, 0.0));
        assert!(codec.r.2 >= CODEC_W);
    }

    #[test]
    fn the_networks_are_listed_in_order_and_found_by_name() {
        let o = options();
        let choice = ExportChoice { preset: "discord".into(), codec: "h264".into(), height: 720, fps: 30 };
        let all = preset_items(&o, &choice, "Custom", "");
        assert_eq!(all.len(), 13);
        assert_eq!(all[0].key, ORIGINAL);
        assert_eq!(all[1].key, "discord");
        assert_eq!(all[0].mark, Mark::Glyph(icons::VIDEO));
        assert_eq!(all[4].mark, Mark::Brand("x".into()));
        // Values set by hand: listed after the clip as it is.
        let custom = ExportChoice { preset: CUSTOM.into(), ..choice.clone() };
        let with = preset_items(&o, &custom, "Custom", "");
        assert_eq!(with[1].key, CUSTOM);
        assert_eq!(with[1].detail, "720p30 · H.264");
        // Searching: the start of the name first, then of a word or another name, case and accents aside.
        assert_eq!(keys(&preset_items(&o, &choice, "Custom", "nitro")), ["discord_nitro_basic", "discord_nitro"]);
        assert_eq!(keys(&preset_items(&o, &choice, "Custom", "TWIT")), ["x", "x_premium"]);
        assert_eq!(keys(&preset_items(&o, &choice, "Custom", "t"))[..3], ["tiktok", "threads", "telegram"]);
        assert_eq!(keys(&preset_items(&o, &choice, "Custom", "orîg")), [ORIGINAL]);
        assert!(preset_items(&o, &choice, "Custom", "myspace").is_empty());
        assert_eq!(fold("Équilibré"), "equilibre");
    }

    #[test]
    fn a_limit_holds_the_end_that_moved_or_brings_the_start_nearer() {
        let f = frames(600, 60.0, 60);
        let mut before = state(600);
        before.from = 100;
        before.to = 200;
        // The end dragged too far: it stops at the limit (two seconds: 120 frames).
        let mut s = before;
        s.to = 500;
        hold(&f, &mut s, &before, Some(2.0));
        assert_eq!((s.from, s.to), (100, 219));
        assert!(f.length(s.from, s.to) <= 2.0 + 1e-9);
        // The start dragged too far: it stops at the limit.
        let mut s = before;
        s.from = 0;
        hold(&f, &mut s, &before, Some(2.0));
        assert_eq!((s.from, s.to), (81, 200));
        // A limit set on the whole clip: the end stays, the start comes nearer.
        let whole = state(600);
        let mut s = whole;
        hold(&f, &mut s, &whole, Some(2.0));
        assert_eq!((s.from, s.to), (480, 599));
        // No limit, or within it: nothing moves.
        let mut s = whole;
        hold(&f, &mut s, &whole, None);
        assert_eq!((s.from, s.to), (0, 599));
        hold(&f, &mut s, &whole, Some(60.0));
        assert_eq!((s.from, s.to), (0, 599));
    }

    #[test]
    fn the_editor_draws_in_both_themes() {
        let frames = frames(1800, 60.0, 60);
        let texts = CutterTexts {
            title: "Replay 2026-10-15 21-04-11.mp4".into(),
            save: "Enregistrer".into(),
            save_as: "Enregistrer sous…".into(),
            cancel: "Annuler".into(),
            custom: "Personnalisé".into(),
            search: "Rechercher".into(),
            no_match: "Aucun résultat".into(),
            max: "max".into(),
            ..CutterTexts::default()
        };
        let gfx = Gfx::new().expect("gfx");
        let u = 1.25;
        let l = Layout::new(1100.0, 860.0, frames.aspect);
        let (w, h) = ((l.w * u).round() as u32, (l.h * u).round() as u32);
        // A picture: a gradient, as a game would show.
        let (pw, ph) = (320_u32, 180_u32);
        let picture: Vec<u8> = (0..ph)
            .flat_map(|y| (0..pw).flat_map(move |x| [(x * 255 / pw) as u8, (y * 255 / ph) as u8, 120, 255]))
            .collect();
        let o = options();
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
            let export = ExportLook {
                mark: Mark::Brand("discord".into()),
                preset: "Discord".into(),
                codec: "H.264".into(),
                height: 720,
                fps: 30,
            };
            let menu = dark.then_some(MenuKind::Preset);
            let look = BottomLook {
                state,
                hover: Target::Save,
                held: None,
                thumbs: 12,
                export: Some(export),
                menu,
                limit: Some(46.1),
            };
            let title = TitleLook { hover: Target::None, held: None };
            let th = (TITLE_H * u).ceil() as u32;
            let bh = (BOTTOM_H * u).ceil() as u32;
            let vh = h - th - bh;
            let top = gfx.render(w, th, |c| paint_title(c, &t, u, &l, &texts.title, &title)).expect("title");
            let video =
                gfx.render(w, vh, |c| paint_video(c, &t, u, &l, Some(&bitmap), (w as f32, vh as f32))).expect("video");
            let bottom = gfx
                .render(w, bh, |c| {
                    paint_bottom(c, &t, u, &l, &frames, &texts, &look, &thumbs)?;
                    let (cx, cy) = clock_at(&l);
                    paint_clock(c, &t, u, &frames, state.position, (cx * u, (cy - l.bottom) * u))?;
                    let (px, py) = playhead_at(&l, &frames, state.position);
                    paint_playhead(c, &t, u, (px * u, (py - l.bottom) * u))
                })
                .expect("bottom");
            let mut whole: Vec<u8> = [top, video, bottom].concat();
            assert_eq!(whole.len(), (w * h * 4) as usize);
            // The list of networks open over the video.
            if let Some(kind) = menu {
                let choice = ExportChoice { preset: "discord".into(), codec: "h264".into(), height: 720, fps: 30 };
                let items = preset_items(&o, &choice, &texts.custom, "");
                let m = l.menu(kind, items.len());
                let look = MenuLook {
                    w: m.r.2,
                    h: m.r.3,
                    search_h: m.search,
                    search: String::new(),
                    placeholder: texts.search.clone(),
                    no_match: texts.no_match.clone(),
                    selected: Some(1),
                    hover: Some(4),
                    first: 0,
                    total: items.len(),
                    items: items.into_iter().take(m.rows).collect(),
                };
                let (sw, sh) = (((m.r.2 + 2.0 * SHADOW) * u).ceil() as u32, ((m.r.3 + 2.0 * SHADOW) * u).ceil() as u32);
                let list = gfx.render(sw, sh, |c| paint_menu(c, &t, u, &look)).expect("list");
                let (ox, oy) = (((m.r.0 - SHADOW) * u).round() as i64, ((m.r.1 - SHADOW) * u).round() as i64);
                for y in 0..i64::from(sh) {
                    for x in 0..i64::from(sw) {
                        let (dx, dy) = (ox + x, oy + y);
                        if dx < 0 || dy < 0 || dx >= i64::from(w) || dy >= i64::from(h) {
                            continue;
                        }
                        let s = ((y * i64::from(sw) + x) * 4) as usize;
                        let d = ((dy * i64::from(w) + dx) * 4) as usize;
                        let a = f32::from(list[s + 3]) / 255.0;
                        for k in 0..4 {
                            let v = f32::from(list[s + k]) + f32::from(whole[d + k]) * (1.0 - a);
                            whole[d + k] = v.round().min(255.0) as u8;
                        }
                    }
                }
            }
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
