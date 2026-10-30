// SPDX-License-Identifier: GPL-3.0-or-later
//! CPU cost of refreshing an overlay view with unchanged annotations.
use std::{hint::black_box, time::Instant};
use vixeeny_editor::{Color, Modifiers, Point, RgbaImage, Session, Tool};

fn main() {
    let mut session = Session::new(RgbaImage::filled(1920, 1080, Color::rgb(0, 0, 0)), Vec::new());
    session.select_all();
    session.choose_tool(Some(Tool::Pen));
    let modifiers = Modifiers { shift: false };
    for stroke in 0..20 {
        session.pointer_down(Point::new(100.0, 100.0 + stroke as f32), modifiers);
        for point in 0..1000 {
            session.pointer_move(Point::new(100.0 + point as f32, 100.0 + stroke as f32), modifiers);
        }
        session.pointer_up(Point::new(1100.0, 100.0 + stroke as f32), modifiers);
    }
    session.choose_tool(None);
    black_box(session.view());
    let mut samples = [0.0_f64; 7];
    for sample in &mut samples {
        let start = Instant::now();
        for _ in 0..10_000 {
            black_box(session.view());
        }
        *sample = start.elapsed().as_secs_f64() * 1e6 / 10_000.0;
    }
    samples.sort_by(f64::total_cmp);
    println!("unchanged overlay view, 20 strokes of 1000 points: {:.3} us/view", samples[3]);
}
