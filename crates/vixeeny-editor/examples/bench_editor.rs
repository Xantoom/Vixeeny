// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo run --profile quick -p vixeeny-editor --example bench_editor`
//! Synthetic CPU timings, with buffers allocated before timing. See docs/PERFORMANCE.md.

use std::hint::black_box;
use std::time::Instant;

use vixeeny_editor::{
    bgra_to_rgba,
    effects::{Region, blur},
};

fn measure(name: &str, iterations: usize, mut run: impl FnMut()) {
    run();
    let mut times = [0.0_f64; 7];
    for time in &mut times {
        let start = Instant::now();
        for _ in 0..iterations {
            run();
        }
        *time = start.elapsed().as_secs_f64() * 1000.0 / iterations as f64;
    }
    times.sort_by(f64::total_cmp);
    println!("{name}: {:.4} ms (median of 7 batches, {iterations} iterations/batch)", times[3]);
}

fn main() {
    for (w, h, iterations) in [(128, 128, 200), (1920, 1080, 30), (3840, 2160, 15), (7680, 2160, 10)] {
        let src: Vec<u8> = (0..w * h * 4).map(|i| (i * 71 % 251) as u8).collect();
        let mut dst = vec![0; src.len()];
        measure(&format!("BGRA {w}x{h}"), iterations, || {
            bgra_to_rgba(black_box(&src), w * 4, black_box(&mut dst), w * 4, (w, h));
            black_box(&dst);
        });
    }
    for (w, h, iterations) in [(128, 128, 30), (1920, 1080, 3)] {
        let mut data: Vec<u8> = (0..w * h * 4).map(|i| (i * 71 % 251) as u8).collect();
        measure(&format!("blur {w}x{h}"), iterations, || {
            blur(black_box(&mut data), w, Region { x: 0, y: 0, w, h }, 8.0);
            black_box(&data);
        });
    }
}
