// SPDX-License-Identifier: GPL-3.0-or-later
//! The overlay driven by posted messages, its windows cloaked (nothing shows on screen).
#![allow(clippy::expect_used, reason = "test helpers fail the test")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use vixeeny_editor::{Command, Rect, RgbaImage, Session};
use vixeeny_overlay::{Look, Overlay, Screen, layout, post};

const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CLOSE: u32 = 0x0010;
const VK_ESCAPE: usize = 0x1b;

fn at(x: i32, y: i32) -> isize {
    ((y as isize) << 16) | (x as isize & 0xffff)
}

fn session() -> Session {
    let (w, h) = (1200, 900);
    let data = (0..w * h).flat_map(|i| [(i % 251) as u8, 90, 160, 255]).collect();
    Session::new(RgbaImage::new(w, h, data).expect("image"), Vec::new())
}

type Log = Rc<RefCell<Vec<(Command, Option<Rect>)>>>;

fn overlay(log: &Log) -> Overlay {
    let screen = Screen { position: (0, 0), size: (1200, 900), area: Rect::new(0.0, 0.0, 1200.0, 900.0) };
    let log = log.clone();
    Overlay::on_screens(session(), 1.0, &[screen], Look::default(), move |command, s| {
        log.borrow_mut().push((command, s.zone()));
        true
    })
    .expect("overlay")
    .headless()
}

fn draw_zone(hwnd: u64) {
    post(hwnd, WM_MOUSEMOVE, 0, at(100, 100));
    post(hwnd, WM_LBUTTONDOWN, 1, at(100, 100));
    post(hwnd, WM_MOUSEMOVE, 1, at(300, 250));
    post(hwnd, WM_MOUSEMOVE, 1, at(500, 400));
    post(hwnd, WM_LBUTTONUP, 0, at(500, 400));
}

#[test]
fn a_zone_is_drawn_and_escape_closes() {
    let log: Log = Rc::default();
    let overlay = overlay(&log);
    let hwnd = overlay.handles()[0];
    draw_zone(hwnd);
    post(hwnd, WM_KEYDOWN, VK_ESCAPE, 0);
    post(hwnd, WM_CLOSE, 0, 0);
    overlay.run((10, 10)).expect("ran");
    let log = log.borrow();
    assert_eq!(log[0].0, Command::Close);
    assert_eq!(log[0].1, Some(Rect::new(100.0, 100.0, 400.0, 300.0)));
}

type WatchedArea = Option<(u32, u32, u32, u32)>;

struct LivePictures {
    pictures: RefCell<std::collections::VecDeque<Option<RgbaImage>>>,
    grabs: Rc<Cell<usize>>,
    watches: Rc<RefCell<Vec<WatchedArea>>>,
}

impl vixeeny_overlay::Screens for LivePictures {
    fn grab(&self) -> Option<RgbaImage> {
        self.grabs.set(self.grabs.get() + 1);
        self.pictures.borrow_mut().pop_front().flatten()
    }

    fn watch(&self, area: Option<(u32, u32, u32, u32)>, _: vixeeny_overlay::Wake, _: &vixeeny_overlay::ID3D11Device) {
        self.watches.borrow_mut().push(area);
    }

    fn take(&self) -> Vec<vixeeny_overlay::ScreenPart> {
        Vec::new()
    }
}

