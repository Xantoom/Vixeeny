// SPDX-License-Identifier: GPL-3.0-or-later
//! X11 capture against a real X server (CI runs it under Xvfb with `VIXEENY_REQUIRE_X11=1`).
//! Without an X server that allows reading the root window (a Wayland-only session, WSLg) the
//! test says so and passes, unless the variable asks for a failure.
#![cfg(target_os = "linux")]
#![allow(clippy::print_stderr)]

use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer, X11Backend, X11VideoStream};
use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, WindowClass};
use x11rb::wrapper::ConnectionExt as _;

fn required() -> bool {
    std::env::var_os("VIXEENY_REQUIRE_X11").is_some()
}

macro_rules! skip_or_fail {
    ($result:expr, $what:expr) => {
        match $result {
            Ok(v) => v,
            Err(e) if required() => panic!("{}: {e}", $what),
            Err(e) => {
                eprintln!("skipped ({}): {e}", $what);
                return;
            }
        }
    };
}

/// A 200×200 red window at (100, 100) that the window manager (there is none) leaves alone.
fn show_red_window() -> Result<x11rb::rust_connection::RustConnection, Box<dyn std::error::Error>> {
    let (conn, n) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[n];
    let id = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        id,
        screen.root,
        100,
        100,
        200,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .background_pixel(0x00FF_0000)
            .override_redirect(1),
    )?;
    conn.map_window(id)?;
    conn.sync()?;
    // The window lives as long as the connection.
    Ok(conn)
}

#[test]
fn a_monitor_grab_shows_what_is_on_the_screen() {
    let _window = skip_or_fail!(show_red_window(), "no X server");
    let monitors = skip_or_fail!(vixeeny_platform::monitors(), "no monitors");
    let backend = skip_or_fail!(X11Backend::new(), "no X11 backend");
    let mut capturer = Capturer::new(backend, monitors.clone());
    let monitor = &monitors[0];
    let frame = skip_or_fail!(
        capturer.grab(
            &CaptureTarget::Monitor(monitor.id),
            CaptureOptions::default()
        ),
        "cannot read the screen"
    );
    assert_eq!(
        (frame.width, frame.height),
        (monitor.rect.width, monitor.rect.height)
    );
    let (x, y) = (
        (150 - monitor.rect.x).max(0) as u32,
        (150 - monitor.rect.y).max(0) as u32,
    );
    // BGRA: pure red, opaque.
    assert_eq!(frame.pixel(x, y), [0, 0, 255, 255]);
}

#[test]
fn the_stream_delivers_frames_with_increasing_times() {
    let monitors = skip_or_fail!(vixeeny_platform::monitors(), "no monitors");
    let stream = skip_or_fail!(
        X11VideoStream::start_monitor(&monitors[0], 30, false),
        "no stream"
    );
    let mut times = Vec::new();
    let started = std::time::Instant::now();
    while times.len() < 5 && started.elapsed() < std::time::Duration::from_secs(5) {
        match stream.recv(std::time::Duration::from_millis(200)) {
            Ok(Some(f)) => times.push(f.time_ns),
            Ok(None) => {}
            Err(e) if required() => panic!("{e}"),
            Err(e) => {
                eprintln!("skipped (cannot read the screen): {e}");
                return;
            }
        }
    }
    assert_eq!(times.len(), 5, "five frames within five seconds");
    assert!(times.windows(2).all(|w| w[1] > w[0]));
}
