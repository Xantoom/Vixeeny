// SPDX-License-Identifier: GPL-3.0-or-later
//! The editor over the frozen screens: one window per monitor, all showing the same session. Input goes to the
//! [`Session`]; after each input the visuals follow its [`View`] and one commit hands the changes to the compositor.
//! Nothing is drawn while nothing changes.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use vixeeny_editor::{BarSide, Color, Command, CursorHint, Key, KeyInput, Modifiers, Point, Rect, Session, Tool, View};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_BACK,
    VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, MSG, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE, SetCursor,
    SetForegroundWindow, SetTimer, ShowWindow, TranslateMessage, WM_APP, WM_CAPTURECHANGED, WM_CHAR, WM_CLOSE,
    WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WM_SYSKEYDOWN, WM_TIMER,
};

use crate::drawing::{Drawing, ScreenPart};
use crate::gfx::{self, Gfx};
use crate::layout::{self, BAR_H, BUTTON, Button, PanelForm, PanelHit, Picks, SHADOW, Tip};
use crate::paint::{self, BarLook, FieldLook, PanelLook};
use crate::scene::{self, Glow, Pane, Solids, TipLook};
use crate::theme::{Look, Rgba, Theme, hsv_to_rgb, rgb_to_hsv};
use crate::window::{self, Pointer};

/// One window of the overlay: a monitor, and the part of the frozen image it shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    /// Where the window goes, physical pixels of the virtual desktop.
    pub position: (i32, i32),
    /// Its size, physical pixels.
    pub size: (u32, u32),
    /// The part of the image it shows (image pixels; the whole image for a scrolling one).
    pub area: Rect,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct OverlayError(#[from] windows::core::Error);

/// Returns `true` when the overlay should close afterwards.
type CommandHandler = dyn Fn(Command, &Session) -> bool;
type FirstFrame = Box<dyn FnOnce(Vec<u64>)>;
/// A fresh picture of a part of the screens waits in [`Screens::take`].
const WM_LIVE: u32 = WM_APP + 0x20;

/// The screens as they are now, which the host captures: the editor shows them live.
pub trait Screens {
    /// A picture of every screen, the editor's windows hidden (or left out).
    fn grab(&self) -> Option<vixeeny_editor::RgbaImage>;
    /// Follows `area` (whole-image pixels: x, y, width, height) of the screens, the editor's windows left out, calling
    /// [`Wake::wake`] each time a new picture of it waits in [`Screens::take`]. The pictures stay on `device`, the
    /// editor's. `None`: stops.
    fn watch(&self, area: Option<(u32, u32, u32, u32)>, wake: Wake, device: &ID3D11Device);
    /// The newest pictures of the area followed.
    fn take(&self) -> Vec<ScreenPart>;
}

/// Tells the editor, from any thread, that new pictures of the screens wait.
#[derive(Clone)]
pub struct Wake {
    hwnd: usize,
    pending: Arc<AtomicBool>,
}

impl Wake {
    pub fn wake(&self) {
        // One message at a time: the editor takes every picture waiting when it comes.
        if !self.pending.swap(true, Ordering::AcqRel) {
            post(self.hwnd as u64, WM_LIVE, 0, 0);
        }
    }
}

/// `WM_MOUSELEAVE` (declared with the common controls in the bindings).
const WM_MOUSELEAVE: u32 = 0x02a3;
const TIP_TIMER: usize = 1;
const CARET_TIMER: usize = 2;
const CARET_BLINK_MS: u32 = 530;
/// The size ring stays this long after the wheel last turned.
const BRUSH_TIMER: usize = 3;
const BRUSH_MS: u32 = 700;
/// Live pictures of the screens are shown at most once per this (60 a second): one that comes sooner waits for the rest
/// of it.
const LIVE_TIMER: usize = 4;
/// A tooltip's surface size and its bubble's (see [`paint::tip_size`]).
type TipSize = ((u32, u32), (f32, f32));
const LIVE_PERIOD: std::time::Duration = std::time::Duration::from_micros(16_667);
/// The opacity of a toolbar inside the zone while an annotation is drawn under it.
const FADED: f32 = 0.12;

/// What a press started.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Press {
    /// A toolbar button, clicked when released over it.
    Button(usize),
    /// A control of the style panel, clicked when released over it.
    Panel(PanelHit),
    /// Dragging in the colour picker.
    Picking(PanelHit),
    /// The zone or an annotation (the session's gesture).
    Image,
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hover {
    Button(usize),
    Bar,
    Panel(PanelHit),
}

#[derive(Default)]
struct Ui {
    hover: Option<Hover>,
    press: Option<Press>,
    style_open: bool,
    picker: bool,
    hsv: (f32, f32, f32),
    /// What the two-tool buttons take.
    picks: Picks,
    /// The size ring is shown (the wheel just turned).
    brush: bool,
    /// The tip shown, and the button waiting for one.
    tip: Option<usize>,
    tip_pending: Option<usize>,
    /// The text being typed and the caret (byte index).
    text: String,
    caret: usize,
    caret_on: bool,
    typing: bool,
    /// A high surrogate waiting for its pair (WM_CHAR gives UTF-16 units).
    surrogate: Option<u16>,
    scroll: f32,
    /// The pane under the pointer, and what the pointer looks like there.
    pointer_pane: Option<usize>,
    pointer: Option<Pointer>,
    tracking: Vec<bool>,
    /// The window whose timer blinks the caret.
    caret_window: Option<HWND>,
}

struct Shared {
    gfx: Rc<Gfx>,
    theme: Theme,
    u: f32,
    solids: Solids,
    session: RefCell<Session>,
    /// The session as the windows show it now: what the pointer finds under it.
    shown: RefCell<Option<Rc<View>>>,
    ui: RefCell<Ui>,
    panes: Vec<Pane>,
    handler: Box<CommandHandler>,
    tips: RefCell<Vec<String>>,
    /// Shows the screens live; without them the screens stay frozen.
    screens: RefCell<Option<Box<dyn Screens>>>,
    /// The editor's windows are left out of captures (live).
    excluded: Cell<bool>,
    /// The part of the screens followed, and its wake.
    watched: Cell<Option<(u32, u32, u32, u32)>>,
    wake: Wake,
    /// The newest picture of each part followed.
    live_parts: RefCell<Vec<ScreenPart>>,
    /// The size label's and the tooltip's last text, with their measured sizes.
    label_size: RefCell<Option<(String, (u32, u32))>>,
    tip_size: RefCell<Option<(String, TipSize)>>,
    /// When live pictures were last shown, and whether newer ones wait for [`LIVE_TIMER`].
    live_shown: Cell<Option<std::time::Instant>>,
    live_deferred: Cell<bool>,
    /// The annotations as the graphics card draws them.
    drawing: RefCell<Option<Drawing>>,
    scrolling: Cell<bool>,
    /// A command is running (a dialog may be up): input waits.
    busy: Cell<bool>,
    /// The windows are on screen (not headless).
    visible: Cell<bool>,
    /// The windows stepped aside for the command running (see [`step_aside`]).
    aside: Cell<bool>,
    closing: Cell<bool>,
}

thread_local! {
    static CURRENT: RefCell<Option<Weak<Shared>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let shared = CURRENT.with(|c| c.borrow().as_ref().and_then(Weak::upgrade));
    if let Some(shared) = shared
        && let Some(i) = shared.panes.iter().position(|p| p.hwnd == hwnd)
        && let Some(result) = shared.message(i, msg, wparam, lparam)
    {
        return result;
    }
    window::default_proc(hwnd, msg, wparam, lparam)
}

fn low_word(v: isize) -> i32 {
    i32::from(v as u16 as i16)
}

fn high_word(v: isize) -> i32 {
    i32::from((v >> 16) as u16 as i16)
}

fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    // SAFETY: plain call.
    unsafe { GetKeyState(i32::from(vk.0)) < 0 }
}

