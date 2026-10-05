// SPDX-License-Identifier: GPL-3.0-or-later
//! Binds an [`EditorWindow`] to an editor [`Session`]: forwards input, pushes the view back.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::Key as SlintKey;
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use vixeeny_editor::selection::CursorHint;
use vixeeny_editor::{
    Color, Command, Key, KeyInput, Modifiers, Point, Rect, RgbaImage, Session, Tool,
};

thread_local! {
    /// The annotation drawing each window has as a texture (by window), to upload it once.
    static UPLOADED: RefCell<Vec<(usize, Rc<RgbaImage>)>> = const { RefCell::new(Vec::new()) };
}

/// Identifies a window of the overlay for [`UPLOADED`].
fn window_id(w: &EditorWindow) -> usize {
    std::ptr::from_ref(w.window()) as usize
}

use crate::EditorWindow;

/// Toolbar button index → tool. Index 12 moves and resizes the zone itself (the default).
pub const TOOLS: [Option<Tool>; 13] = [
    Some(Tool::Select),
    Some(Tool::Pen),
    Some(Tool::Line),
    Some(Tool::Arrow),
    Some(Tool::Rect),
    Some(Tool::Ellipse),
    Some(Tool::Text),
    Some(Tool::Highlighter),
    Some(Tool::Blur),
    Some(Tool::Pixelate),
    Some(Tool::Marker),
    Some(Tool::Eyedropper),
    None,
];

pub fn tool_index(tool: Option<Tool>) -> i32 {
    TOOLS
        .iter()
        .position(|t| *t == tool)
        .map_or(-1, |i| i as i32)
}

fn slint_color(c: Color) -> slint::Color {
    slint::Color::from_argb_u8(c.a, c.r, c.g, c.b)
}

fn editor_color(c: slint::Color) -> Color {
    Color {
        r: c.red(),
        g: c.green(),
        b: c.blue(),
        a: c.alpha(),
    }
}

fn slint_image(img: &RgbaImage) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &img.data, img.width, img.height,
    ))
}

/// The part of `img` inside `area` (clipped to the image), copied once.
fn slint_part(img: &RgbaImage, area: &Rect) -> Option<Image> {
    let x0 = area.x.max(0.0).round() as usize;
    let y0 = area.y.max(0.0).round() as usize;
    let x1 = (area.right().round() as usize).min(img.width as usize);
    let y1 = (area.bottom().round() as usize).min(img.height as usize);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
    let row = img.width as usize * 4;
    for (y, out) in buf.make_mut_bytes().chunks_exact_mut(w * 4).enumerate() {
        let at = (y0 + y) * row + x0 * 4;
        out.copy_from_slice(&img.data[at..at + w * 4]);
    }
    Some(Image::from_rgba8(buf))
}

fn cursor_kind(c: CursorHint) -> i32 {
    match c {
        CursorHint::Crosshair => 0,
        CursorHint::Move => 1,
        CursorHint::ResizeNs => 2,
        CursorHint::ResizeEw => 3,
        CursorHint::ResizeNeSw => 4,
        CursorHint::ResizeNwSe => 5,
        CursorHint::Default => 6,
    }
}

/// Maps the text of a Slint key event to an editor key.
pub fn map_key(text: &str, ctrl: bool, shift: bool) -> Option<KeyInput> {
    let c = text.chars().next()?;
    let key = if c == char::from(SlintKey::Escape) {
        Key::Escape
    } else if c == char::from(SlintKey::Delete) || c == char::from(SlintKey::Backspace) {
        Key::Delete
    } else if c == char::from(SlintKey::Return) || c == '\n' {
        Key::Enter
    } else if c == char::from(SlintKey::LeftArrow) {
        Key::Left
    } else if c == char::from(SlintKey::RightArrow) {
        Key::Right
    } else if c == char::from(SlintKey::UpArrow) {
        Key::Up
    } else if c == char::from(SlintKey::DownArrow) {
        Key::Down
    } else {
        // With Ctrl held, some platforms deliver control characters (Ctrl+C = U+0003).
        let c = if ctrl && (c as u32) < 32 && c != '\u{1b}' {
            char::from_u32(c as u32 + 96).unwrap_or(c)
        } else {
            c
        };
        Key::Char(c)
    };
    Some(KeyInput { key, ctrl, shift })
}