/// The real overlay loop with deterministic capture results; no desktop capture or visible windows.
fn refreeze(picture: Option<RgbaImage>, fail_copy: bool) -> (usize, bool, Vec<Command>, Option<Rect>, Vec<u8>) {
    let grabs = Rc::new(Cell::new(0));
    let observed = Rc::new(RefCell::new(None));
    let out = observed.clone();
    let screen = Screen { position: (0, 0), size: (1200, 900), area: Rect::new(0.0, 0.0, 1200.0, 900.0) };
    let commands = Rc::new(RefCell::new(Vec::new()));
    let command_log = commands.clone();
    let overlay = Overlay::on_screens(session(), 1.0, &[screen], Look::default(), move |c, s| {
        command_log.borrow_mut().push(c);
        *out.borrow_mut() = Some((s.is_live(), s.zone(), s.base().data[..4].to_vec()));
        true
    })
    .expect("overlay")
    .headless();
    overlay.set_screens(LivePictures {
        pictures: RefCell::new([picture].into()),
        grabs: grabs.clone(),
        watches: Rc::default(),
    });
    let hwnd = overlay.handles()[0];
    draw_zone(hwnd);
    post(hwnd, WM_KEYDOWN, 0x20, 0); // live
    for key in [0x20, usize::from(b'K')] {
        post(hwnd, WM_KEYDOWN, key, 1 << 30); // Windows' key-repeat flag: must not toggle or recapture
    }
    post(hwnd, WM_KEYDOWN, usize::from(b'K'), 0); // freeze, or stay live if capture fails
    if fail_copy {
        click_button(hwnd, COPY);
    }
    post(hwnd, WM_CLOSE, 0, 0);
    overlay.run((10, 10)).expect("ran");
    let (live, zone, pixels) = observed.take().expect("closed");
    let commands = commands.borrow().clone();
    (grabs.get(), live, commands, zone, pixels)
}

#[test]
fn refreezing_uses_the_new_picture_and_ignores_key_repeat() {
    let picture = RgbaImage::new(1200, 900, [21, 42, 63, 255].repeat(1200 * 900)).expect("image");
    let (grabs, live, commands, zone, pixels) = refreeze(Some(picture), false);
    assert_eq!(grabs, 1);
    assert!(!live);
    assert_eq!(commands, [Command::Close]);
    assert_eq!(zone, Some(Rect::new(100.0, 100.0, 400.0, 300.0)));
    assert_eq!(pixels, [21, 42, 63, 255]);
}

#[test]
fn a_failed_refreeze_stays_live_and_never_exports_the_stale_picture() {
    let (grabs, live, commands, _, pixels) = refreeze(None, true);
    assert_eq!(grabs, 2, "refreeze and copy each attempt a fresh capture");
    assert!(live);
    assert_eq!(commands, [Command::Close], "copy must not reach the export handler");
    assert_eq!(pixels, [0, 90, 160, 255]);
}

#[test]
fn a_changed_desktop_size_does_not_restore_the_old_freeze() {
    let picture = RgbaImage::new(1, 1, vec![21, 42, 63, 255]).expect("image");
    let (_, live, _, _, pixels) = refreeze(Some(picture), false);
    assert!(live);
    assert_eq!(pixels, [0, 90, 160, 255]);
}

#[test]
fn moving_a_live_zone_keeps_its_capture_sessions_until_the_new_crop_is_ready() {
    use vixeeny_editor::{Point, Tool};
    let mut s = session();
    s.pointer_down(Point::new(100.0, 100.0), Default::default());
    s.pointer_up(Point::new(500.0, 400.0), Default::default());
    s.choose_tool(Some(Tool::Blur));
    s.pointer_down(Point::new(200.0, 200.0), Default::default());
    s.pointer_up(Point::new(250.0, 250.0), Default::default());
    s.choose_tool(None);
    let screen = Screen { position: (0, 0), size: (1200, 900), area: Rect::new(0.0, 0.0, 1200.0, 900.0) };
    let overlay = Overlay::on_screens(s, 1.0, &[screen], Look::default(), |_, _| true).expect("overlay").headless();
    let watches = Rc::new(RefCell::new(Vec::new()));
    overlay.set_screens(LivePictures { pictures: RefCell::default(), grabs: Rc::default(), watches: watches.clone() });
    let hwnd = overlay.handles()[0];
    post(hwnd, WM_KEYDOWN, 0x20, 0);
    post(hwnd, WM_LBUTTONDOWN, 1, at(400, 350));
    post(hwnd, WM_MOUSEMOVE, 1, at(420, 370));
    post(hwnd, WM_LBUTTONUP, 0, at(420, 370));
    post(hwnd, WM_CLOSE, 0, 0);
    overlay.run((10, 10)).expect("ran");
    assert_eq!(*watches.borrow(), [Some((100, 100, 400, 300)), Some((120, 120, 400, 300)), None]);
}

