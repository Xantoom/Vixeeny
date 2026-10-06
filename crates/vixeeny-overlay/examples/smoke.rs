// SPDX-License-Identifier: GPL-3.0-or-later
//! A short visible run of the overlay in a small window: draws a zone, captures the screen,
//! closes itself after a few seconds. `cargo run --example smoke -- <out.png>`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer, WgcBackend};
use vixeeny_editor::{Rect, RgbaImage, Session};
use vixeeny_overlay::{Look, Overlay, Screen, post};

const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_CLOSE: u32 = 0x0010;

fn at(x: i32, y: i32) -> isize {
    ((y as isize) << 16) | (x as isize & 0xffff)
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "smoke.png".into());
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    vixeeny_platform::ensure_dpi_aware();
    let (x0, y0, w, h) = (200, 200, 900u32, 560u32);
    let data = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [(x * 255 / w) as u8, (y * 255 / h) as u8, 150, 255]
        })
        .collect();
    let image = RgbaImage::new(w, h, data).unwrap();
    let style_button = {
        let mut probe = Session::new(image.clone(), Vec::new());
        probe.pointer_down(vixeeny_editor::Point::new(100.0, 80.0), Default::default());
        probe.pointer_up(vixeeny_editor::Point::new(500.0, 300.0), Default::default());
        probe.view().toolbar.map(|b| {
            let spec = &vixeeny_overlay::layout::BUTTONS[12];
            ((b.x + spec.x + 15.0) as i32, (b.y + 23.0) as i32)
        })
    };
    let mut session = Session::new(image, Vec::new());
    session.dim = 0.5;
    let screen = Screen {
        position: (x0, y0),
        size: (w, h),
        area: Rect::new(0.0, 0.0, w as f32, h as f32),
    };
    let t = std::time::Instant::now();
    let mut overlay = Overlay::on_screens(session, 1.0, &[screen], Look::default(), |c, _| {
        eprintln!("command {c:?}");
        true
    })
    .expect("overlay");
    eprintln!("built in {:?}", t.elapsed());
    let hwnd = overlay.handles()[0];
    overlay.on_first_frame(move |_| {
        eprintln!("first frame after {:?}", t.elapsed());
        post(hwnd, WM_MOUSEMOVE, 0, at(100, 80));
        post(hwnd, WM_LBUTTONDOWN, 1, at(100, 80));
        post(hwnd, WM_MOUSEMOVE, 1, at(500, 300));
        post(hwnd, WM_LBUTTONUP, 0, at(500, 300));
        // Open the style panel (button 12), if the bar is where the session puts it.
        if let Some((x, y)) = style_button {
            post(hwnd, WM_MOUSEMOVE, 0, at(x, y));
            post(hwnd, WM_LBUTTONDOWN, 1, at(x, y));
            post(hwnd, WM_LBUTTONUP, 0, at(x, y));
        }
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1200));
            let front = vixeeny_platform::foreground_window()
                .ok()
                .flatten()
                .map(|w| w.id.0);
            eprintln!("keyboard on the overlay: {}", front == Some(hwnd));
            let monitors = vixeeny_platform::monitors().expect("monitors");
            let mut capturer = Capturer::new(WgcBackend::new().expect("wgc"), monitors.clone());
            let bounds =
                vixeeny_platform::virtual_bounds(monitors.iter().map(|m| &m.rect)).unwrap();
            let frame = capturer
                .grab(
                    &CaptureTarget::AllMonitors,
                    CaptureOptions {
                        show_cursor: false,
                        tonemap: None,
                    },
                )
                .expect("grab");
            // The window's part, as BGRA → RGBA.
            let (ox, oy) = ((x0 - bounds.x) as usize, (y0 - bounds.y) as usize);
            let mut rgba = Vec::new();
            for row in 0..h as usize {
                let start = (oy + row) * frame.stride + ox * 4;
                for px in frame.data[start..start + w as usize * 4].chunks(4) {
                    rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            }
            let file = std::fs::File::create(&out).expect("png");
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&rgba).unwrap();
            eprintln!("captured");
            post(hwnd, WM_CLOSE, 0, 0);
        });
    });
    // Whatever happens, close after a few seconds.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(6));
        eprintln!("timeout: closing");
        post(hwnd, WM_CLOSE, 0, 0);
        std::thread::sleep(std::time::Duration::from_secs(2));
        eprintln!("still stuck: exiting");
        std::process::exit(3);
    });
    overlay.run((x0 + 10, y0 + 10)).expect("run");
    eprintln!("closed after {:?}", t.elapsed());
}
