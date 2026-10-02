// SPDX-License-Identifier: GPL-3.0-or-later
//! Finding how far the content moved between two frames.

use std::collections::HashMap;

use crate::frame::Frame;

/// Which way the content moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The user scrolled down: content moved up, new rows appear at the bottom.
    Down,
    /// The user scrolled up: new rows appear at the top.
    Up,
}

/// How rows are compared.
#[derive(Debug, Clone, Copy)]
pub struct Tolerance {
    /// Largest mean absolute byte difference for two rows to count as equal.
    pub row_mean: u8,
}

pub fn rows_equal(a: &[u8], b: &[u8], tol: Tolerance) -> bool {
    if a == b {
        return true;
    }
    if tol.row_mean == 0 {
        return false;
    }
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    sum <= u64::from(tol.row_mean) * a.len() as u64
}

fn fingerprint(row: &[u8]) -> u64 {
    // FNV-1a over the row with the 2 low bits of each byte dropped (absorbs ±3 noise away
    // from quantisation edges).
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in row {
        h ^= u64::from(b & 0xFC);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Number of identical rows at the same index, from the top (or from the bottom).
pub fn fixed_rows(prev: &Frame, new: &Frame, from_top: bool, tol: Tolerance) -> usize {
    let h = prev.height as usize;
    (0..h)
        .take_while(|&i| {
            let y = if from_top { i } else { h - 1 - i };
            rows_equal(prev.row(y), new.row(y), tol)
        })
        .count()
}

/// A verified shift between the middle parts of two frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shift {
    pub direction: Direction,
    /// Rows the content moved, ≥ 1.
    pub rows: usize,
}

/// Rows of the overlap that agree for a candidate shift, and the overlap size.
fn verify(
    prev: &Frame,
    new: &Frame,
    range: (usize, usize),
    dir: Direction,
    s: usize,
    tol: Tolerance,
) -> (usize, usize) {
    let (a, b) = range;
    let n = b - a;
    if s >= n {
        return (0, 0);
    }
    let overlap = n - s;
    let mut ok = 0;
    for j in 0..overlap {
        let (pi, ni) = match dir {
            Direction::Down => (a + s + j, a + j),
            Direction::Up => (a + j, a + s + j),
        };
        if rows_equal(prev.row(pi), new.row(ni), tol) {
            ok += 1;
        }
    }
    (ok, overlap)
}

/// The shift between `prev` and `new`, comparing only rows `range.0..range.1` (the part that
/// scrolls). `Ok(None)` = identical frames; `Err(())` = no consistent shift (scrolled too far
/// or the content changed).
pub fn find(
    prev: &Frame,
    new: &Frame,
    range: (usize, usize),
    tol: Tolerance,
    min_overlap: usize,
) -> Result<Option<Shift>, ()> {
    let (a, b) = range;
    let n = b.saturating_sub(a);
    if n < 2 {
        return Err(());
    }
    if (a..b).all(|y| rows_equal(prev.row(y), new.row(y), tol)) {
        return Ok(None);
    }

    // Vote with rows whose fingerprint is rare (blank lines match everywhere).
    let prev_fp: Vec<u64> = (a..b).map(|y| fingerprint(prev.row(y))).collect();
    let new_fp: Vec<u64> = (a..b).map(|y| fingerprint(new.row(y))).collect();
    let mut positions: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, fp) in prev_fp.iter().enumerate() {
        positions.entry(*fp).or_default().push(i);
    }
    let mut new_count: HashMap<u64, usize> = HashMap::new();
    for fp in &new_fp {
        *new_count.entry(*fp).or_default() += 1;
    }
    let mut votes: HashMap<(bool, usize), usize> = HashMap::new(); // (is_down, shift)
    for (j, fp) in new_fp.iter().enumerate() {
        let Some(list) = positions.get(fp) else {
            continue;
        };
        if list.len() > 3 || new_count[fp] > 3 {
            continue;
        }
        for &i in list {
            match i.cmp(&j) {
                std::cmp::Ordering::Greater => *votes.entry((true, i - j)).or_default() += 1,
                std::cmp::Ordering::Less => *votes.entry((false, j - i)).or_default() += 1,
                std::cmp::Ordering::Equal => {}
            }
        }
    }
    let mut ranked: Vec<_> = votes.into_iter().collect();
    ranked.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.1.cmp(&y.0.1)));

    let mut best: Option<(usize, Shift)> = None;
    for ((down, s), _) in ranked.into_iter().take(6) {
        let dir = if down { Direction::Down } else { Direction::Up };
        let (ok, overlap) = verify(prev, new, range, dir, s, tol);
        if overlap >= min_overlap && ok * 100 >= overlap * 92 && best.is_none_or(|(o, _)| ok > o) {
            best = Some((
                ok,
                Shift {
                    direction: dir,
                    rows: s,
                },
            ));
        }
    }
    if let Some((_, shift)) = best {
        return Ok(Some(shift));
    }
    coarse(prev, new, range, min_overlap).map(Some).ok_or(())
}

/// Luminance of a row sampled at `cols` evenly spaced pixels.
fn sample(frame: &Frame, y: usize, cols: usize) -> Vec<i32> {
    let w = frame.width as usize;
    let row = frame.row(y);
    (0..cols)
        .map(|c| {
            let x = (c * w / cols).min(w - 1);
            let p = &row[x * 4..x * 4 + 3];
            (i32::from(p[0]) * 30 + i32::from(p[1]) * 59 + i32::from(p[2]) * 11) / 100
        })
        .collect()
}

/// Fallback for noisy frames: smallest mean luminance difference over all shifts.
fn coarse(prev: &Frame, new: &Frame, range: (usize, usize), min_overlap: usize) -> Option<Shift> {
    let (a, b) = range;
    let n = b - a;
    let cols = (prev.width as usize).min(48);
    let p: Vec<Vec<i32>> = (a..b).map(|y| sample(prev, y, cols)).collect();
    let q: Vec<Vec<i32>> = (a..b).map(|y| sample(new, y, cols)).collect();
    let mut best: Option<(f64, Shift)> = None;
    for s in 1..n.saturating_sub(min_overlap) + 1 {
        for dir in [Direction::Down, Direction::Up] {
            let overlap = n - s;
            if overlap < min_overlap {
                continue;
            }
            let mut sum = 0u64;
            for j in 0..overlap {
                let (pi, ni) = match dir {
                    Direction::Down => (s + j, j),
                    Direction::Up => (j, s + j),
                };
                sum += p[pi]
                    .iter()
                    .zip(&q[ni])
                    .map(|(x, y)| u64::from(x.abs_diff(*y)))
                    .sum::<u64>();
            }
            let mean = sum as f64 / (overlap * cols) as f64;
            if mean < 6.0 && best.is_none_or(|(m, _)| mean < m) {
                best = Some((
                    mean,
                    Shift {
                        direction: dir,
                        rows: s,
                    },
                ));
            }
        }
    }
    best.map(|(_, s)| s)
}