/// Returns `true` when the overlay should close afterwards.
type CommandHandler = dyn Fn(Command, &Session, &EditorWindow) -> bool;

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

/// The editor over the frozen screens: one window per monitor, all showing the same session.
/// A window per monitor (rather than one window over the whole desktop) keeps every window on a
/// single monitor, so Windows never rescales it when the monitors' DPI differ.
pub struct Overlay {
    windows: Vec<EditorWindow>,
    session: Rc<RefCell<Session>>,
    /// The window under the pointer (it draws continuously, see `install_notifiers`).
    active: Rc<Cell<usize>>,
    first_frame: RefCell<Option<FirstFrame>>,
}

/// Called with the native window handles once the windows have drawn (see `on_first_frame`).
type FirstFrame = Box<dyn FnOnce(Vec<u64>)>;

/// Turns the v-sync of the window being drawn on or off (OpenGL on Windows; nothing elsewhere).
fn set_swap_interval(api: &slint::GraphicsAPI<'_>, interval: i32) {
    #[cfg(windows)]
    if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
        let f = get_proc_address(c"wglSwapIntervalEXT");
        if !f.is_null() {
            // SAFETY: `wglSwapIntervalEXT` has this signature (WGL_EXT_swap_control) and applies
            // to the current context, which is this window's while it is drawn.
            let f: extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(f) };
            f(interval);
        }
    }
    #[cfg(not(windows))]
    let _ = (api, interval);
}

/// Every window of the overlay, for the callbacks (weak: the windows own the callbacks).
#[derive(Clone)]
struct Windows(Rc<Vec<slint::Weak<EditorWindow>>>);

impl Windows {
    fn each(&self, mut f: impl FnMut(&EditorWindow)) {
        for w in self.0.iter().filter_map(slint::Weak::upgrade) {
            f(&w);
        }
    }

    fn hide_all(&self) {
        self.each(|w| {
            let _ = w.hide();
        });
        let _ = slint::quit_event_loop();
    }

    fn refresh(&self, session: &Rc<RefCell<Session>>) {
        let view = session.borrow().view();
        self.each(|w| refresh_window(w, &view));
    }

    /// The text being typed, wherever its field is (only one window has it non-empty).
    fn take_text(&self) -> String {
        let mut text = String::new();
        self.each(|w| {
            let value = w.get_text_value();
            if !value.is_empty() {
                text = value.to_string();
            }
            w.set_text_value("".into());
        });
        text
    }
}

impl Overlay {
    /// One window showing the whole image (tests, and the scrolling editor through
    /// [`Overlay::on_screens`]). `on_command` is called when the user asks to copy, save, run OCR
    /// or close; the host does the work (it has the clipboard and the file system).
    pub fn new(
        session: Session,
        ui_scale: f32,
        on_command: impl Fn(Command, &Session, &EditorWindow) -> bool + 'static,
    ) -> Result<Self, slint::PlatformError> {
        let (w, h) = (session.base().width, session.base().height);
        let screen = Screen {
            position: (0, 0),
            size: (w, h),
            area: Rect::new(0.0, 0.0, w as f32, h as f32),
        };
        Self::on_screens(session, ui_scale, &[screen], on_command)
    }

