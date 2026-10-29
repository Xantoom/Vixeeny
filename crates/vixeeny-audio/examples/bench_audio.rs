// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo run --profile quick -p vixeeny-audio --example bench_audio` (Windows).

use std::hint::black_box;
use std::time::Instant;

use vixeeny_audio::{AudioChunk, Mixer};

fn main() {
    for seconds in [1, 10] {
        let chunk = AudioChunk::stereo(0, vec![0.1; seconds * 48_000 * 2]);
        let mut times = [0.0_f64; 7];
        for time in &mut times {
            let mut mixer = Mixer::new(&[1.0; 4], 0);
            for source in 0..4 {
                mixer.push(source, &chunk);
            }
            let start = Instant::now();
            black_box(mixer.finish(seconds as i64 * 1_000_000_000));
            *time = start.elapsed().as_secs_f64() * 1000.0;
        }
        times.sort_by(f64::total_cmp);
        println!("audio burst {seconds}s, 4 sources: {:.4} ms (median of 7)", times[3]);
    }
    let mut chunk = AudioChunk::stereo(0, vec![0.1; 480 * 2]);
    let mut times = [0.0_f64; 7];
    for time in &mut times {
        let mut mixer = Mixer::new(&[1.0; 4], 0);
        let start = Instant::now();
        for tick in 0..6000 {
            chunk.time_ns = tick * 10_000_000;
            for source in 0..4 {
                mixer.push(source, black_box(&chunk));
            }
            black_box(mixer.drain((tick + 1) * 10_000_000));
        }
        black_box(mixer.finish(60_000_000_000));
        *time = start.elapsed().as_secs_f64() * 1000.0;
    }
    times.sort_by(f64::total_cmp);
    println!("audio streaming 60s, 4 sources: {:.4} ms (median of 7)", times[3]);
}
