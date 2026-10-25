// SPDX-License-Identifier: GPL-3.0-or-later
//! Replays fast scrolls over long page images and checks the assembled image row by row.
//!
//! `cargo run --release -p vixeeny-stitch --example scroll_sim -- <page.bgra>...`: each file holds its width and height
//! (u32, little endian) then BGRA pixels.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use vixeeny_stitch::{Config, Frame, Push, Stitcher};

struct Page {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Page {
    fn row(&self, y: usize) -> &[u8] {
        &self.px[y * self.w * 4..(y + 1) * self.w * 4]
    }
}

#[derive(Clone, Copy)]
struct Look {
    /// Rows of the page's top kept on screen.
    header: usize,
    /// A dark square at the bottom right once the page has moved.
    button: bool,
    scrollbar: bool,
    /// Every byte off by up to ±3 (another rendering of the same content).
    noise: bool,
}

const SCROLLBAR: usize = 12;

fn frame(page: &Page, o: usize, height: usize, look: Look) -> Frame {
    let w = page.w;
    let mut data = Vec::with_capacity(w * height * 4);
    for y in 0..height {
        let src = if y < look.header { y } else { o + y };
        data.extend_from_slice(page.row(src));
    }
    let mut set = |x: usize, y: usize, c: [u8; 4]| {
        let i = (y * w + x) * 4;
        data[i..i + 4].copy_from_slice(&c);
    };
    if look.button && o > 300 {
        for y in height - 80..height - 30 {
            for x in w - 90..w - 40 {
                set(x, y, [40, 30, 20, 255]);
            }
        }
    }
    if look.scrollbar {
        let thumb = height * height / page.h;
        let top = o * (height - thumb) / (page.h - height).max(1);
        for y in 0..height {
            let c = if (top..top + thumb).contains(&y) { 120 } else { 235 };
            for x in w - SCROLLBAR..w {
                set(x, y, [c, c, c, 255]);
            }
        }
    }
    if look.noise {
        let mut rng = Rng(o as u64 + 1);
        for b in data.iter_mut() {
            *b = b.saturating_add_signed(rng.below(7) as i8 - 3);
        }
    }
    Frame::new(w as u32, height as u32, data).unwrap()
}

/// Deterministic pseudo-random numbers.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Offsets of a scroll: flicks that speed up to `top` rows a frame and slow down, sometimes back up, sometimes
/// stopping; frames dropped at random.
fn offsets(max: usize, top: usize, back: bool, seed: u64) -> Vec<usize> {
    let mut rng = Rng(seed);
    let mut out = vec![0usize];
    let mut o = 0isize;
    let mut v = 0isize;
    let mut target = top as isize;
    while (o as usize) < max {
        if rng.below(12) == 0 {
            target = if back && rng.below(3) == 0 {
                -(rng.below(top as u64 / 2 + 1) as isize)
            } else {
                rng.below(top as u64 + 1) as isize
            };
        }
        v += (target - v).signum() * (1 + rng.below(top as u64 / 4 + 1) as isize);
        v = v.clamp(-(top as isize), top as isize);
        o = (o + v).clamp(0, max as isize);
        // A dropped frame: the next one moved twice as far.
        if rng.below(10) != 0 {
            out.push(o as usize);
        }
    }
    out.push(max);
    out
}

fn run(page: &Page, height: usize, look: Look, top: usize, back: bool, seed: u64) -> String {
    let max = page.h - height;
    let offs = offsets(max, top, back, seed);
    let mut st = Stitcher::new(Config::default());
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    let mut lost = 0;
    let frames: Vec<Frame> = offs.iter().map(|&o| frame(page, o, height, look)).collect();
    let trace = std::env::var_os("TRACE").is_some();
    for (f, o) in frames.iter().zip(&offs) {
        let t = Instant::now();
        let r = st.push(f).unwrap();
        if trace {
            eprintln!("offset {o:>5} -> {r:?} height {}", st.height());
        }
        if r == Push::Lost {
            lost += 1;
        }
        let d = t.elapsed();
        worst = worst.max(d);
        total += d;
    }
    let out = st.finish().unwrap().frame;
    // The page itself, but the scrollbar and the last screen's lower part (its button).
    let cols = page.w - if look.scrollbar { SCROLLBAR } else { 0 };
    let keep_to = max + height - if look.button { 100 } else { 0 };
    let wrong = (0..keep_to.min(out.height as usize))
        .filter(|&y| {
            let diff: u64 = out.row(y)[..cols * 4]
                .iter()
                .zip(&page.row(y)[..cols * 4])
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum();
            diff > if look.noise { 4 * cols as u64 * 4 } else { 0 }
        })
        .inspect(|y| {
            if std::env::var_os("TRACE").is_some() {
                eprintln!("wrong row {y}");
            }
        })
        .count();
    format!(
        "{:>4} frames, speed {:>3}{} | height {:>5}/{:>5} | wrong rows {:>4} | lost {:>2} | {:>5.2} ms/frame, worst {:>5.1} ms",
        frames.len(),
        top,
        if back { " +back" } else { "      " },
        out.height,
        max + height,
        wrong,
        lost,
        total.as_secs_f64() * 1000.0 / frames.len() as f64,
        worst.as_secs_f64() * 1000.0,
    )
}

fn main() {
    if let Ok(case) = std::env::var("CASE") {
        // `page,height,top,back,seed,overlays`
        let v: Vec<&str> = case.split(',').collect();
        let bytes = std::fs::read(v[0]).expect("page file");
        let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let page = Page { w, h, px: bytes[8..].to_vec() };
        let on = v[5] == "1";
        let look = Look { header: if on { 60 } else { 0 }, button: on, scrollbar: on, noise: v.get(6) == Some(&"1") };
        println!(
            "{}",
            run(&page, v[1].parse().unwrap(), look, v[2].parse().unwrap(), v[3] == "1", v[4].parse().unwrap())
        );
        return;
    }
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("page file");
        let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let page = Page { w, h, px: bytes[8..].to_vec() };
        println!("{path} ({w}x{h})");
        let height = 900.min(h / 2);
        for (name, look) in [
            ("plain", Look { header: 0, button: false, scrollbar: false, noise: false }),
            ("overlays", Look { header: 60, button: true, scrollbar: true, noise: false }),
            ("noisy", Look { header: 60, button: true, scrollbar: true, noise: true }),
        ] {
            for (top, back) in [(height / 4, false), (height / 2, true), (height * 3 / 4, true)] {
                for seed in [1, 2] {
                    println!("  {name:<8} {}", run(&page, height, look, top, back, seed));
                }
            }
        }
    }
}