    /// A window per screen.
    pub fn on_screens(
        mut session: Session,
        ui_scale: f32,
        screens: &[Screen],
        on_command: impl Fn(Command, &Session, &EditorWindow) -> bool + 'static,
    ) -> Result<Self, slint::PlatformError> {
        session.set_ui_scale(ui_scale);
        let base = session.base();
        let palette: Vec<slint::Color> = Color::PALETTE.iter().map(|c| slint_color(*c)).collect();
        let palette = ModelRc::from(Rc::new(VecModel::from(palette)));
        let mut windows = Vec::with_capacity(screens.len());
        for screen in screens {
            let window = EditorWindow::new()?;
            crate::theme::apply(&window, crate::theme::default_look());
            let whole = screen.area.x <= 0.0
                && screen.area.y <= 0.0
                && screen.area.w >= base.width as f32
                && screen.area.h >= base.height as f32;
            // Each window uploads only its own part of the image.
            let part = if whole {
                Some(slint_image(base))
            } else {
                slint_part(base, &screen.area)
            };
            if let Some(part) = part {
                window.set_frozen(part);
            }
            window.set_area_x(screen.area.x);
            window.set_area_y(screen.area.y);
            window.set_area_w(screen.area.w);
            window.set_area_h(screen.area.h);
            window.set_view_width(i32::try_from(screen.size.0).unwrap_or(i32::MAX));
            window.set_view_height(i32::try_from(screen.size.1).unwrap_or(i32::MAX));
            window.set_image_width(i32::try_from(base.width).unwrap_or(i32::MAX));
            window.set_image_height(i32::try_from(base.height).unwrap_or(i32::MAX));
            window.set_ui_scale(ui_scale);
            window.set_palette(palette.clone());
            let w = window.window();
            w.set_position(slint::PhysicalPosition::new(
                screen.position.0,
                screen.position.1,
            ));
            w.set_size(slint::PhysicalSize::new(screen.size.0, screen.size.1));
            windows.push(window);
        }
        let overlay = Self {
            windows,
            session: Rc::new(RefCell::new(session)),
            active: Rc::default(),
            first_frame: RefCell::new(None),
        };
        overlay.wire(Rc::new(on_command));
        overlay.refresh();
        Ok(overlay)
    }

    /// The first window (the only one of a single-screen overlay).
    pub fn window(&self) -> &EditorWindow {
        &self.windows[0]
    }

    /// Shows every window and runs until the overlay closes. The window under the cursor is
    /// shown last so that it has the keyboard.
    pub fn run(self, cursor: (i32, i32)) -> Result<(), slint::PlatformError> {
        let under_cursor = |w: &EditorWindow| {
            let (p, s) = (w.window().position(), w.window().size());
            cursor.0 >= p.x
                && cursor.1 >= p.y
                && cursor.0 < p.x + s.width as i32
                && cursor.1 < p.y + s.height as i32
        };
        if let Some(i) = self.windows.iter().position(under_cursor) {
            self.active.set(i);
        }
        self.install_notifiers();
        let (front, others): (Vec<_>, Vec<_>) = self.windows.iter().partition(|w| under_cursor(w));
        for w in others.iter().chain(&front) {
            w.show()?;
        }
        slint::run_event_loop()?;
        for w in &self.windows {
            let _ = w.hide();
        }
        UPLOADED.with(|u| u.borrow_mut().clear());
        Ok(())
    }

    /// Calls `f` with the native window handles once every window has drawn its first frame
    /// (or after a second, for a renderer that does not tell). The screen stays undimmed until
    /// then: the windows may be hidden (cloaked) until `f` shows them.
    pub fn on_first_frame(&self, f: impl FnOnce(Vec<u64>) + 'static) {
        for w in &self.windows {
            w.set_hold(true);
        }
        *self.first_frame.borrow_mut() = Some(Box::new(f));
    }

