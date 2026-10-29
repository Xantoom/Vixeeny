// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo run --profile quick -p vixeeny-stitch --example bench_stitch`

#![allow(clippy::unwrap_used, reason = "a developer benchmark with valid synthetic frames")]

use std::hint::black_box;
use std::time::Instant;

use vixeeny_stitch::{Config, Frame, Push, Stitcher};

fn frame(w: usize, h: usize, offset: usize) -> Frame {
    let data = (offset..offset + h)
        .flat_map(|y| {
            (0..w).flat_map(move |x| {
                let n = (y as u32).wrapping_mul(2_654_435_761) ^ ((x / 64) as u32).wrapping_mul(2_246_822_519);
                [n as u8, (n >> 8) as u8, (n >> 16) as u8, 255]
            })
        })
        .collect();
    Frame::new(w as u32, h as u32, data).unwrap()
}

fn main() {
    for (w, h) in [(1920, 1080), (3840, 2160)] {
        let frames: Vec<Frame> = (0..8).map(|i| frame(w, h, i * 120)).collect();
        let mut times = [0.0_f64; 7];
        for time in &mut times {
            let mut stitcher = Stitcher::new(Config::default());
            let start = Instant::now();
            for frame in &frames {
                let pushed = stitcher.push(black_box(frame)).unwrap();
                assert!(matches!(pushed, Push::First | Push::Added { .. }));
            }
            *time = start.elapsed().as_secs_f64() * 1000.0 / frames.len() as f64;
            assert_eq!(stitcher.height(), (h + 7 * 120) as u32);
            black_box(stitcher);
        }
        times.sort_by(f64::total_cmp);
        println!("scroll {w}x{h}: {:.4} ms/frame (median of 7, 8 frames)", times[3]);
    }
}
