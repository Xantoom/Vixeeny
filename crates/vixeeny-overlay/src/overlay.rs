// SPDX-License-Identifier: GPL-3.0-or-later
//! The editor over the frozen screens: one window per monitor, all showing the same session.
//! Input goes to the [`Session`]; after each input the visuals follow its [`View`] and one
//! commit hands the changes to the compositor. Nothing is drawn while nothing changes.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use vixeeny_editor::{Color, Command, Key, KeyInput, Modifiers, Point, Rect, Session, Tool, View};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_NEXT,
    VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, MSG, SW_HIDE, SW_SHOW,
    SW_SHOWNOACTIVATE, SetCursor, SetForegroundWindow, SetTimer, ShowWindow, TranslateMessage,
    WM_CHAR, WM_CLOSE, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_SETCURSOR, WM_SYSKEYDOWN, WM_TIMER,
};

use crate::gfx::{self, Gfx};
use crate::layout::{self, BAR_H, BAR_W, BUTTON, BUTTONS, Button, PanelHit, SHADOW, TOOLS};
use crate::paint::{self, BarLook, FieldLook, PanelLook};
use crate::scene::{self, Pane, Solids, TipLook};
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

/// `WM_MOUSELEAVE` (declared with the common controls in the bindings).
const WM_MOUSELEAVE: u32 = 0x02a3;
const TIP_TIMER: usize = 1;
const CARET_TIMER: usize = 2;
const CARET_BLINK_MS: u32 = 530;

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
    gfx: Gfx,
    theme: Theme,
    u: f32,
    solids: Solids,
    session: RefCell<Session>,
    ui: RefCell<Ui>,
    panes: Vec<Pane>,
    handler: Box<CommandHandler>,
    tips: RefCell<Vec<String>>,
    scrolling: Cell<bool>,
    /// A command is running (a dialog may be up): input waits.
    busy: Cell<bool>,
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
    fn bar_look(&self, view: &View, ui: &Ui) -> BarLook {
        BarLook {
            tool: layout::tool_index(view.tool),
            color: view.color,
            can_undo: view.can_undo,
            can_redo: view.can_redo,
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
            filled: view.filled,
            picker: ui.picker,
            hsv: ui.hsv,
            eyedropper: view.tool == Some(Tool::Eyedropper),
            hover: match (ui.press, ui.hover) {
                (Some(Press::Picking(_)), _) => None,
                (_, Some(Hover::Panel(hit))) => Some(hit),
                _ => None,
            },
        }
    }

    /// Where the bar itself (not its shadow) is in `pane`, window pixels; `None` when the pane
    /// does not show it.
    fn bar_in(&self, pane: &Pane, view: &View, scroll: f32) -> Option<(f32, f32)> {
        let at = view.toolbar?;
        let u = self.u;
        let (x, y) = if self.scrolling.get() {
            // The zone is the whole long image: keep the bar in view, top right.
            ((pane.size.0 as f32 - (BAR_W + 16.0) * u).max(0.0), 16.0 * u)
        } else {
            pane.to_window(at.x, at.y, scroll)
        };
        let (w, h) = (BAR_W * u, BAR_H * u);
        let visible =
            x < pane.size.0 as f32 && x + w > 0.0 && y < pane.size.1 as f32 && y + h > 0.0;
        visible.then_some((x, y))
    }

    fn panel_in(&self, pane: &Pane, view: &View, ui: &Ui) -> Option<(f32, f32)> {
        if !ui.style_open {
            return None;
        }
        let (bx, by) = self.bar_in(pane, view, ui.scroll)?;
        let (dx, dy) = layout::panel_offset(by / self.u, ui.picker);
        Some((bx + dx * self.u, by + dy * self.u))
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

    fn update(&self) -> windows::core::Result<()> {
        let view = self.session.borrow().view();
        self.sync_typing(&view);
        let ui = self.ui.borrow();
        for pane in &self.panes {
            self.update_pane(pane, &view, &ui)?;
        }
        drop(ui);
        self.update_pointer(&view);
        // SAFETY: plain call.
        unsafe { self.gfx.dcomp.Commit() }
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
                let size = paint::label_size(gfx, u, text)?;
                pane.label.show(
                    gfx,
                    text,
                    || Ok(size),
                    |c| paint::size_label(c, t, u, text, size),
                )?;
                pane.label.move_to(at.x.round(), at.y.round())?;
            }
            _ => pane.label.hide()?,
        }

        // The magnifier.
        match &view.magnifier {
            Some(m) if pane.sees((m.position.x, m.position.y, m.size, m.size + 28.0 * u), 0.0) => {
                pane.magnifier.show(
                    gfx,
                    m,
                    || Ok(paint::magnifier_size(u, m)),
                    |c| paint::magnifier(c, t, u, m),
                )?;
                pane.magnifier
                    .move_to(m.position.x.round(), m.position.y.round())?;
            }
            _ => pane.magnifier.hide()?,
        }

        // The zone with its annotations.
        let annotated = view
            .annotated
            .as_ref()
            .filter(|(img, at)| pane.sees((at.x, at.y, img.width as f32, img.height as f32), 0.0));
        let same = match (annotated, pane.annotated_image.borrow().as_ref()) {
            (Some((img, at)), Some((old, old_at))) => {
                Rc::ptr_eq(img, old) && (at.x, at.y) == *old_at
            }
            (None, None) => true,
            _ => false,
        };
        if !same {
            match annotated {
                Some((img, at)) => {
                    pane.annotated
                        .set(gfx, img, (0, 0, img.width, img.height), at.x, at.y)?;
                    *pane.annotated_image.borrow_mut() = Some((img.clone(), (at.x, at.y)));
                }
                None => {
                    pane.annotated.clear()?;
                    *pane.annotated_image.borrow_mut() = None;
                }
            }
        }

        // The text field.
        match (view.text_input, Self::field_look(view, ui)) {
            (Some(at), Some(look)) => {
                pane.field.show(
                    gfx,
                    &look,
                    || paint::field_size(gfx, &look),
                    |c| paint::field(c, &look),
                )?;
                pane.field.move_to(at.x.round(), at.y.round())?;
            }
            _ => pane.field.hide()?,
        }

        // The scroll bar of a long image.
        let image_h = self.session.borrow().base().height as f32;
        let view_h = pane.size.1 as f32;
        if self.scrolling.get() && image_h > view_h {
            let h = (24.0 * u).max(view_h * view_h / image_h);
            let y = scroll / image_h * view_h;
            scene::place(
                &pane.scrollbar,
                (pane.size.0 as f32 - 6.0 * u, y, 4.0 * u, h),
            )?;
        } else {
            scene::place(&pane.scrollbar, (0.0, 0.0, 0.0, 0.0))?;
        }

        // The toolbar, its tip and the style panel.
        let margin = SHADOW * u;
        match self.bar_in(pane, view, scroll) {
            Some((bx, by)) => {
                let look = self.bar_look(view, ui);
                pane.toolbar.show(
                    gfx,
                    &look,
                    || Ok(paint::bar_surface(u)),
                    |c| paint::toolbar(c, t, u, &look),
                )?;
                if pane
                    .toolbar
                    .move_to((bx - margin).round(), (by - margin).round())?
                {
                    pane.toolbar.appear(gfx, 6.0 * u, 0.18, t.animations)?;
                }
                match ui
                    .tip
                    .and_then(|k| Some((k, self.tips.borrow().get(k)?.clone())))
                {
                    Some((k, text)) if !text.is_empty() => {
                        let ((w, h), bubble) = paint::tip_size(gfx, u, &text)?;
                        let look = TipLook { button: k, text };
                        pane.tip.show(
                            gfx,
                            &look,
                            || Ok((w, h)),
                            |c| paint::tip(c, t, u, &look.text, bubble),
                        )?;
                        let below = by < 48.0 * u;
                        let cx = bx + (BUTTONS[k].x + BUTTON / 2.0) * u;
                        let top = by + layout::BUTTON_Y * u;
                        let y = if below {
                            top + BUTTON * u + 14.0 * u
                        } else {
                            top - bubble.1 - 14.0 * u
                        };
                        let m = paint::TIP_MARGIN * u;
                        pane.tip
                            .move_to((cx - bubble.0 / 2.0 - m).round(), (y - m).round())?;
                    }
                    _ => pane.tip.hide()?,
                }
            }
            None => {
                pane.toolbar.hide()?;
                pane.tip.hide()?;
            }
        }
        match self.panel_in(pane, view, ui) {
            Some((px, py)) => {
                let look = Self::panel_look(view, ui);
                pane.panel.show(
                    gfx,
                    &look,
                    || Ok(paint::panel_surface(u)),
                    |c| paint::panel(c, t, u, &look),
                )?;
                let above = py < self.bar_in(pane, view, scroll).map_or(0.0, |b| b.1);
                if pane
                    .panel
                    .move_to((px - margin).round(), (py - margin).round())?
                {
                    pane.panel.appear(
                        gfx,
                        if above { 4.0 * u } else { -4.0 * u },
                        0.1,
                        t.animations,
                    )?;
                }
            }
            None => pane.panel.hide()?,
        }
        Ok(())
    }

    /// The pointer's shape where it is.
    fn update_pointer(&self, view: &View) {
        let mut ui = self.ui.borrow_mut();
        let shape = match (ui.press, ui.hover) {
            (Some(Press::Button(_) | Press::Panel(_)), _) | (None, Some(Hover::Button(_))) => {
                Pointer::Hand
            }
            (None, Some(Hover::Panel(hit))) if hit != PanelHit::Background => Pointer::Hand,
            (Some(Press::Picking(_)), _) | (None, Some(Hover::Bar | Hover::Panel(_))) => {
                Pointer::Arrow
            }
            _ => Pointer::Zone(view.cursor),
        };
        if ui.pointer != Some(shape) && ui.pointer_pane.is_some() {
            ui.pointer = Some(shape);
            // SAFETY: plain call with a system cursor.
            unsafe { SetCursor(Some(window::cursor(shape))) };
        }
    }

    /// The toolbar or panel control at `(x, y)` (window pixels of `pane`).
    fn hit(&self, pane: &Pane, x: f32, y: f32) -> Option<Hover> {
        let view = self.session.borrow().view();
        let ui = self.ui.borrow();
        let u = self.u;
        if let Some((px, py)) = self.panel_in(pane, &view, &ui) {
            let recent = view.recent_colors.len();
            if let Some(hit) = layout::panel_at(
                (x - px) / u,
                (y - py) / u,
                Color::PALETTE.len(),
                recent,
                ui.picker,
            ) {
                return Some(Hover::Panel(hit));
            }
        }
        let (bx, by) = self.bar_in(pane, &view, ui.scroll)?;
        let (lx, ly) = ((x - bx) / u, (y - by) / u);
        if !(0.0..BAR_W).contains(&lx) || !(0.0..BAR_H).contains(&ly) {
            return None;
        }
        Some(layout::button_at(lx, ly).map_or(Hover::Bar, Hover::Button))
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
                let shape = self
                    .ui
                    .borrow()
                    .pointer
                    .unwrap_or(Pointer::Zone(vixeeny_editor::CursorHint::Crosshair));
                // SAFETY: plain call with a system cursor.
                unsafe { SetCursor(Some(window::cursor(shape))) };
                return Some(LRESULT(1));
            }
            WM_CLOSE => {
                self.run_command(Command::Close);
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
                self.scroll_by(-delta);
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(wparam.0 as u16);
                if !self.key(vk) && msg == WM_SYSKEYDOWN {
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
                self.session
                    .borrow_mut()
                    .pointer_move(Point::new(ix, iy), Modifiers { shift });
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
                // A hover that changes nothing on screen costs nothing.
                if !changed && !moved {
                    let view = self.session.borrow().view();
                    self.update_pointer(&view);
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
            Some(Hover::Bar) | Some(Hover::Panel(PanelHit::Background)) => {}
            Some(Hover::Panel(hit @ (PanelHit::Shade | PanelHit::Hue))) => {
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
                self.session
                    .borrow_mut()
                    .pointer_down(Point::new(ix, iy), Modifiers { shift });
                self.refresh();
                return;
            }
        }
        drop(ui);
        self.refresh();
    }

    fn pointer_released(&self, i: usize, x: f32, y: f32) {
        let pane = &self.panes[i];
        // SAFETY: plain call.
        let _ = unsafe { ReleaseCapture() };
        let hit = self.hit(pane, x, y);
        let press = self.ui.borrow_mut().press.take();
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
                let command = self
                    .session
                    .borrow_mut()
                    .pointer_up(Point::new(ix, iy), Modifiers { shift });
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

    /// Clicks toolbar button `k`; `true` when the overlay closed.
    fn button(&self, k: usize) -> bool {
        match BUTTONS[k].button {
            Button::Tool(i) => {
                self.session.borrow_mut().choose_tool(TOOLS[i]);
            }
            Button::Style => {
                let mut ui = self.ui.borrow_mut();
                ui.style_open = !ui.style_open;
                ui.picker = false;
            }
            Button::Undo => self.session.borrow_mut().undo(),
            Button::Redo => self.session.borrow_mut().redo(),
            Button::Copy => return self.run_command(Command::Copy),
            Button::Save => return self.run_command(Command::Save),
            Button::SaveAs => return self.run_command(Command::SaveAs),
            Button::Scroll => return self.run_command(Command::Scroll),
            Button::Close => return self.run_command(Command::Close),
        }
        false
    }

    fn panel_click(&self, hit: PanelHit) {
        let view = self.session.borrow().view();
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
                ui.hsv = rgb_to_hsv(
                    f32::from(c.r) / 255.0,
                    f32::from(c.g) / 255.0,
                    f32::from(c.b) / 255.0,
                );
            }
            PanelHit::Eyedropper => {
                session.choose_tool(Some(Tool::Eyedropper));
                ui.style_open = false;
            }
            PanelHit::Thinner => session.set_width(view.width - 1.0),
            PanelHit::Thicker => session.set_width(view.width + 1.0),
            PanelHit::Fill => session.set_filled(!view.filled),
            PanelHit::Shade | PanelHit::Hue | PanelHit::Background => {}
        }
    }

    /// Picks a colour in the picker from the pointer at `(x, y)`.
    fn pick(&self, pane: &Pane, hit: PanelHit, x: f32, y: f32) {
        let view = self.session.borrow().view();
        let Some((px, py)) = self.panel_in(pane, &view, &self.ui.borrow()) else {
            return;
        };
        let (lx, ly) = ((x - px) / self.u, (y - py) / self.u);
        let mut ui = self.ui.borrow_mut();
        match hit {
            PanelHit::Shade => {
                let (s, v) = layout::shade_at(lx, ly);
                ui.hsv.1 = s;
                ui.hsv.2 = v;
            }
            _ => ui.hsv.0 = layout::hue_at(ly),
        }
        let (h, s, v) = ui.hsv;
        drop(ui);
        self.session.borrow_mut().set_color(color_of_hsv(h, s, v));
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
    fn key(&self, vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
        let ctrl = key_down(VK_CONTROL);
        let shift = key_down(VK_SHIFT);
        if self.ui.borrow().typing {
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
        let prev = ui.text[..caret]
            .chars()
            .next_back()
            .map_or(caret, |c| caret - c.len_utf8());
        let next = ui.text[caret..]
            .chars()
            .next()
            .map_or(caret, |c| caret + c.len_utf8());
        match vk {
            VK_ESCAPE => {
                drop(ui);
                self.session.borrow_mut().cancel_text();
            }
            VK_RETURN => {
                drop(ui);
                self.commit_text();
            }
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
        if let Ok(mut ui) = self.ui.try_borrow_mut() {
            ui.caret_on = true;
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
                if ui.press.is_none()
                    && ui.tip_pending.is_some()
                    && ui.hover == ui.tip_pending.map(Hover::Button)
                {
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
            _ => {}
        }
    }

    /// Hands `command` to the host; `true` when the overlay closed.
    fn run_command(&self, command: Command) -> bool {
        if self.busy.replace(true) {
            return false;
        }
        // A system dialog (Save as) must be able to appear above the overlay.
        let dialog = command == Command::SaveAs;
        if dialog {
            for p in &self.panes {
                window::set_topmost(p.hwnd, false);
            }
        }
        let close = (self.handler)(command, &self.session.borrow());
        if dialog {
            for p in &self.panes {
                window::set_topmost(p.hwnd, true);
            }
        }
        self.busy.set(false);
        if close {
            self.closing.set(true);
            for p in &self.panes {
                // SAFETY: plain call on a window of this thread.
                let _ = unsafe { ShowWindow(p.hwnd, SW_HIDE) };
            }
        } else {
            self.refresh();
        }
        close
    }
}

/// The editor over the frozen screens: one window per monitor, all showing the same session.
/// A window per monitor (rather than one over the whole desktop) keeps every window on a single
/// monitor, so Windows never rescales it when the monitors' DPI differ.
pub struct Overlay {
    shared: Rc<Shared>,
    first_frame: Option<FirstFrame>,
    /// Never shown nor activated (tests): the windows stay cloaked.
    headless: bool,
}

impl Overlay {
    /// A window per screen. `on_command` is called when the user asks to copy, save, run OCR or
    /// close; the host does the work (it has the clipboard and the file system).
    pub fn on_screens(
        mut session: Session,
        ui_scale: f32,
        screens: &[Screen],
        look: Look,
        on_command: impl Fn(Command, &Session) -> bool + 'static,
    ) -> Result<Self, OverlayError> {
        session.set_ui_scale(ui_scale);
        let u = session.ui_scale();
        let gfx = Gfx::new()?;
        let theme = Theme::new(look);
        let handle_side = paint::handle_side(u);
        let handle = gfx.surface(handle_side, handle_side)?;
        gfx.draw(&handle, handle_side, handle_side, |c| {
            paint::handle(c, &theme, u)
        })?;
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
            let (ax, ay, aw, ah) = pane.area_px();
            let x0 = (ax.max(0.0) as u32).min(base.width);
            let y0 = (ay.max(0.0) as u32).min(base.height);
            let x1 = ((ax + aw).max(0.0) as u32).min(base.width);
            let y1 = ((ay + ah).max(0.0) as u32).min(base.height);
            if x1 > x0 && y1 > y0 {
                pane.frozen
                    .set(&gfx, base, (x0, y0, x1 - x0, y1 - y0), x0 as f32, y0 as f32)?;
            }
            panes.push(pane);
        }
        let shared = Rc::new(Shared {
            gfx,
            theme,
            u,
            solids,
            session: RefCell::new(session),
            ui: RefCell::new(Ui {
                hsv: (0.0, 1.0, 1.0),
                tracking: vec![false; screens.len()],
                ..Ui::default()
            }),
            panes,
            handler: Box::new(on_command),
            tips: RefCell::default(),
            scrolling: Cell::new(false),
            busy: Cell::new(false),
            closing: Cell::new(false),
        });
        CURRENT.with(|c| *c.borrow_mut() = Some(Rc::downgrade(&shared)));
        shared.refresh();
        Ok(Self {
            shared,
            first_frame: None,
            headless: false,
        })
    }

    /// What each toolbar button does, said on hover (in the order of [`layout::BUTTONS`]).
    pub fn set_tips(&self, tips: &[String]) {
        *self.shared.tips.borrow_mut() = tips.to_vec();
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

    /// Shows every window and runs until the overlay closes. The window under the cursor gets
    /// the keyboard.
    pub fn run(mut self, cursor: (i32, i32)) -> Result<(), OverlayError> {
        let shared = self.shared.clone();
        let under_cursor = shared.panes.iter().position(|p| {
            cursor.0 >= p.position.0
                && cursor.1 >= p.position.1
                && cursor.0 < p.position.0 + p.size.0 as i32
                && cursor.1 < p.position.1 + p.size.1 as i32
        });
        shared.ui.borrow_mut().pointer_pane = under_cursor;
        // SAFETY: plain call; everything built so far reaches the compositor before the windows
        // appear.
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
            // SAFETY: plain call: waits for the composition that shows the windows.
            let _ = unsafe { DwmFlush() };
        }
        if let Some(f) = self.first_frame.take() {
            f(self.handles());
        }
        // The veil fades in now that the screen shows exactly what it showed.
        let dim = shared.session.borrow().dim;
        for p in &shared.panes {
            // SAFETY: plain property calls with an animation of the same device.
            unsafe {
                if shared.theme.animations {
                    p.veil_effect
                        .SetOpacity(&scene::ease_out(&shared.gfx, 0.0, dim, 0.16)?)?;
                } else {
                    p.veil_effect.SetOpacity2(dim)?;
                }
            }
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