    /// Paces the windows: the one under the pointer redraws at the screen's refresh rate
    /// (v-sync), which keeps a variable-refresh (VRR) screen at its top rate and steady, while
    /// the others draw only when something changes, without waiting for the v-sync (two windows
    /// waiting in turn would halve the rate). Also reports the first frame.
    fn install_notifiers(&self) {
        let pending = self.first_frame.borrow_mut().take();
        let weak: Rc<Vec<_>> = Rc::new(self.windows.iter().map(|w| w.as_weak()).collect());
        let pending = Rc::new(RefCell::new(pending));
        let fire = {
            let weak = weak.clone();
            Rc::new(move || {
                let Some(f) = pending.borrow_mut().take() else {
                    return;
                };
                let windows: Vec<EditorWindow> = weak.iter().filter_map(|w| w.upgrade()).collect();
                #[cfg(feature = "desktop")]
                let handles = windows
                    .iter()
                    .filter_map(|w| crate::theme::native_handle(w.window()))
                    .collect();
                #[cfg(not(feature = "desktop"))]
                let handles = Vec::new();
                f(handles);
                for w in &windows {
                    w.set_hold(false);
                }
            })
        };
        let left = Rc::new(Cell::new(self.windows.len()));
        for (i, w) in self.windows.iter().enumerate() {
            let (left, fire, drawn) = (left.clone(), fire.clone(), Cell::new(false));
            let (active, me) = (self.active.clone(), w.as_weak());
            let interval = Cell::new(-1);
            let _ = w
                .window()
                .set_rendering_notifier(move |state, api| match state {
                    slint::RenderingState::BeforeRendering => {
                        let want = i32::from(active.get() == i);
                        if interval.replace(want) != want {
                            set_swap_interval(api, want);
                        }
                    }
                    slint::RenderingState::AfterRendering => {
                        if !drawn.replace(true) {
                            left.set(left.get() - 1);
                            if left.get() == 0 {
                                // The frame is presented right after this notification.
                                let fire = fire.clone();
                                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                                    fire()
                                });
                            }
                        }
                        if active.get() == i
                            && let Some(w) = me.upgrade()
                        {
                            w.window().request_redraw();
                        }
                    }
                    _ => {}
                });
        }
        slint::Timer::single_shot(std::time::Duration::from_secs(1), move || fire());
    }

    /// Image taller than the window: it scrolls (wheel, Page Up/Down) under a fixed toolbar.
    pub fn set_scrolling(&self, scrolling: bool) {
        for w in &self.windows {
            w.set_scrolling(scrolling);
        }
        self.refresh();
    }

    pub fn session(&self) -> Rc<RefCell<Session>> {
        self.session.clone()
    }

    fn wire(&self, on_command: Rc<CommandHandler>) {
        let all = Windows(Rc::new(self.windows.iter().map(|w| w.as_weak()).collect()));
        for (i, w) in self.windows.iter().enumerate() {
            wire_window(w, &all, &self.session, &on_command);
            // The pointer entering a window makes it the one that draws continuously.
            let (active, me) = (self.active.clone(), w.as_weak());
            w.on_pointer_entered(move || {
                if active.replace(i) != i
                    && let Some(w) = me.upgrade()
                {
                    w.window().request_redraw();
                }
            });
        }
    }

    /// Pushes the session's view into the windows.
    pub fn refresh(&self) {
        let view = self.session.borrow().view();
        for w in &self.windows {
            refresh_window(w, &view);
        }
    }
}

/// Runs `command` through the host; closes every window when it says so.
fn run_command(
    command: Command,
    session: &Rc<RefCell<Session>>,
    window: &EditorWindow,
    all: &Windows,
    handler: &CommandHandler,
) -> bool {
    // A system dialog (Save as) must be able to appear above every window of the overlay.
    let dialog = matches!(command, Command::SaveAs);
    if dialog {
        all.each(|w| w.set_on_top(false));
    }
    let close = handler(command, &session.borrow(), window);
    if dialog {
        all.each(|w| w.set_on_top(true));
    }
    if close {
        all.hide_all();
    }
    close
}