#[test]
fn the_toolbar_buttons_answer_clicks() {
    // Where the bar goes for that zone (the same session, run without windows).
    let mut probe = session();
    probe.pointer_down(vixeeny_editor::Point::new(100.0, 100.0), Default::default());
    probe.pointer_move(vixeeny_editor::Point::new(500.0, 400.0), Default::default());
    probe.pointer_up(vixeeny_editor::Point::new(500.0, 400.0), Default::default());
    let bar = probe.view().toolbar.expect("a toolbar once the zone is drawn");

    let log: Log = Rc::default();
    let overlay = overlay(&log);
    let hwnd = overlay.handles()[0];
    draw_zone(hwnd);
    // The copy button.
    let (bx, by) = layout::button_xy(COPY, bar.side.upright());
    let x = (bar.at.x + bx + 15.0) as i32;
    let y = (bar.at.y + by + 15.0) as i32;
    post(hwnd, WM_MOUSEMOVE, 0, at(x, y));
    post(hwnd, WM_LBUTTONDOWN, 1, at(x, y));
    post(hwnd, WM_LBUTTONUP, 0, at(x, y));
    post(hwnd, WM_CLOSE, 0, 0);
    overlay.run((10, 10)).expect("ran");
    let log = log.borrow();
    assert_eq!(log[0].0, Command::Copy);
    assert_eq!(log[0].1, Some(Rect::new(100.0, 100.0, 400.0, 300.0)));
}

const WM_CHAR: u32 = 0x0102;
const VK_RETURN: usize = 0x0d;
const VK_BACK: usize = 0x08;
const VK_LEFT: usize = 0x25;
const VK_HOME: usize = 0x24;
const VK_END: usize = 0x23;
const VK_DELETE: usize = 0x2e;

/// Buttons of the bar (see `layout::BUTTONS`).
const PEN: usize = 2;
const SHAPE: usize = 5;
const TEXT: usize = 6;
const STYLE: usize = 9;
const CLEAR: usize = 12;
const COPY: usize = 13;

/// Where the bar goes for the zone of [`draw_zone`] (the same session, run without windows).
fn bar_place() -> vixeeny_editor::BarPlace {
    let mut probe = session();
    probe.pointer_down(vixeeny_editor::Point::new(100.0, 100.0), Default::default());
    probe.pointer_up(vixeeny_editor::Point::new(500.0, 400.0), Default::default());
    probe.view().toolbar.expect("a toolbar")
}

fn click(hwnd: u64, x: i32, y: i32) {
    post(hwnd, WM_MOUSEMOVE, 0, at(x, y));
    post(hwnd, WM_LBUTTONDOWN, 1, at(x, y));
    post(hwnd, WM_LBUTTONUP, 0, at(x, y));
}

/// Clicks toolbar button `k` for the zone of [`draw_zone`].
fn click_button(hwnd: u64, k: usize) {
    let bar = bar_place();
    let (bx, by) = layout::button_xy(k, bar.side.upright());
    click(hwnd, (bar.at.x + bx + 15.0) as i32, (bar.at.y + by + 15.0) as i32);
}

/// Where the style panel opens (its top-left corner), for a panel `height` units tall.
fn panel_at(height: f32) -> (f32, f32) {
    let bar = bar_place();
    let upright = bar.side.upright();
    let room = (bar.at.y, 900.0 - bar.at.y - layout::BAR_H);
    let before = matches!(bar.side, vixeeny_editor::BarSide::Above);
    let (dx, dy) = layout::panel_offset(upright, before, room, height);
    (bar.at.x + dx, bar.at.y + dy)
}