fn color_of_hsv(h: f32, s: f32, v: f32) -> Color {
    let (r, g, b) = hsv_to_rgb(h, s, v);
    let byte = |c: f32| (c * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::rgb(byte(r), byte(g), byte(b))
}

impl Shared {
    fn can_live(&self) -> bool {
        self.screens.borrow().is_some() && !self.scrolling.get()
    }

    /// The tip of button `k`; a two-tool button adds what a right-click takes.
    fn tip_text(&self, k: usize, picks: &Picks, live: bool) -> Option<String> {
        let tips = self.tips.borrow();
        let (tip, other) = layout::tip_of(k, picks, live);
        let text = tips.get(tip as usize)?;
        let Some(other) = other.and_then(|o| tips.get(o as usize)) else {
            return Some(text.clone());
        };
        let right = tips.get(Tip::RightClick as usize).map_or("", String::as_str);
        let mut chars = other.chars();
        let other: String = chars.next().map(|c| c.to_lowercase().chain(chars).collect()).unwrap_or_default();
        Some(format!("{text} · {right}{other}"))
    }

    fn bar_look(&self, view: &View, ui: &Ui, side: BarSide) -> BarLook {
        BarLook {
            tool: view.tool,
            picks: ui.picks,
            upright: side.upright(),
            color: view.color,
            live: view.live,
            can_live: self.can_live(),
            can_undo: view.can_undo,
            can_redo: view.can_redo,
            can_clear: view.can_clear,
            style_open: ui.style_open,
            hover: match ui.hover {
                Some(Hover::Button(k)) => Some(k),
                _ => None,
            },
            pressed: match ui.press {
                Some(Press::Button(k)) => Some(k),
                _ => None,
            },
        }
    }

    fn panel_look(view: &View, ui: &Ui) -> PanelLook {
        PanelLook {
            color: view.color,
            recent: view.recent_colors.clone(),
            width: view.width,
            max_width: view.max_width,
            size: view.brush.unwrap_or(view.width),
            filled: view.filled,
            tool: view.tool,
            form: PanelForm::of(view.tool, ui.picker),
            hsv: ui.hsv,
            eyedropper: view.tool == Some(Tool::Eyedropper),
            hover: match (ui.press, ui.hover) {
                (Some(Press::Picking(_)), _) => None,
                (_, Some(Hover::Panel(hit))) => Some(hit),
                _ => None,
            },
        }
    }

    /// Where the bar itself (not its shadow) is in `pane`, window pixels, and on which side of the zone; `None` when
    /// the pane does not show it.
    fn bar_in(&self, pane: &Pane, view: &View, scroll: f32) -> Option<(f32, f32, BarSide)> {
        let place = view.toolbar?;
        let u = self.u;
        let (x, y, side) = if self.scrolling.get() {
            // The zone is the whole long image: keep the bar in view, top right.
            let w = layout::BAR_W * u;
            ((pane.size.0 as f32 - w - 16.0 * u).max(0.0), 16.0 * u, BarSide::Inside)
        } else {
            let (x, y) = pane.to_window(place.at.x, place.at.y, scroll);
            (x, y, place.side)
        };
        let (w, h) = layout::bar_size(side.upright());
        let (w, h) = (w * u, h * u);
        let visible = x < pane.size.0 as f32 && x + w > 0.0 && y < pane.size.1 as f32 && y + h > 0.0;
        visible.then_some((x, y, side))
    }

    fn panel_in(&self, pane: &Pane, view: &View, ui: &Ui) -> Option<(f32, f32)> {
        if !ui.style_open {
            return None;
        }
        let u = self.u;
        let (bx, by, side) = self.bar_in(pane, view, ui.scroll)?;
        let upright = side.upright();
        let height = PanelForm::of(view.tool, ui.picker).height();
        let (pw, ph) = (pane.size.0 as f32, pane.size.1 as f32);
        // The room before and after the bar, across it.
        let room = if upright { (bx / u, (pw - bx) / u - BAR_H) } else { (by / u, (ph - by) / u - BAR_H) };
        let before_first = matches!(side, BarSide::Above | BarSide::Left);
        let (dx, dy) = layout::panel_offset(upright, before_first, room, height);
        let x = (bx + dx * u).clamp(0.0, (pw - layout::PANEL_W * u).max(0.0));
        let y = (by + dy * u).clamp(0.0, (ph - height * u).max(0.0));
        Some((x, y))
    }

    fn field_look(view: &View, ui: &Ui) -> Option<FieldLook> {
        view.text_input?;
        Some(FieldLook {
            text: ui.text.clone(),
            caret: ui.text[..ui.caret].encode_utf16().count(),
            caret_on: ui.caret_on,
            size: view.text_size,
            color: view.color,
        })
    }

    /// Makes every window show the session as it is now, then commits.
    fn refresh(&self) {
        if let Err(e) = self.update() {
            tracing::warn!("editor refresh: {e}");
        }
    }

    /// The size of the size label showing `text`, measured when the text changes.
    fn label_size(&self, text: &str) -> windows::core::Result<(u32, u32)> {
        let mut cached = self.label_size.borrow_mut();
        if let Some((was, size)) = cached.as_ref()
            && was == text
        {
            return Ok(*size);
        }
        let size = paint::label_size(&self.gfx, self.u, text)?;
        *cached = Some((text.to_owned(), size));
        Ok(size)
    }

    /// The sizes of the tooltip showing `text` (see [`paint::tip_size`]), measured when the text changes.
    fn tip_size(&self, text: &str) -> windows::core::Result<TipSize> {
        let mut cached = self.tip_size.borrow_mut();
        if let Some((was, size)) = cached.as_ref()
            && was == text
        {
            return Ok(*size);
        }
        let size = paint::tip_size(&self.gfx, self.u, text)?;
        *cached = Some((text.to_owned(), size));
        Ok(size)
    }

    /// The view the windows show (computed once per change, see [`Self::update`]).
    fn shown(&self) -> Rc<View> {
        if let Some(view) = self.shown.borrow().as_ref() {
            return view.clone();
        }
        let view = Rc::new(self.session.borrow().view());
        *self.shown.borrow_mut() = Some(view.clone());
        view
    }

    fn update(&self) -> windows::core::Result<()> {
        let view = Rc::new(self.session.borrow().view());
        *self.shown.borrow_mut() = Some(view.clone());
        self.sync_typing(&view);
        if let Err(e) = self.sync_drawing(&view) {
            tracing::warn!("annotations not drawn: {e}");
            *self.drawing.borrow_mut() = None;
        }
        let ui = self.ui.borrow();
        for pane in &self.panes {
            self.update_pane(pane, &view, &ui)?;
        }
        drop(ui);
        self.update_pointer(view.cursor);
        // SAFETY: plain call.
        unsafe { self.gfx.dcomp.Commit()? };
        // Make live/frozen changes visible before opening or closing capture sessions for the effects.
        self.follow(&view);
        Ok(())
    }

    /// Draws the annotations again where they changed, over the screens under them.
    fn sync_drawing(&self, view: &View) -> windows::core::Result<()> {
        let mut slot = self.drawing.borrow_mut();
        let Some(area) = view.layers_area.map(|a| self.bounded(a)) else {
            *slot = None;
            return Ok(());
        };
        let session = self.session.borrow();
        match slot.as_mut() {
            Some(d) if d.area == area => {
                if d.base_version != view.base_version {
                    d.set_screens(&self.gfx, session.base(), view.base_version)?;
                }
            }
            _ => {
                let mut d = Drawing::new(&self.gfx, area, session.base(), view.base_version)?;
                for part in self.live_parts.borrow().iter() {
                    d.feed(&self.gfx, part)?;
                }
                *slot = Some(d);
            }
        }
        drop(session);
        if let Some(d) = slot.as_mut() {
            d.update(&self.gfx, &view.layers, view.live)?;
        }
        Ok(())
    }

    /// `area` cut to the largest bitmap the graphics card takes.
    fn bounded(&self, (x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
        // SAFETY: plain call.
        let most = unsafe { self.gfx.dc.GetMaximumBitmapSize() }.max(1);
        (x, y, w.min(most), h.min(most))
    }

    /// Opens or closes the text field with the session's, and runs its caret.
    fn sync_typing(&self, view: &View) {
        let typing = view.text_input.is_some();
        let mut ui = self.ui.borrow_mut();
        if typing == ui.typing {
            return;
        }
        ui.typing = typing;
        ui.text.clear();
        ui.caret = 0;
        ui.caret_on = true;
        // SAFETY: plain timer calls on windows of this thread.
        unsafe {
            if let Some(hwnd) = ui.caret_window.take() {
                let _ = KillTimer(Some(hwnd), CARET_TIMER);
            }
            if typing {
                let hwnd = self.active_hwnd(&ui);
                SetTimer(Some(hwnd), CARET_TIMER, CARET_BLINK_MS, None);
                ui.caret_window = Some(hwnd);
            }
        }
    }

    fn active_hwnd(&self, ui: &Ui) -> HWND {
        let i = ui.pointer_pane.unwrap_or(0).min(self.panes.len() - 1);
        self.panes[i].hwnd
    }

    fn update_pane(&self, pane: &Pane, view: &View, ui: &Ui) -> windows::core::Result<()> {
        let u = self.u;
        let gfx = &self.gfx;
        let t = &self.theme;
        let scroll = ui.scroll;
        let content_at = (-pane.area.x, -pane.area.y - scroll);
        if pane.content_at.replace(Some(content_at)) != Some(content_at) {
            scene::set_offset(&pane.content, content_at.0, content_at.1)?;
        }
        // Live, the frozen picture goes out of sight: the windows show the screens beneath them.
        if pane.frozen_away.replace(view.live) != view.live {
            let away = if view.live { -1.0e6 } else { 0.0 };
            scene::set_offset(&pane.frozen.visual, away, away)?;
        }
        let area = pane.area_px();
        let zone = view.selection.as_ref().map(scene::snap);

        // The veil, and the zone's border and handles.
        for (v, r) in pane.veil.iter().zip(scene::veil_rects(area, zone)) {
            scene::place(v, r)?;
        }
        let border = zone.map_or([(0.0, 0.0, 0.0, 0.0); 4], |z| scene::border_rects(z, 2.0));
        for (v, r) in pane.border.iter().zip(border) {
            scene::place(v, r)?;
        }
        // The selected annotation, outlined just around it.
        let outline = view.selected.map_or([(0.0, 0.0, 0.0, 0.0); 4], |r| {
            let r = Rect::new(r.x - 3.0, r.y - 3.0, r.w + 6.0, r.h + 6.0);
            scene::border_rects(scene::snap(&r), 1.0)
        });
        for (v, r) in pane.outline.iter().zip(outline) {
            scene::place(v, r)?;
        }
        let handles = zone.filter(|z| view.settled && pane.sees(*z, 20.0 * u));
        if pane.handles_at.replace(handles) != handles {
            let half = self.solids.handle_side as f32 / 2.0;
            let centres = handles.map(scene::handle_centres);
            for (i, v) in pane.handles.iter().enumerate() {
                let (x, y) = centres.map_or((-1.0e6, -1.0e6), |c| (c[i].0 - half, c[i].1 - half));
                scene::set_offset(v, x.round(), y.round())?;
            }
        }

        // The size label.
        match &view.size_label {
            Some((text, at)) if zone.is_some_and(|z| pane.sees(z, 40.0 * u)) => {
                let size = self.label_size(text)?;
                pane.label.show(gfx, text, || Ok(size), |c| paint::size_label(c, t, u, text, size))?;
                pane.label.move_to(at.x.round(), at.y.round())?;
            }
            _ => pane.label.hide()?,
        }

        // The annotations.
        let drawing = self.drawing.borrow();
        let shown = drawing.as_ref().filter(|d| {
            let (x, y, w, h) = d.area;
            view.layers_area.is_some() && pane.sees((x as f32, y as f32, w as f32, h as f32), 0.0)
        });
        match shown {
            Some(d) => {
                let (x, y, w, h) = d.area;
                pane.drawing.show(
                    gfx,
                    &(d.version, view.live),
                    || Ok((w, h)),
                    |c| {
                        d.draw(c, (0.0, 0.0), view.live);
                        Ok(())
                    },
                )?;
                pane.drawing.move_to(x as f32, y as f32)?;
            }
            None => pane.drawing.hide()?,
        }
        drop(drawing);

        // The text field.
        match (view.text_input, Self::field_look(view, ui)) {
            (Some(at), Some(look)) => {
                pane.field.show(gfx, &look, || paint::field_size(gfx, &look), |c| paint::field(c, &look))?;
                pane.field.move_to(at.x.round(), at.y.round())?;
            }
            _ => pane.field.hide()?,
        }

        // The size ring, around the pointer while the wheel changes the size.
        match view.brush.filter(|_| ui.brush) {
            Some(size) => {
                let diameter = size.round().max(2.0) as u32;
                let side = paint::brush_side(diameter) as f32;
                let (x, y) = (view.pointer.x - side / 2.0, view.pointer.y - side / 2.0);
                if pane.sees((x, y, side, side), 0.0) {
                    pane.brush.show(
                        gfx,
                        &diameter,
                        || Ok((side as u32, side as u32)),
                        |c| paint::brush(c, diameter),
                    )?;
                    pane.brush.move_to(x.round(), y.round())?;
                } else {
                    pane.brush.hide()?;
                }
            }
            None => pane.brush.hide()?,
        }

        // The scroll bar of a long image.
        let image_h = self.session.borrow().base().height as f32;
        let view_h = pane.size.1 as f32;
        if self.scrolling.get() && image_h > view_h {
            let h = (24.0 * u).max(view_h * view_h / image_h);
            let y = scroll / image_h * view_h;
            scene::place(&pane.scrollbar, (pane.size.0 as f32 - 6.0 * u, y, 4.0 * u, h))?;
        } else {
            scene::place(&pane.scrollbar, (0.0, 0.0, 0.0, 0.0))?;
        }

        // The toolbar, its tip and the style panel.
        let margin = SHADOW * u;
        match self.bar_in(pane, view, scroll) {
            Some((bx, by, side)) => {
                let lit = self.bar_look(view, ui, side);
                // The button under the pointer is lit by a layer of its own: the bar is not drawn again for it.
                let look = BarLook { hover: None, ..lit.clone() };
                let upright = side.upright();
                pane.toolbar.show(
                    gfx,
                    &look,
                    || Ok(paint::bar_surface(u, upright)),
                    |c| paint::toolbar(c, t, u, &look),
                )?;
                if pane.toolbar.move_to((bx - margin).round(), (by - margin).round())? {
                    pane.toolbar.appear(gfx, 6.0 * u, 0.18, t.animations)?;
                }
                let at = lit.hover.filter(|k| paint::bar_lights(&lit, *k)).map(|k| {
                    let (x, y) = layout::button_xy(k, upright);
                    (k, paint::glow_at(u, (margin + x * u, margin + y * u)))
                });
                Self::light(gfx, t, u, &pane.bar_glow, at)?;
                // Inside the zone, the bar steps aside while an annotation is drawn.
                let faded = side == BarSide::Inside && view.drawing;
                pane.toolbar.fade(gfx, if faded { FADED } else { 1.0 }, t.animations)?;
                let tip = ui.tip.filter(|_| !faded).and_then(|k| Some((k, self.tip_text(k, &ui.picks, view.live)?)));
                match tip {
                    Some((k, text)) if !text.is_empty() => {
                        let ((w, h), bubble) = self.tip_size(&text)?;
                        let look = TipLook { button: k, text };
                        pane.tip.show(gfx, &look, || Ok((w, h)), |c| paint::tip(c, t, u, &look.text, bubble))?;
                        let (x, y) = layout::button_xy(k, upright);
                        let (cx, cy) = (bx + (x + BUTTON / 2.0) * u, by + (y + BUTTON / 2.0) * u);
                        let gap = (BUTTON / 2.0 + 14.0) * u;
                        let pw = pane.size.0 as f32;
                        // Beside an upright bar, on the side away from the zone if it fits.
                        let right = cx + gap;
                        let left = cx - gap - bubble.0;
                        let (left, top) = match side {
                            BarSide::Right if right + bubble.0 <= pw || left < 0.0 => (right, cy - bubble.1 / 2.0),
                            BarSide::Left if left >= 0.0 || right + bubble.0 > pw => (left, cy - bubble.1 / 2.0),
                            BarSide::Right => (left, cy - bubble.1 / 2.0),
                            BarSide::Left => (right, cy - bubble.1 / 2.0),
                            // Above the bar, unless the screen's top edge is in the way.
                            BarSide::Above if by >= 48.0 * u => (cx - bubble.0 / 2.0, cy - gap - bubble.1),
                            _ => (cx - bubble.0 / 2.0, cy + gap),
                        };
                        let left = left.clamp(0.0, (pw - bubble.0).max(0.0));
                        let m = paint::TIP_MARGIN * u;
                        if pane.tip.glide_to(gfx, (left - m).round(), (top - m).round(), 0.1, t.animations)? {
                            pane.tip.appear(gfx, 0.0, 0.1, t.animations)?;
                        }
                    }
                    _ => pane.tip.hide()?,
                }
            }
            None => {
                pane.toolbar.hide()?;
                Self::light(gfx, t, u, &pane.bar_glow, None)?;
                pane.tip.hide()?;
            }
        }
        match self.panel_in(pane, view, ui) {
            Some((px, py)) => {
                let lit = Self::panel_look(view, ui);
                let at = match lit.hover {
                    Some(PanelHit::Eyedropper) if !lit.eyedropper => Some(layout::EYEDROPPER),
                    Some(PanelHit::Fill) if lit.form.fill => Some(layout::FILL),
                    _ => None,
                };
                let at =
                    at.zip(lit.hover).map(|((x, y), hit)| (hit, paint::glow_at(u, (margin + x * u, margin + y * u))));
                // Only the size's knob changes under the pointer; the buttons are lit by a layer of their own.
                let look = PanelLook { hover: lit.hover.filter(|h| *h == PanelHit::Size), ..lit };
                Self::light(gfx, t, u, &pane.panel_glow, at)?;
                pane.panel.show(gfx, &look, || Ok(paint::panel_surface(u)), |c| paint::panel(c, t, u, &look))?;
                let above = py < self.bar_in(pane, view, scroll).map_or(0.0, |b| b.1);
                if pane.panel.move_to((px - margin).round(), (py - margin).round())? {
                    pane.panel.appear(gfx, if above { 4.0 * u } else { -4.0 * u }, 0.1, t.animations)?;
                }
            }
            None => pane.panel.hide()?,
        }
        Ok(())
    }

    /// Lights the button of `glow` at `at` (its key, and [`paint::glow_at`]), or none.
    fn light<K: Copy + PartialEq>(
        gfx: &Gfx,
        t: &Theme,
        u: f32,
        glow: &Glow<K, ()>,
        at: Option<(K, paint::GlowAt)>,
    ) -> windows::core::Result<()> {
        let (lit, (offset, size)) = match at {
            Some((k, at)) => (Some((k, &(), at.place)), (at.offset, at.size)),
            None => (None, ((0.0, 0.0), (1, 1))),
        };
        glow.set(gfx, lit, || Ok(size), |c| paint::glow(c, t, u, offset), t.animations)
    }

    /// The pointer's shape where it is.
    fn update_pointer(&self, cursor: CursorHint) {
        let mut ui = self.ui.borrow_mut();
        let shape = match (ui.press, ui.hover) {
            (Some(Press::Button(_) | Press::Panel(_)), _) | (None, Some(Hover::Button(_))) => Pointer::Hand,
            (None, Some(Hover::Panel(hit))) if hit != PanelHit::Background => Pointer::Hand,
            (Some(Press::Picking(_)), _) | (None, Some(Hover::Bar | Hover::Panel(_))) => Pointer::Arrow,
            _ => Pointer::Zone(cursor),
        };
        if ui.pointer != Some(shape) && ui.pointer_pane.is_some() {
            ui.pointer = Some(shape);
            // SAFETY: plain call with a system cursor.
            unsafe { SetCursor(Some(window::cursor(shape))) };
        }
    }

    /// The toolbar or panel control at `(x, y)` (window pixels of `pane`).
    fn hit(&self, pane: &Pane, x: f32, y: f32) -> Option<Hover> {
        let view = self.shown();
        let ui = self.ui.borrow();
        let u = self.u;
        if let Some((px, py)) = self.panel_in(pane, &view, &ui) {
            let recent = view.recent_colors.len();
            if let Some(hit) = layout::panel_at(
                (x - px) / u,
                (y - py) / u,
                Color::PALETTE.len(),
                recent,
                &PanelForm::of(view.tool, ui.picker),
            ) {
                return Some(Hover::Panel(hit));
            }
        }
        let (bx, by, side) = self.bar_in(pane, &view, ui.scroll)?;
        // A bar that stepped aside for a drawing is not in the way.
        if side == BarSide::Inside && view.drawing {
            return None;
        }
        let upright = side.upright();
        let (w, h) = layout::bar_size(upright);
        let (lx, ly) = ((x - bx) / u, (y - by) / u);
        if !(0.0..w).contains(&lx) || !(0.0..h).contains(&ly) {
            return None;
        }
        Some(layout::button_at(lx, ly, upright).map_or(Hover::Bar, Hover::Button))
    }

    fn message(&self, i: usize, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let pane = &self.panes[i];
        match msg {
            WM_PAINT => {
                // SAFETY: plain call; the content is DirectComposition's, nothing to paint.
                let _ = unsafe { ValidateRect(Some(pane.hwnd), None) };
                return Some(LRESULT(0));
            }
            WM_ERASEBKGND => return Some(LRESULT(1)),
            // The window keeps the size of its monitor whatever the DPI.
            WM_DPICHANGED => return Some(LRESULT(0)),
            WM_SETCURSOR if low_word(lparam.0) == 1 => {
                // HTCLIENT: the shape follows the editor.
                let shape = self.ui.borrow().pointer.unwrap_or(Pointer::Zone(CursorHint::Crosshair));
                // SAFETY: plain call with a system cursor.
                unsafe { SetCursor(Some(window::cursor(shape))) };
                return Some(LRESULT(1));
            }
            WM_LIVE => {
                self.screens_changed();
                return Some(LRESULT(0));
            }
            WM_CLOSE => {
                self.run_command(Command::Close);
                return Some(LRESULT(0));
            }
            // Another window took the mouse (Alt+Tab, the Start menu…) in the middle of a press: its release will not
            // come here.
            WM_CAPTURECHANGED => {
                self.lost_mouse();
                return Some(LRESULT(0));
            }
            _ => {}
        }
        if self.busy.get() || self.closing.get() {
            return None;
        }
        let x = low_word(lparam.0) as f32;
        let y = high_word(lparam.0) as f32;
        match msg {
            WM_MOUSEMOVE => self.pointer_moved(i, x, y),
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => self.pointer_pressed(i, x, y),
            WM_LBUTTONUP => self.pointer_released(i, x, y),
            WM_RBUTTONDOWN => self.right_click(i, x, y),
            WM_MOUSELEAVE => {
                let mut ui = self.ui.borrow_mut();
                if let Some(t) = ui.tracking.get_mut(i) {
                    *t = false;
                }
                if ui.press.is_none() && ui.pointer_pane == Some(i) {
                    ui.hover = None;
                    ui.tip = None;
                    ui.pointer_pane = None;
                    drop(ui);
                    self.refresh();
                }
            }
            WM_MOUSEWHEEL => {
                let delta = high_word(wparam.0 as isize) as f32;
                // A long image scrolls; Ctrl, or any other image, sizes the tool.
                if self.scrolling.get() && !key_down(VK_CONTROL) {
                    self.scroll_by(-delta);
                } else {
                    self.wheel(i, delta);
                }
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(wparam.0 as u16);
                if !self.key(vk, lparam.0 & (1 << 30) != 0) && msg == WM_SYSKEYDOWN {
                    return None; // Alt+F4 and the like
                }
            }
            WM_CHAR => self.typed(wparam.0 as u16),
            WM_TIMER => self.timer(i, wparam.0),
            _ => return None,
        }
        Some(LRESULT(0))
    }

    fn track_leave(&self, i: usize) {
        let mut ui = self.ui.borrow_mut();
        if ui.tracking.len() < self.panes.len() {
            ui.tracking.resize(self.panes.len(), false);
        }
        if !ui.tracking[i] {
            ui.tracking[i] = true;
            let mut t = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.panes[i].hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: `t` is valid for the call.
            let _ = unsafe { TrackMouseEvent(&mut t) };
        }
    }

    fn pointer_moved(&self, i: usize, x: f32, y: f32) {
        self.track_leave(i);
        let pane = &self.panes[i];
        let press = self.ui.borrow().press;
        match press {
            Some(Press::Picking(hit)) => {
                self.pick(pane, hit, x, y);
            }
            Some(Press::Button(_) | Press::Panel(_)) => {
                let hover = self.hit(pane, x, y);
                self.ui.borrow_mut().hover = hover;
            }
            Some(Press::Image) => {
                let scroll = self.ui.borrow().scroll;
                let (ix, iy) = pane.to_image(x, y, scroll);
                let shift = key_down(VK_SHIFT);
                self.session.borrow_mut().pointer_move(Point::new(ix, iy), Modifiers { shift });
            }
            None => {
                let hover = self.hit(pane, x, y);
                let mut ui = self.ui.borrow_mut();
                ui.pointer_pane = Some(i);
                let changed = ui.hover != hover;
                if changed {
                    ui.hover = hover;
                    ui.tip = None;
                    ui.tip_pending = None;
                    if let Some(Hover::Button(k)) = hover {
                        ui.tip_pending = Some(k);
                        // SAFETY: plain timer call on a window of this thread.
                        unsafe { SetTimer(Some(pane.hwnd), TIP_TIMER, layout::TIP_DELAY_MS, None) };
                    }
                }
                let scroll = ui.scroll;
                drop(ui);
                let mut moved = false;
                if hover.is_none() {
                    let (ix, iy) = pane.to_image(x, y, scroll);
                    let shift = key_down(VK_SHIFT);
                    let mut session = self.session.borrow_mut();
                    let before = session.hover_state();
                    session.pointer_move(Point::new(ix, iy), Modifiers { shift });
                    moved = session.hover_state() != before || session.in_gesture();
                }
                // A hover that changes nothing on screen costs nothing (the size ring follows the pointer).
                if !changed && !moved && !self.ui.borrow().brush {
                    let cursor = self.session.borrow().cursor_hint();
                    self.update_pointer(cursor);
                    return;
                }
            }
        }
        self.refresh();
    }

    fn pointer_pressed(&self, i: usize, x: f32, y: f32) {
        let pane = &self.panes[i];
        // SAFETY: plain call on a window of this thread.
        unsafe { SetCapture(pane.hwnd) };
        // Clicking anywhere validates the text being typed instead of dropping it.
        self.commit_text();
        let hit = self.hit(pane, x, y);
        let mut ui = self.ui.borrow_mut();
        ui.tip = None;
        ui.tip_pending = None;
        ui.pointer_pane = Some(i);
        match hit {
            Some(Hover::Button(k)) => ui.press = Some(Press::Button(k)),
            Some(Hover::Bar | Hover::Panel(PanelHit::Background)) => {}
            Some(Hover::Panel(hit @ (PanelHit::Shade | PanelHit::Hue | PanelHit::Size))) => {
                ui.press = Some(Press::Picking(hit));
                drop(ui);
                self.pick(pane, hit, x, y);
                self.refresh();
                return;
            }
            Some(Hover::Panel(hit)) => ui.press = Some(Press::Panel(hit)),
            None => {
                // A press outside the panel closes it.
                ui.style_open = false;
                ui.picker = false;
                ui.press = Some(Press::Image);
                let (ix, iy) = pane.to_image(x, y, ui.scroll);
                drop(ui);
                let shift = key_down(VK_SHIFT);
                self.session.borrow_mut().pointer_down(Point::new(ix, iy), Modifiers { shift });
                self.refresh();
                return;
            }
        }
        drop(ui);
        self.refresh();
    }

    /// Drops the press in progress; a drawing or a zone ends where the pointer last was.
    fn lost_mouse(&self) {
        let press = self.ui.borrow_mut().press.take();
        if press.is_none() {
            return;
        }
        if press == Some(Press::Image)
            && let Ok(mut session) = self.session.try_borrow_mut()
        {
            session.interrupt();
        }
        self.refresh();
    }

    fn pointer_released(&self, i: usize, x: f32, y: f32) {
        let pane = &self.panes[i];
        // The press is taken first: releasing the mouse reports a loss of it (`lost_mouse`).
        let press = self.ui.borrow_mut().press.take();
        // SAFETY: plain call.
        let _ = unsafe { ReleaseCapture() };
        let hit = self.hit(pane, x, y);
        match press {
            Some(Press::Button(k)) if hit == Some(Hover::Button(k)) => {
                self.ui.borrow_mut().hover = hit;
                if self.button(k) {
                    return;
                }
            }
            Some(Press::Panel(p)) if hit == Some(Hover::Panel(p)) => self.panel_click(p),
            Some(Press::Image) => {
                let scroll = self.ui.borrow().scroll;
                let (ix, iy) = pane.to_image(x, y, scroll);
                let shift = key_down(VK_SHIFT);
                let command = self.session.borrow_mut().pointer_up(Point::new(ix, iy), Modifiers { shift });
                if let Some(command) = command
                    && self.run_command(command)
                {
                    return;
                }
            }
            _ => self.ui.borrow_mut().hover = hit,
        }
        self.refresh();
    }

    /// A right-click on a two-tool button takes its other tool.
    fn right_click(&self, i: usize, x: f32, y: f32) {
        if self.ui.borrow().press.is_some() {
            return;
        }
        let Some(Hover::Button(k)) = self.hit(&self.panes[i], x, y) else {
            return;
        };
        let Button::Tool(kit) = layout::button(k).button else {
            return;
        };
        let mut ui = self.ui.borrow_mut();
        let Some(tool) = layout::other_tool(kit, &ui.picks) else {
            return;
        };
        ui.picks.remember(tool);
        drop(ui);
        self.session.borrow_mut().choose_tool(Some(tool));
        self.refresh();
    }

    /// Clicks toolbar button `k`; `true` when the overlay closed.
    fn button(&self, k: usize) -> bool {
        match layout::button(k).button {
            Button::Tool(kit) => {
                let tool = self.ui.borrow().picks.tool(kit);
                self.session.borrow_mut().choose_tool(tool);
            }
            Button::Style => {
                let mut ui = self.ui.borrow_mut();
                ui.style_open = !ui.style_open;
                ui.picker = false;
            }
            Button::Live => {
                if self.can_live() {
                    let live = !self.session.borrow().is_live();
                    self.set_live(live);
                }
            }
            Button::Undo => self.session.borrow_mut().undo(),
            Button::Redo => self.session.borrow_mut().redo(),
            Button::Clear => self.session.borrow_mut().clear(),
            Button::Copy => return self.run_command(Command::Copy),
            Button::CopyText => return self.run_command(Command::CopyText),
            Button::Save => return self.run_command(Command::Save),
            Button::Close => return self.run_command(Command::Close),
        }
        false
    }

    fn panel_click(&self, hit: PanelHit) {
        let view = self.shown();
        let mut session = self.session.borrow_mut();
        let mut ui = self.ui.borrow_mut();
        match hit {
            PanelHit::Palette(i) => {
                if let Some(c) = Color::PALETTE.get(i) {
                    session.set_color(*c);
                }
            }
            PanelHit::Recent(i) => {
                if let Some(c) = view.recent_colors.get(i) {
                    session.set_color(*c);
                }
            }
            PanelHit::OwnColour => {
                ui.picker = !ui.picker;
                let c = view.color;
                ui.hsv = rgb_to_hsv(f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0);
            }
            PanelHit::Eyedropper => {
                session.choose_tool(Some(Tool::Eyedropper));
                ui.style_open = false;
            }
            PanelHit::Fill => session.set_filled(!view.filled),
            PanelHit::Size | PanelHit::Shade | PanelHit::Hue | PanelHit::Background => {}
        }
    }

    /// Picks a colour in the picker from the pointer at `(x, y)`.
    fn pick(&self, pane: &Pane, hit: PanelHit, x: f32, y: f32) {
        let view = self.shown();
        let Some((px, py)) = self.panel_in(pane, &view, &self.ui.borrow()) else {
            return;
        };
        let (lx, ly) = ((x - px) / self.u, (y - py) / self.u);
        if hit == PanelHit::Size {
            self.session.borrow_mut().set_width(layout::size_at(lx, view.max_width));
            return;
        }
        let mut ui = self.ui.borrow_mut();
        let form = PanelForm::of(view.tool, ui.picker);
        match hit {
            PanelHit::Shade => {
                let (s, v) = layout::shade_at(&form, lx, ly);
                ui.hsv.1 = s;
                ui.hsv.2 = v;
            }
            _ => ui.hsv.0 = layout::hue_at(&form, ly),
        }
        let (h, s, v) = ui.hsv;
        drop(ui);
        self.session.borrow_mut().set_color(color_of_hsv(h, s, v));
    }

    /// The wheel turned by `delta` (120 a notch) over pane `i`: the tool in hand draws larger or smaller, and a ring
    /// shows the size for a moment.
    fn wheel(&self, i: usize, delta: f32) {
        let steps = (delta / 120.0).round() as i32;
        let steps = if steps == 0 { delta.signum() as i32 } else { steps };
        let mut session = self.session.borrow_mut();
        if steps == 0 || session.brush().is_none() {
            return;
        }
        session.grow(steps);
        drop(session);
        self.ui.borrow_mut().brush = true;
        // SAFETY: plain timer call on a window of this thread.
        unsafe { SetTimer(Some(self.panes[i].hwnd), BRUSH_TIMER, BRUSH_MS, None) };
        self.refresh();
    }

    fn scroll_by(&self, dy: f32) {
        if !self.scrolling.get() {
            return;
        }
        let image_h = self.session.borrow().base().height as f32;
        let view_h = self.panes[0].size.1 as f32;
        let mut ui = self.ui.borrow_mut();
        ui.scroll = (ui.scroll + dy).clamp(0.0, (image_h - view_h).max(0.0));
        drop(ui);
        self.refresh();
    }

    /// A key went down; `false` when it was not for the editor.
    fn key(&self, vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY, repeated: bool) -> bool {
        let alt = key_down(VK_MENU);
        let held_ctrl = key_down(VK_CONTROL);
        // AltGr reaches programs as Ctrl+Alt: it types characters (€, @…), it is no shortcut.
        let ctrl = held_ctrl && !alt;
        let shift = key_down(VK_SHIFT);
        if self.ui.borrow().typing {
            if alt && !held_ctrl {
                return false; // Alt+F4 and the like
            }
            self.edit_key(vk, ctrl);
            return true;
        }
        if self.scrolling.get() {
            let page = self.panes[0].size.1 as f32 * 0.9;
            let step = match vk {
                VK_NEXT => Some(page),
                VK_PRIOR => Some(-page),
                VK_HOME => Some(-1.0e9),
                VK_END => Some(1.0e9),
                _ => None,
            };
            if let Some(step) = step {
                self.scroll_by(step);
                return true;
            }
        }
        // Space or K: the screen live or frozen, as a video plays or pauses.
        if !ctrl && !alt && (vk == VK_SPACE || vk.0 == u16::from(b'K')) && self.can_live() {
            if !repeated {
                let live = !self.session.borrow().is_live();
                self.set_live(live);
                self.refresh();
            }
            return true;
        }
        let key = match vk {
            VK_ESCAPE => Key::Escape,
            VK_DELETE | VK_BACK => Key::Delete,
            VK_RETURN => Key::Enter,
            VK_LEFT => Key::Left,
            VK_RIGHT => Key::Right,
            VK_UP => Key::Up,
            VK_DOWN => Key::Down,
            v if ctrl && (u16::from(b'A')..=u16::from(b'Z')).contains(&v.0) => {
                Key::Char(char::from(v.0 as u8).to_ascii_lowercase())
            }
            _ => return false,
        };
        let command = self.session.borrow_mut().key(KeyInput { key, ctrl, shift });
        if let Some(command) = command
            && self.run_command(command)
        {
            return true;
        }
        self.refresh();
        true
    }

    /// Editing keys of the text field.
    fn edit_key(&self, vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY, ctrl: bool) {
        let mut ui = self.ui.borrow_mut();
        let caret = ui.caret;
        let prev = ui.text[..caret].chars().next_back().map_or(caret, |c| caret - c.len_utf8());
        let next = ui.text[caret..].chars().next().map_or(caret, |c| caret + c.len_utf8());
        match vk {
            VK_ESCAPE => {
                drop(ui);
                self.session.borrow_mut().cancel_text();
            }
            VK_RETURN => {
                drop(ui);
                self.commit_text();
            }
            _ => {
                match vk {
                    VK_BACK if caret > 0 => {
                        ui.text.replace_range(prev..caret, "");
                        ui.caret = prev;
                    }
                    VK_DELETE if caret < ui.text.len() => {
                        ui.text.replace_range(caret..next, "");
                    }
                    VK_LEFT => ui.caret = prev,
                    VK_RIGHT => ui.caret = next,
                    VK_HOME => ui.caret = 0,
                    VK_END => ui.caret = ui.text.len(),
                    _ => {
                        let _ = ctrl;
                        return;
                    }
                }
                // The caret shows at once where it went (it blinks again from there).
                ui.caret_on = true;
                // Released before the refresh, which reads it.
                drop(ui);
            }
        }
        self.refresh();
    }

    /// A character was typed (UTF-16 unit).
    fn typed(&self, unit: u16) {
        let mut ui = self.ui.borrow_mut();
        if !ui.typing {
            return;
        }
        let c = if (0xd800..0xdc00).contains(&unit) {
            ui.surrogate = Some(unit);
            return;
        } else if (0xdc00..0xe000).contains(&unit) {
            let Some(high) = ui.surrogate.take() else {
                return;
            };
            char::decode_utf16([high, unit]).next().and_then(Result::ok)
        } else {
            char::from_u32(u32::from(unit))
        };
        let Some(c) = c.filter(|c| !c.is_control()) else {
            return;
        };
        let caret = ui.caret;
        ui.text.insert(caret, c);
        ui.caret = caret + c.len_utf8();
        ui.caret_on = true;
        drop(ui);
        self.refresh();
    }

    /// Validates the text being typed, if any.
    fn commit_text(&self) {
        let text = {
            let mut ui = self.ui.borrow_mut();
            if !ui.typing {
                return;
            }
            ui.caret = 0;
            std::mem::take(&mut ui.text)
        };
        let mut session = self.session.borrow_mut();
        if text.is_empty() {
            session.cancel_text();
        } else {
            session.commit_text(&text);
        }
    }

    fn timer(&self, i: usize, id: usize) {
        match id {
            TIP_TIMER => {
                // SAFETY: plain call on a window of this thread.
                let _ = unsafe { KillTimer(Some(self.panes[i].hwnd), TIP_TIMER) };
                let mut ui = self.ui.borrow_mut();
                if ui.press.is_none() && ui.tip_pending.is_some() && ui.hover == ui.tip_pending.map(Hover::Button) {
                    ui.tip = ui.tip_pending.take();
                    drop(ui);
                    self.refresh();
                }
            }
            CARET_TIMER => {
                let mut ui = self.ui.borrow_mut();
                ui.caret_on = !ui.caret_on;
                drop(ui);
                self.refresh();
            }
            LIVE_TIMER => {
                // SAFETY: plain call on a window of this thread.
                let _ = unsafe { KillTimer(Some(self.panes[i].hwnd), LIVE_TIMER) };
                self.live_deferred.set(false);
                self.screens_changed();
            }
            BRUSH_TIMER => {
                // SAFETY: plain call on a window of this thread.
                let _ = unsafe { KillTimer(Some(self.panes[i].hwnd), BRUSH_TIMER) };
                self.ui.borrow_mut().brush = false;
                self.refresh();
            }
            _ => {}
        }
    }

    /// Hides the windows for good: the overlay is closing.
    fn hide(&self) {
        self.closing.set(true);
        for p in &self.panes {
            // SAFETY: plain call on a window of this thread.
            let _ = unsafe { ShowWindow(p.hwnd, SW_HIDE) };
        }
        if self.watched.take().is_some()
            && let Some(screens) = self.screens.borrow().as_ref()
        {
            screens.watch(None, self.wake.clone(), &self.gfx.d3d);
        }
    }

    /// Follows, live, the part of the screens the annotations take their pixels from.
    fn follow(&self, view: &View) {
        let area = if view.live && self.excluded.get() && !self.closing.get() {
            let session = self.session.borrow();
            // Keep the monitor sessions and their last full frames during a selection gesture. Only the crop
            // changes when it ends; closing/reopening WGC on every move adds latency and loses static frames.
            if session.in_gesture() && session.live_area().is_none() {
                return;
            }
            session.live_area()
        } else {
            None
        };
        if self.watched.replace(area) != area {
            self.live_parts.borrow_mut().clear();
            if let Some(screens) = self.screens.borrow().as_ref() {
                screens.watch(area, self.wake.clone(), &self.gfx.d3d);
            }
        }
    }

    /// Takes the pictures of the screens waiting.
    fn screens_changed(&self) {
        if self.live_deferred.get() {
            return;
        }
        if let Some(wait) = self
            .live_shown
            .get()
            .and_then(|shown| LIVE_PERIOD.checked_sub(shown.elapsed()))
            .filter(|wait| !wait.is_zero())
        {
            // `pending` stays set: the pictures coming meanwhile wait for the timer too.
            self.live_deferred.set(true);
            let ms = u32::try_from(wait.as_millis() + 1).unwrap_or(u32::MAX);
            // SAFETY: plain timer call on a window of this thread.
            unsafe { SetTimer(Some(HWND(self.wake.hwnd as *mut _)), LIVE_TIMER, ms, None) };
            return;
        }
        self.wake.pending.store(false, Ordering::Release);
        let parts = match self.screens.borrow().as_ref() {
            Some(screens) if self.watched.get().is_some() => screens.take(),
            _ => return,
        };
        if parts.is_empty() || self.closing.get() {
            return;
        }
        self.live_shown.set(Some(std::time::Instant::now()));
        let mut kept = self.live_parts.borrow_mut();
        let mut drawing = self.drawing.borrow_mut();
        for part in parts {
            if let Some(d) = drawing.as_mut()
                && let Err(e) = d.feed(&self.gfx, &part)
            {
                tracing::warn!("live screens not shown: {e}");
            }
            let same = |k: &ScreenPart| (k.x, k.y, k.width, k.height) == (part.x, part.y, part.width, part.height);
            match kept.iter_mut().find(|k| same(k)) {
                Some(k) => *k = part,
                None => kept.push(part),
            }
        }
        drop((kept, drawing));
        self.refresh();
    }

    /// Shows the screens live, or freezes them as they are now.
    fn set_live(&self, live: bool) {
        let started = std::time::Instant::now();
        if !live && !self.thaw() {
            // A failed recapture must not bring back the stale image from the first Print Screen press.
            return;
        }
        self.session.borrow_mut().set_live(live);
        self.live_shown.set(None);
        tracing::debug!(live, elapsed = ?started.elapsed(), "editor live mode changed");
    }

    /// Takes a fresh picture of the screens, the windows hidden meanwhile when they are not left out of captures.
    fn thaw(&self) -> bool {
        let screens = self.screens.borrow();
        let Some(screens) = screens.as_ref() else {
            return false;
        };
        let hide = !self.excluded.get();
        if hide {
            for p in &self.panes {
                window::set_cloak(p.hwnd, true);
            }
            // SAFETY: plain calls; the next frames are composed without the windows.
            unsafe {
                let _ = DwmFlush();
                let _ = DwmFlush();
            }
        }
        let picture = screens.grab();
        if hide {
            for p in &self.panes {
                window::set_cloak(p.hwnd, false);
            }
        }
        let Some(picture) = picture else {
            return false;
        };
        if self.session.borrow_mut().replace_base(picture) {
            // Older than the picture just taken.
            self.live_parts.borrow_mut().clear();
            let session = self.session.borrow();
            for pane in &self.panes {
                if let Err(e) = upload_frozen(&self.gfx, pane, session.base()) {
                    tracing::warn!("editor picture: {e}");
                    return false;
                }
            }
            true
        } else {
            tracing::warn!("the desktop size changed while refreezing the editor");
            false
        }
    }

    /// Hands `command` to the host; `true` when the overlay closed.
    fn run_command(&self, command: Command) -> bool {
        if self.busy.replace(true) {
            return false;
        }
        let exports = matches!(command, Command::Copy | Command::CopyText | Command::Save | Command::SaveAs);
        let live = self.session.borrow().is_live();
        if exports && live && !self.thaw() {
            self.busy.set(false);
            return false;
        }
        // A system dialog (Save as) must be able to appear above the overlay.
        let dialog = command == Command::SaveAs;
        if dialog {
            for p in &self.panes {
                window::set_topmost(p.hwnd, false);
            }
        }
        // Reading the text takes a moment: the editor goes at once, the result follows it.
        if command == Command::CopyText {
            self.hide();
        }
        let close = (self.handler)(command, &self.session.borrow());
        if dialog {
            for p in &self.panes {
                window::set_topmost(p.hwnd, true);
            }
        }
        self.busy.set(false);
        let aside = self.aside.replace(false);
        if close {
            self.hide();
        } else {
            // The command failed: the editor comes back as it was.
            if aside {
                for p in &self.panes {
                    window::set_cloak(p.hwnd, false);
                }
            }
            self.refresh();
        }
        close
    }

    fn step_aside(&self) {
        if self.busy.get() && self.visible.get() && !self.aside.replace(true) {
            for p in &self.panes {
                window::set_cloak(p.hwnd, true);
            }
        }
    }
}

/// Shows in `pane` its part of the screens' picture.
fn upload_frozen(gfx: &Gfx, pane: &Pane, base: &vixeeny_editor::RgbaImage) -> windows::core::Result<()> {
    let (ax, ay, aw, ah) = pane.area_px();
    let x0 = (ax.max(0.0) as u32).min(base.width);
    let y0 = (ay.max(0.0) as u32).min(base.height);
    let x1 = ((ax + aw).max(0.0) as u32).min(base.width);
    let y1 = ((ay + ah).max(0.0) as u32).min(base.height);
    if x1 > x0 && y1 > y0 {
        pane.frozen.set(gfx, base, (x0, y0, x1 - x0, y1 - y0), x0 as f32, y0 as f32)?;
    }
    Ok(())
}

/// The editor over the frozen screens: one window per monitor, all showing the same session. A window per monitor
/// (rather than one over the whole desktop) keeps every window on a single monitor, so Windows never rescales it when
/// the monitors' DPI differ.
pub struct Overlay {
    shared: Rc<Shared>,
    first_frame: Option<FirstFrame>,
    /// Never shown nor activated (tests): the windows stay cloaked.
    headless: bool,
}

impl Overlay {
    /// A window per screen. `on_command` is called when the user asks to copy, save, run OCR or close; the host does
    /// the work (it has the clipboard and the file system).
    pub fn on_screens(
        mut session: Session,
        ui_scale: f32,
        screens: &[Screen],
        look: Look,
        on_command: impl Fn(Command, &Session) -> bool + 'static,
    ) -> Result<Self, OverlayError> {
        session.set_ui_scale(ui_scale);
        let u = session.ui_scale();
        let gfx = Gfx::shared()?;
        let theme = Theme::new(look);
        let handle_side = paint::handle_side(u);
        let handle = gfx.surface(handle_side, handle_side)?;
        gfx.draw(&handle, handle_side, handle_side, |c| paint::handle(c, &theme, u))?;
        let solids = Solids {
            black: gfx::solid(&gfx, Rgba([0.0, 0.0, 0.0, 1.0]))?,
            accent: gfx::solid(&gfx, theme.accent)?,
            scrollbar: gfx::solid(&gfx, Rgba::hex(0xffffff, 0x88))?,
            handle,
            handle_side,
        };
        let base = session.base();
        let mut panes = Vec::with_capacity(screens.len());
        for s in screens {
            let hwnd = window::create(wndproc, s.position.0, s.position.1, s.size.0, s.size.1)?;
            let pane = Pane::new(&gfx, &solids, hwnd, s.position, s.size, s.area)?;
            // This window's part of the frozen image, uploaded once.
            upload_frozen(&gfx, &pane, base)?;
            panes.push(pane);
        }
        let first_hwnd = panes.first().map_or(0, |p| p.hwnd.0 as usize);
        let shared = Rc::new(Shared {
            gfx,
            theme,
            u,
            solids,
            session: RefCell::new(session),
            ui: RefCell::new(Ui { hsv: (0.0, 1.0, 1.0), tracking: vec![false; screens.len()], ..Ui::default() }),
            panes,
            handler: Box::new(on_command),
            tips: RefCell::default(),
            screens: RefCell::default(),
            excluded: Cell::new(false),
            watched: Cell::new(None),
            live_parts: RefCell::default(),
            label_size: RefCell::default(),
            tip_size: RefCell::default(),
            live_shown: Cell::new(None),
            live_deferred: Cell::new(false),
            drawing: RefCell::default(),
            wake: Wake { hwnd: first_hwnd, pending: Arc::default() },
            scrolling: Cell::new(false),
            busy: Cell::new(false),
            visible: Cell::new(false),
            aside: Cell::new(false),
            closing: Cell::new(false),
            shown: RefCell::new(None),
        });
        CURRENT.with(|c| *c.borrow_mut() = Some(Rc::downgrade(&shared)));
        shared.refresh();
        Ok(Self { shared, first_frame: None, headless: false })
    }

    /// What each toolbar button does, said on hover (in the order of [`layout::BUTTONS`]).
    pub fn set_tips(&self, tips: &[String]) {
        *self.shared.tips.borrow_mut() = tips.to_vec();
    }

    /// Lets the user show the screens live, from `screens`: a fresh picture of them is exported, and the blurred or
    /// pixelated parts follow them.
    pub fn set_screens(&self, screens: impl Screens + 'static) {
        // Set affinity while the editor is still hidden, and keep it through toggles. Repeated affinity changes
        // invalidate compositor capture state and can put our UI into an effect's last in-flight frame.
        let mut excluded = true;
        for p in &self.shared.panes {
            excluded &= window::set_excluded(p.hwnd, true);
        }
        self.shared.excluded.set(excluded);
        *self.shared.screens.borrow_mut() = Some(Box::new(screens));
        self.shared.refresh();
    }

    /// The image is taller than the window (scrolling capture): it scrolls, the toolbar stays.
    pub fn set_scrolling(&self, scrolling: bool) {
        self.shared.scrolling.set(scrolling);
        self.shared.refresh();
    }

    /// Calls `f` with the native window handles once the windows are on screen.
    pub fn on_first_frame(&mut self, f: impl FnOnce(Vec<u64>) + 'static) {
        self.first_frame = Some(Box::new(f));
    }

    /// The windows stay cloaked and inactive (tests drive them with posted messages).
    #[doc(hidden)]
    pub fn headless(mut self) -> Self {
        self.headless = true;
        self
    }

    /// The native handles of the windows, in the order of the screens.
    pub fn handles(&self) -> Vec<u64> {
        self.shared.panes.iter().map(|p| p.hwnd.0 as u64).collect()
    }

    /// Shows every window and runs until the overlay closes. The window under the cursor gets the keyboard.
    pub fn run(mut self, cursor: (i32, i32)) -> Result<(), OverlayError> {
        let shared = self.shared.clone();
        let under_cursor = shared.panes.iter().position(|p| {
            cursor.0 >= p.position.0
                && cursor.1 >= p.position.1
                && cursor.0 < p.position.0 + p.size.0 as i32
                && cursor.1 < p.position.1 + p.size.1 as i32
        });
        shared.ui.borrow_mut().pointer_pane = under_cursor;
        // The daemon already shows a dimmed freeze. Commit the same veil before exposing the editor, so the
        // handoff never brightens the picture or waits for a second fade-in.
        let dim = shared.session.borrow().dim;
        // SAFETY: plain property/commit calls on the panes' composition device.
        unsafe {
            for p in &shared.panes {
                p.veil_effect.SetOpacity2(dim)?;
            }
            shared.gfx.dcomp.Commit()?;
        }
        // SAFETY: plain call; everything built so far reaches the compositor before the windows appear.
        unsafe { shared.gfx.dcomp.WaitForCommitCompletion()? };
        if !self.headless {
            let front = under_cursor.unwrap_or(0);
            for (i, p) in shared.panes.iter().enumerate() {
                if i != front {
                    // SAFETY: plain call on a window of this thread.
                    let _ = unsafe { ShowWindow(p.hwnd, SW_SHOWNOACTIVATE) };
                }
            }
            let hwnd = shared.panes[front].hwnd;
            // SAFETY: plain calls on a window of this thread.
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
                let _ = SetFocus(Some(hwnd));
            }
            for p in &shared.panes {
                window::set_cloak(p.hwnd, false);
            }
            shared.visible.set(true);
            // SAFETY: plain call: waits for the composition that shows the windows.
            let _ = unsafe { DwmFlush() };
        }
        if let Some(f) = self.first_frame.take() {
            f(self.handles());
        }
        shared.refresh();

        let mut msg = MSG::default();
        while !shared.closing.get() {
            // SAFETY: `msg` is valid for the calls; messages of this thread.
            unsafe {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        for p in &shared.panes {
            // SAFETY: the windows were created by this thread.
            let _ = unsafe { DestroyWindow(p.hwnd) };
        }
        CURRENT.with(|c| *c.borrow_mut() = None);
        Ok(())
    }
}

/// Called by a command handler once it holds what it needs from the session (the exported picture, the file chosen):
/// the editor's windows leave the screen at once while the command goes on (encoding, writing, the clipboard), and come
/// back if it fails.
pub fn step_aside() {
    if let Some(shared) = CURRENT.with(|c| c.borrow().as_ref().and_then(Weak::upgrade)) {
        shared.step_aside();
    }
}

/// Closes the editor whose window is `hwnd` as Escape would, from any thread (its shortcut pressed again). Nothing
/// happens while a command runs.
pub fn close(hwnd: u64) {
    // SAFETY: plain call; a window that is gone makes it fail, which changes nothing.
    let _ = unsafe {
        windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(HWND(hwnd as *mut _)),
            WM_CLOSE,
            WPARAM(0),
            LPARAM(0),
        )
    };
}

/// Posts a message to an editor window (tests drive a headless overlay this way).
#[doc(hidden)]
pub fn post(hwnd: u64, msg: u32, wparam: usize, lparam: isize) {
    // SAFETY: plain call; the window belongs to this thread's overlay.
    let _ = unsafe {
        windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(HWND(hwnd as *mut _)),
            msg,
            WPARAM(wparam),
            LPARAM(lparam),
        )
    };
}