fn wire_window(
    w: &EditorWindow,
    all: &Windows,
    session: &Rc<RefCell<Session>>,
    on_command: &Rc<CommandHandler>,
) {
    let shift = Rc::new(Cell::new(false));

    let (s, a) = (session.clone(), all.clone());
    let sh = shift.clone();
    w.on_pointer_pressed(move |x, y, held| {
        sh.set(held);
        // Clicking elsewhere validates the text being typed instead of dropping it.
        if s.borrow().is_typing() {
            let text = a.take_text();
            s.borrow_mut().commit_text(&text);
        }
        s.borrow_mut()
            .pointer_down(Point::new(x, y), Modifiers { shift: held });
        a.refresh(&s);
    });
    let (s, a) = (session.clone(), all.clone());
    let sh = shift.clone();
    w.on_pointer_moved(move |x, y, _| {
        let mut session = s.borrow_mut();
        let hovering = !session.in_gesture();
        let before = session.hover_state();
        session.pointer_move(Point::new(x, y), Modifiers { shift: sh.get() });
        // A hover that changes nothing on screen costs nothing.
        let changed = !hovering || session.hover_state() != before;
        drop(session);
        if changed {
            a.refresh(&s);
        }
    });
    let (s, a, weak) = (session.clone(), all.clone(), w.as_weak());
    let sh = shift.clone();
    let handler = on_command.clone();
    w.on_pointer_released(move |x, y, held| {
        sh.set(held);
        let command = s
            .borrow_mut()
            .pointer_up(Point::new(x, y), Modifiers { shift: held });
        if let (Some(command), Some(window)) = (command, weak.upgrade())
            && run_command(command, &s, &window, &a, &*handler)
        {
            return;
        }
        a.refresh(&s);
    });

    let sh = shift.clone();
    w.on_key_released(move |text| {
        if text.starts_with(char::from(SlintKey::Shift)) {
            sh.set(false);
        }
    });
    let (s, a, weak, handler) = (
        session.clone(),
        all.clone(),
        w.as_weak(),
        on_command.clone(),
    );
    let sh = shift;
    w.on_key_pressed(move |text, ctrl, shift_held| {
        if text.starts_with(char::from(SlintKey::Shift)) {
            sh.set(true);
        }
        let Some(input) = map_key(&text, ctrl, shift_held) else {
            return;
        };
        // While the text field has the focus the field handles typing itself.
        if s.borrow().is_typing() && !matches!(input.key, Key::Escape) {
            return;
        }
        let command = s.borrow_mut().key(input);
        if let (Some(command), Some(window)) = (command, weak.upgrade())
            && run_command(command, &s, &window, &a, &*handler)
        {
            return;
        }
        a.refresh(&s);
    });

    let (s, a) = (session.clone(), all.clone());
    w.on_tool_chosen(move |i| {
        let tool = usize::try_from(i)
            .ok()
            .and_then(|i| TOOLS.get(i).copied())
            .flatten();
        s.borrow_mut().choose_tool(tool);
        a.refresh(&s);
    });
    let (s, a) = (session.clone(), all.clone());
    w.on_color_chosen(move |c| {
        s.borrow_mut().set_color(editor_color(c));
        a.refresh(&s);
    });
    let (s, a) = (session.clone(), all.clone());
    w.on_width_changed(move |v| {
        s.borrow_mut().set_width(v);
        a.refresh(&s);
    });
    let (s, a) = (session.clone(), all.clone());
    w.on_filled_toggled(move || {
        let filled = s.borrow().view().filled;
        s.borrow_mut().set_filled(!filled);
        a.refresh(&s);
    });
    let (s, a) = (session.clone(), all.clone());
    w.on_text_committed(move |text| {
        s.borrow_mut().commit_text(&text);
        a.take_text();
        a.refresh(&s);
    });
    let (s, a, weak, handler) = (
        session.clone(),
        all.clone(),
        w.as_weak(),
        on_command.clone(),
    );
    w.on_action(move |name| {
        match name.as_str() {
            "undo" => s.borrow_mut().undo(),
            "redo" => s.borrow_mut().redo(),
            other => {
                let command = match other {
                    "copy" => Command::Copy,
                    "save" => Command::Save,
                    "save-as" => Command::SaveAs,
                    "ocr" => Command::Ocr,
                    "scroll" => Command::Scroll,
                    _ => Command::Close,
                };
                if let Some(window) = weak.upgrade()
                    && run_command(command, &s, &window, &a, &*handler)
                {
                    return;
                }
            }
        }
        a.refresh(&s);
    });
}

/// Where something that does not touch a window is put: values that never change, so the
/// window has nothing to redraw (each redraw of each monitor's window waits for its vsync).
const PARKED: Rect = Rect {
    x: -100_000.0,
    y: -100_000.0,
    w: 0.0,
    h: 0.0,
};

/// The part of the image `w` shows.
fn area_of(w: &EditorWindow) -> Rect {
    Rect::new(
        w.get_area_x(),
        w.get_area_y(),
        w.get_area_w(),
        w.get_area_h(),
    )
}

/// `r` (with a margin for what is drawn around it) touches the window's area.
fn shows(area: &Rect, r: &Rect, margin: f32) -> bool {
    area.intersect(&r.inflate(margin)).is_some()
}