#[test]
fn text_is_typed_into_the_zone() {
    let log: Log = Rc::default();
    let shared = Rc::new(RefCell::new(None));
    let screen = Screen { position: (0, 0), size: (1200, 900), area: Rect::new(0.0, 0.0, 1200.0, 900.0) };
    let (l, out) = (log.clone(), shared.clone());
    let overlay = Overlay::on_screens(session(), 1.0, &[screen], Look::default(), move |c, s| {
        l.borrow_mut().push((c, s.zone()));
        let first = s.editor().doc.items.first().and_then(|i| match &i.annotation {
            vixeeny_editor::Annotation::Text { text, .. } => Some(text.clone()),
            _ => None,
        });
        *out.borrow_mut() = Some((s.editor().doc.items.len(), first));
        true
    })
    .expect("overlay")
    .headless();
    let hwnd = overlay.handles()[0];
    draw_zone(hwnd);
    click_button(hwnd, TEXT);
    post(hwnd, WM_MOUSEMOVE, 0, at(200, 200));
    post(hwnd, WM_LBUTTONDOWN, 1, at(200, 200));
    post(hwnd, WM_LBUTTONUP, 0, at(200, 200));
    for c in "Hé ✓".encode_utf16() {
        post(hwnd, WM_CHAR, usize::from(c), 0);
    }
    // Editing keys: the last character goes, then the first.
    for vk in [VK_BACK, VK_LEFT, VK_HOME, VK_DELETE, VK_END] {
        post(hwnd, WM_KEYDOWN, vk, 0);
    }
    post(hwnd, WM_KEYDOWN, VK_RETURN, 0);
    // A second text, validated by clicking elsewhere.
    post(hwnd, WM_LBUTTONDOWN, 1, at(300, 300));
    post(hwnd, WM_LBUTTONUP, 0, at(300, 300));
    post(hwnd, WM_CHAR, usize::from(b'x'), 0);
    post(hwnd, WM_LBUTTONDOWN, 1, at(350, 350));
    post(hwnd, WM_LBUTTONUP, 0, at(350, 350));
    post(hwnd, WM_KEYDOWN, VK_ESCAPE, 0);
    post(hwnd, WM_KEYDOWN, VK_ESCAPE, 0);
    post(hwnd, WM_CLOSE, 0, 0);
    overlay.run((10, 10)).expect("ran");
    assert_eq!(log.borrow()[0].0, Command::Close);
    assert_eq!(*shared.borrow(), Some((2, Some("é ".to_owned()))));
}

#[test]
fn the_style_panel_opens_and_picks_a_colour() {
    let log: Log = Rc::default();
    let colour = Rc::new(RefCell::new(None));
    let screen = Screen { position: (0, 0), size: (1200, 900), area: Rect::new(0.0, 0.0, 1200.0, 900.0) };
    let (l, out) = (log, colour.clone());
    let overlay = Overlay::on_screens(session(), 1.0, &[screen], Look::default(), move |c, s| {
        l.borrow_mut().push((c, s.zone()));
        *out.borrow_mut() = Some(s.view().color);
        true
    })
    .expect("overlay")
    .headless();
    let hwnd = overlay.handles()[0];
    draw_zone(hwnd);
    click_button(hwnd, STYLE);
    // The third palette swatch of the panel.
    let (px, py) = panel_at(layout::PanelForm::of(None, false).height());
    click(hwnd, (px + layout::swatch_x(2) + 12.0) as i32, (py + 22.0) as i32);
    post(hwnd, WM_CLOSE, 0, 0);
    overlay.run((10, 10)).expect("ran");
    assert_eq!(*colour.borrow(), Some(vixeeny_editor::Color::PALETTE[2]));
}

const WM_MOUSEWHEEL: u32 = 0x020a;

/// Runs the overlay over [`draw_zone`]'s zone with `input`, then reads the session as it closes.
fn after<T: 'static>(input: impl FnOnce(u64), read: impl Fn(&Session) -> T + 'static) -> Option<T> {
    let out = Rc::new(RefCell::new(None));
    let screen = Screen { position: (0, 0), size: (1200, 900), area: Rect::new(0.0, 0.0, 1200.0, 900.0) };
    let o = out.clone();
    let overlay = Overlay::on_screens(session(), 1.0, &[screen], Look::default(), move |_, s| {
        *o.borrow_mut() = Some(read(s));
        true
    })
    .expect("overlay")
    .headless();
    let hwnd = overlay.handles()[0];
    draw_zone(hwnd);
    input(hwnd);
    post(hwnd, WM_CLOSE, 0, 0);
    overlay.run((10, 10)).expect("ran");
    out.take()
}

