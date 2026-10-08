// SPDX-License-Identifier: GPL-3.0-or-later
//! The overlay driven by posted messages, its windows cloaked (nothing shows on screen).
#![allow(clippy::expect_used)]

use std::cell::RefCell;
use std::rc::Rc;

use vixeeny_editor::{Command, Rect, RgbaImage, Session};
use vixeeny_overlay::{Look, Overlay, Screen, layout, post};

const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CLOSE: u32 = 0x0010;
const VK_ESCAPE: usize = 0x1b;

fn at(x: i32, y: i32) -> isize {
    ((y as isize) << 16) | (x as isize & 0xffff)
}

fn session() -> Session {
    let (w, h) = (1200, 900);
    let data = (0..w * h)
        .flat_map(|i| [(i % 251) as u8, 90, 160, 255])
        .collect();
    Session::new(RgbaImage::new(w, h, data).expect("image"), Vec::new())
}

type Log = Rc<RefCell<Vec<(Command, Option<Rect>)>>>;

fn overlay(log: &Log) -> Overlay {
    let screen = Screen {
        position: (0, 0),
        size: (1200, 900),
        area: Rect::new(0.0, 0.0, 1200.0, 900.0),
    };
    let log = log.clone();
    Overlay::on_screens(
        session(),
        1.0,
        &[screen],
        Look::default(),
        move |command, s| {
            log.borrow_mut().push((command, s.zone()));
            true
        },
    )
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

#[test]
fn the_toolbar_buttons_answer_clicks() {
    // Where the bar goes for that zone (the same session, run without windows).
    let mut probe = session();
    probe.pointer_down(vixeeny_editor::Point::new(100.0, 100.0), Default::default());
    probe.pointer_move(vixeeny_editor::Point::new(500.0, 400.0), Default::default());
    probe.pointer_up(vixeeny_editor::Point::new(500.0, 400.0), Default::default());
    let bar = probe
        .view()
        .toolbar
        .expect("a toolbar once the zone is drawn");

    let log: Log = Rc::default();
    let overlay = overlay(&log);
    let hwnd = overlay.handles()[0];
    draw_zone(hwnd);
    // The copy button (index 15).
    let spec = &layout::BUTTONS[15];
    let x = (bar.x + spec.x + 15.0) as i32;
    let y = (bar.y + layout::BUTTON_Y + 15.0) as i32;
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

/// Clicks toolbar button `k` for the zone of [`draw_zone`].
fn click_button(hwnd: u64, k: usize) {
    let mut probe = session();
    probe.pointer_down(vixeeny_editor::Point::new(100.0, 100.0), Default::default());
    probe.pointer_up(vixeeny_editor::Point::new(500.0, 400.0), Default::default());
    let bar = probe.view().toolbar.expect("a toolbar");
    let x = (bar.x + layout::BUTTONS[k].x + 15.0) as i32;
    let y = (bar.y + layout::BUTTON_Y + 15.0) as i32;
    post(hwnd, WM_MOUSEMOVE, 0, at(x, y));
    post(hwnd, WM_LBUTTONDOWN, 1, at(x, y));
    post(hwnd, WM_LBUTTONUP, 0, at(x, y));
}

#[test]
fn text_is_typed_into_the_zone() {
    let log: Log = Rc::default();
    let shared = Rc::new(RefCell::new(None));
    let screen = Screen {
        position: (0, 0),
        size: (1200, 900),
        area: Rect::new(0.0, 0.0, 1200.0, 900.0),
    };
    let (l, out) = (log.clone(), shared.clone());
    let overlay = Overlay::on_screens(session(), 1.0, &[screen], Look::default(), move |c, s| {
        l.borrow_mut().push((c, s.zone()));
        *out.borrow_mut() = Some(s.editor().doc.items.len());
        true
    })
    .expect("overlay")
    .headless();
    let hwnd = overlay.handles()[0];
    draw_zone(hwnd);
    click_button(hwnd, 8); // Text
    post(hwnd, WM_MOUSEMOVE, 0, at(200, 200));
    post(hwnd, WM_LBUTTONDOWN, 1, at(200, 200));
    post(hwnd, WM_LBUTTONUP, 0, at(200, 200));
    for c in "Hé ✓".encode_utf16() {
        post(hwnd, WM_CHAR, usize::from(c), 0);
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
    assert_eq!(*shared.borrow(), Some(2));
}

#[test]
fn the_style_panel_opens_and_picks_a_colour() {
    let log: Log = Rc::default();
    let colour = Rc::new(RefCell::new(None));
    let screen = Screen {
        position: (0, 0),
        size: (1200, 900),
        area: Rect::new(0.0, 0.0, 1200.0, 900.0),
    };
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
    click_button(hwnd, 12); // Style
    // The panel opens below the bar or above it: find the third palette swatch either way.
    let mut probe = session();
    probe.pointer_down(vixeeny_editor::Point::new(100.0, 100.0), Default::default());
    probe.pointer_up(vixeeny_editor::Point::new(500.0, 400.0), Default::default());
    let bar = probe.view().toolbar.expect("a toolbar");
    let (dx, dy) = layout::panel_offset(bar.y, false);
    let x = (bar.x + dx + layout::swatch_x(2) + 12.0) as i32;
    let y = (bar.y + dy + 22.0) as i32;
    post(hwnd, WM_MOUSEMOVE, 0, at(x, y));
    post(hwnd, WM_LBUTTONDOWN, 1, at(x, y));
    post(hwnd, WM_LBUTTONUP, 0, at(x, y));
    post(hwnd, WM_CLOSE, 0, 0);
    overlay.run((10, 10)).expect("ran");
    assert_eq!(*colour.borrow(), Some(vixeeny_editor::Color::PALETTE[2]));
}