fn refresh_window(w: &EditorWindow, v: &vixeeny_editor::View) {
    let area = area_of(w);
    let u = w.get_ui_scale();
    // The zone, its handles and its size label (above or below it).
    let selection = v.selection.map(|r| {
        if shows(&area, &r, 40.0 * u) {
            r
        } else {
            PARKED
        }
    });
    w.set_dim(v.dim);
    w.set_has_selection(selection.is_some());
    w.set_settled(v.settled);
    if let Some(r) = selection {
        w.set_sel_x(r.x);
        w.set_sel_y(r.y);
        w.set_sel_w(r.w);
        w.set_sel_h(r.h);
    }
    let hover = v.hover_window.filter(|r| shows(&area, r, 4.0));
    w.set_has_hover(hover.is_some());
    if let Some(r) = hover {
        w.set_hover_x(r.x);
        w.set_hover_y(r.y);
        w.set_hover_w(r.w);
        w.set_hover_h(r.h);
    }
    if let Some((text, at)) = &v.size_label {
        let at = if selection == Some(PARKED) {
            Point::new(PARKED.x, PARKED.y)
        } else {
            *at
        };
        w.set_size_text(text.as_str().into());
        w.set_size_x(at.x);
        w.set_size_y(at.y);
    }
    let annotated = v.annotated.as_ref().filter(|(img, at)| {
        let r = Rect::new(at.x, at.y, img.width as f32, img.height as f32);
        shows(&area, &r, 0.0)
    });
    w.set_has_annotated(annotated.is_some());
    if let Some((img, at)) = annotated {
        // The same drawing as last time: no new texture.
        let uploaded = UPLOADED.with(|u| {
            u.borrow()
                .iter()
                .any(|(id, image)| *id == window_id(w) && Rc::ptr_eq(image, img))
        });
        if !uploaded {
            w.set_annotated(slint_image(img));
            UPLOADED.with(|u| {
                let mut u = u.borrow_mut();
                u.retain(|(id, _)| *id != window_id(w));
                u.push((window_id(w), img.clone()));
            });
        }
        w.set_annotated_x(at.x);
        w.set_annotated_y(at.y);
    }
    let magnifier = v.magnifier.as_ref().filter(|m| {
        let r = Rect::new(m.position.x, m.position.y, m.size, m.size + 28.0 * u);
        shows(&area, &r, 0.0)
    });
    w.set_has_magnifier(magnifier.is_some());
    if let Some(m) = magnifier {
        w.set_magnifier(slint_image(&m.pixels));
        w.set_mag_x(m.position.x);
        w.set_mag_y(m.position.y);
        w.set_mag_size(m.size);
        w.set_mag_hex(m.hex.as_str().into());
        w.set_mag_color(slint_color(m.color));
    }
    w.set_has_toolbar(v.toolbar.is_some());
    if let Some(at) = v.toolbar {
        w.set_toolbar_x(at.x);
        w.set_toolbar_y(at.y);
        if w.get_scrolling() {
            // The zone is the whole long image: keep the bar in view, top right.
            let u = w.get_ui_scale();
            let width = w.window().size().width as f32;
            w.set_toolbar_x((width - 772.0 * u).max(0.0));
            w.set_toolbar_y(16.0 * u);
        }
    }
    w.set_tool(tool_index(v.tool));
    w.set_current_color(slint_color(v.color));
    let recent: Vec<slint::Color> = v.recent_colors.iter().map(|c| slint_color(*c)).collect();
    // Only touch the model when it changed: replacing it every pointer move would rebuild the row.
    let same = {
        let old = w.get_recent();
        old.row_count() == recent.len()
            && recent
                .iter()
                .enumerate()
                .all(|(i, c)| old.row_data(i) == Some(*c))
    };
    if !same {
        w.set_recent(ModelRc::from(Rc::new(VecModel::from(recent))));
    }
    w.set_stroke_width(v.width);
    w.set_filled(v.filled);
    w.set_can_undo(v.can_undo);
    w.set_can_redo(v.can_redo);
    w.set_has_text_input(v.text_input.is_some());
    w.set_text_size(v.text_size);
    if let Some(at) = v.text_input {
        w.set_text_x(at.x);
        w.set_text_y(at.y);
    }
    w.set_cursor_kind(cursor_kind(v.cursor));
}