#[test]
fn the_wheel_sizes_the_tool_in_hand_only() {
    let wheel = |hwnd: u64, notches: i16| {
        let delta = (notches * 120) as u16 as usize;
        post(hwnd, WM_MOUSEMOVE, 0, at(300, 300));
        post(hwnd, WM_MOUSEWHEEL, delta << 16, at(300, 300));
    };
    // No tool: nothing changes.
    let width = after(|hwnd| wheel(hwnd, 3), |s| s.view().width);
    assert_eq!(width, Some(4.0));
    let width = after(
        |hwnd| {
            click_button(hwnd, PEN);
            wheel(hwnd, 3);
            wheel(hwnd, -1);
        },
        |s| s.view().width,
    );
    assert_eq!(width, Some(6.0));
}

#[test]
fn a_right_click_switches_a_button_to_its_other_tool() {
    let tool = after(
        |hwnd| {
            let bar = bar_place();
            let (bx, by) = layout::button_xy(SHAPE, bar.side.upright());
            let (x, y) = ((bar.at.x + bx + 15.0) as i32, (bar.at.y + by + 15.0) as i32);
            post(hwnd, WM_MOUSEMOVE, 0, at(x, y));
            post(hwnd, WM_RBUTTONDOWN, 2, at(x, y));
            post(hwnd, WM_RBUTTONUP, 0, at(x, y));
            // The button now takes the ellipse.
            click_button(hwnd, PEN);
            click_button(hwnd, SHAPE);
        },
        |s| s.tool(),
    );
    assert_eq!(tool, Some(Some(vixeeny_editor::Tool::Ellipse)));
}

#[test]
fn the_clear_button_takes_every_annotation_away() {
    let counts = after(
        |hwnd| {
            click_button(hwnd, PEN);
            post(hwnd, WM_MOUSEMOVE, 0, at(200, 200));
            post(hwnd, WM_LBUTTONDOWN, 1, at(200, 200));
            post(hwnd, WM_MOUSEMOVE, 1, at(260, 240));
            post(hwnd, WM_LBUTTONUP, 0, at(260, 240));
            click_button(hwnd, CLEAR);
        },
        |s| (s.editor().doc.items.len(), s.view().can_undo),
    );
    assert_eq!(counts, Some((0, true)));
}

/// Whether DWM keeps `hwnd` off the screen.
fn cloaked(hwnd: u64) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    let mut value = 0_u32;
    // SAFETY: `value` is a valid out-pointer of the size given.
    let read = unsafe {
        DwmGetWindowAttribute(
            HWND(hwnd as usize as *mut _),
            DWMWA_CLOAKED,
            (&raw mut value).cast(),
            size_of::<u32>() as u32,
        )
    };
    read.is_ok() && value != 0
}

#[test]
fn a_command_that_fails_after_stepping_aside_leaves_the_editor_open() {
    let screen = Screen { position: (0, 0), size: (1200, 900), area: Rect::new(0.0, 0.0, 1200.0, 900.0) };
    let log: Log = Rc::default();
    let seen = log.clone();
    let window = Rc::new(std::cell::Cell::new(0_u64));
    let handle = window.clone();
    let hidden = Rc::new(std::cell::Cell::new(false));
    let hidden_seen = hidden.clone();
    let overlay = Overlay::on_screens(session(), 1.0, &[screen], Look::default(), move |command, s| {
        seen.borrow_mut().push((command, s.zone()));
        if command == Command::Copy {
            // The picture taken, the editor steps aside; then the command fails.
            vixeeny_overlay::step_aside();
            return false;
        }
        // A headless editor never comes on screen, even after a failed command.
        hidden_seen.set(cloaked(handle.get()));
        true
    })
    .expect("overlay")
    .headless();
    let hwnd = overlay.handles()[0];
    window.set(hwnd);
    draw_zone(hwnd);
    click_button(hwnd, COPY);
    post(hwnd, WM_CLOSE, 0, 0);
    overlay.run((10, 10)).expect("ran");
    let commands: Vec<Command> = log.borrow().iter().map(|(c, _)| *c).collect();
    assert_eq!(commands, [Command::Copy, Command::Close]);
    assert!(hidden.get());
}
