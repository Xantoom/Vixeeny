// SPDX-License-Identifier: GPL-3.0-or-later
//! The assembling state machine.
//!
//! The long image is a column of rows, each at a position in the page. A new frame is placed where its rows agree with
//! the rows already there, then lends them its rows: rows beyond the image extend it, and a row already there takes the
//! new frame's view of it when that view is nearer the middle of the screen (where no sticky header, footer or floating
//! button sits).

use crate::frame::Frame;

/// Columns of a row summed apart in its signature.
const BANDS: usize = 32;
/// The signature of a row: the mean `B + 2G + R` of each band of columns.
type Sig = [u16; BANDS];

/// Rows compared at most to place a frame.
const SAMPLED_ROWS: usize = 256;
/// Difference of a band (out of 1020) under which two rows agree there.
const BAND_SAME: u32 = 8;
/// Difference of a band over which two rows just differ there (no matter by how much).
const ROW_CAP: u32 = 64;
/// Part of the rows compared that must be the same for a place to be taken.
const SAME_SHARE: f32 = 0.7;
/// The same, when the whole frame is compared.
const WHOLE_SHARE: f32 = 0.95;
/// How much a band of a row must differ from the same band of its neighbours for the row to tell where it is (grain
/// from the rendering stays below).
const DETAIL: u32 = 10;
/// A band that changed by this much or less stayed still (grain from the rendering).
const STILL: u16 = 3;
/// Rows that must have moved for a frame to tell how the content moved.
const MOVED_ROWS: usize = 8;
/// Bands of columns that must have moved for a frame to tell how the content moved.
const MOVING_BANDS: usize = 3;
/// How much worse than the best place the next best must be for the best to be sure.
const MARGIN: f32 = 3.0;

#[derive(Debug, Clone)]
pub struct Config {
    /// The assembled image never gets taller than this (30 000 px).
    pub max_height: u32,
    /// Smallest overlap between a frame and the image, as a fraction of the frame. Frames that overlap less are
    /// reported as [`Push::Lost`].
    pub min_overlap: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self { max_height: 30_000, min_overlap: 0.12 }
    }
}

/// Which way the content moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The user scrolled down: content moved up, new rows appear at the bottom.
    Down,
    /// The user scrolled up: new rows appear at the top.
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Push {
    /// The first frame.
    First,
    Added {
        rows: u32,
        direction: Direction,
    },
    /// Nothing moved since the previous frame, or it only showed rows already there.
    Unchanged,
    /// The content moved too far (or changed) to be placed: scroll back a little.
    Lost,
    /// The height limit is reached; nothing more is added.
    Full,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StitchError {
    #[error("frame size changed from {expected:?} to {got:?}")]
    SizeChanged { expected: (u32, u32), got: (u32, u32) },
}

pub struct Stitched {
    pub frame: Frame,
    /// The layer pushed with every frame, assembled from the same rows (see [`Stitcher::push_layered`]).
    pub layer: Option<Vec<u8>>,
    /// The height limit cut the image.
    pub truncated: bool,
}

/// A row of the long image.
struct Row {
    pixels: Box<[u8]>,
    layer: Option<Box<[u8]>>,
    sig: Sig,
    /// How far from the middle of its frame it was seen (0 = the middle).
    view: u32,
}

/// The rows by position: `below[p]` for `p >= 0`, `above[-1 - p]` for `p < 0`.
#[derive(Default)]
struct Canvas {
    above: Vec<Row>,
    below: Vec<Row>,
}

impl Canvas {
    fn lo(&self) -> isize {
        -(self.above.len() as isize)
    }

    fn hi(&self) -> isize {
        self.below.len() as isize
    }

    fn len(&self) -> usize {
        self.above.len() + self.below.len()
    }

    fn get(&self, p: isize) -> Option<&Row> {
        if p >= 0 { self.below.get(p as usize) } else { self.above.get((-1 - p) as usize) }
    }

    fn get_mut(&mut self, p: isize) -> Option<&mut Row> {
        if p >= 0 { self.below.get_mut(p as usize) } else { self.above.get_mut((-1 - p) as usize) }
    }

    /// Every row, top to bottom.
    fn rows(&self) -> impl Iterator<Item = &Row> {
        self.above.iter().rev().chain(&self.below)
    }
}

/// What is known of a frame to place it.
struct Seen {
    sigs: Vec<Sig>,
    /// Per row, the bands that changed since the previous frame (1) and those that did not (0): what stays put on the
    /// screen while the content moves says nothing of the move.
    moving: Vec<Sig>,
    /// The rows with a band that changed, at most [`SAMPLED_ROWS`], evenly spread.
    sampled: Vec<usize>,
}

pub struct Stitcher {
    config: Config,
    canvas: Canvas,
    size: Option<(u32, u32)>,
    /// Signatures of the previous frame (placed or not).
    prev: Vec<Sig>,
    /// Position of the top row of the last frame placed.
    pos: isize,
    /// Rows the last frame moved by: where the next one is looked for first.
    speed: isize,
    /// The last frame could not be placed: the next ones are looked for everywhere.
    lost: bool,
    /// Every frame came with a layer.
    layered: bool,
    truncated: bool,
}

impl Stitcher {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            canvas: Canvas::default(),
            size: None,
            prev: Vec::new(),
            pos: 0,
            speed: 0,
            lost: false,
            layered: true,
            truncated: false,
        }
    }

    /// Height of the image assembled so far.
    pub fn height(&self) -> u32 {
        self.canvas.len() as u32
    }

    pub fn push(&mut self, frame: &Frame) -> Result<Push, StitchError> {
        self.push_layered(frame, None)
    }

    /// [`push`](Self::push), with `layer`: other bytes for the same rows (the HDR pixels of a frame whose SDR rendering
    /// is matched), kept from the same rows as the frame's.
    pub fn push_layered(&mut self, frame: &Frame, layer: Option<&[u8]>) -> Result<Push, StitchError> {
        let got = (frame.width, frame.height);
        match self.size {
            Some(expected) if expected != got => {
                return Err(StitchError::SizeChanged { expected, got });
            }
            _ => {}
        }
        let layer = layer.filter(|l| l.len() % frame.height as usize == 0);
        self.layered &= layer.is_some();
        let sigs = signatures(frame);
        if self.size.is_none() {
            self.size = Some(got);
            for i in 0..frame.height as usize {
                let row = self.row(frame, layer, &sigs, i);
                self.canvas.below.push(row);
            }
            self.prev = sigs;
            return Ok(Push::First);
        }
        let seen = self.seen(sigs);
        self.prev.clone_from(&seen.sigs);
        if seen.sampled.is_empty() {
            return Ok(Push::Unchanged);
        }
        let Some(pos) = self.place(&seen) else {
            self.lost = true;
            self.speed = 0;
            return Ok(Push::Lost);
        };
        self.lost = false;
        self.speed = pos - self.pos;
        self.pos = pos;
        Ok(self.lend(frame, layer, &seen.sigs, pos))
    }

    fn row(&self, frame: &Frame, layer: Option<&[u8]>, sigs: &[Sig], i: usize) -> Row {
        let n = frame.height as usize;
        Row {
            pixels: frame.row(i).into(),
            layer: layer.filter(|_| self.layered).map(|l| layer_row(l, n, i).into()),
            sig: sigs[i],
            view: view(i, n),
        }
    }

    /// The bands that moved, and the rows to compare.
    fn seen(&self, sigs: Vec<Sig>) -> Seen {
        let moving: Vec<Sig> = sigs
            .iter()
            .zip(&self.prev)
            .map(|(a, b)| std::array::from_fn(|k| u16::from(a[k].abs_diff(b[k]) > STILL)))
            .collect();
        let mut rows: Vec<usize> =
            (0..sigs.len()).filter(|&i| moving[i].iter().any(|m| *m != 0) && detailed(&sigs, i)).collect();
        // Only a narrow column moved (a scroll bar's thumb, a blinking caret): it says nothing of how the content
        // moved.
        let bands = (0..BANDS).filter(|&k| rows.iter().any(|&i| moving[i][k] != 0)).count();
        let used = self.size.map_or(BANDS, |(w, _)| BANDS.min(w as usize));
        // A few rows changed (grain, a small animation): not a scroll.
        if bands < MOVING_BANDS.min(used) || rows.len() < MOVED_ROWS.min(sigs.len() / 4) {
            rows.clear();
        }
        Seen { sigs, moving, sampled: spread(rows) }
    }

    /// The whole frame, every band of every row: when what moved cannot be found, the frame came out of a stretch with
    /// nothing in it (blank space), and is placed by all it shows.
    fn whole(seen: &Seen) -> Seen {
        Seen {
            sigs: seen.sigs.clone(),
            moving: vec![[1; BANDS]; seen.sigs.len()],
            sampled: spread((0..seen.sigs.len()).collect()),
        }
    }

    /// Where the top row of the frame goes, if it can be told.
    fn place(&self, seen: &Seen) -> Option<isize> {
        let n = seen.sigs.len() as isize;
        let overlap = ((n as f32 * self.config.min_overlap) as isize).max(4).min(n - 1);
        let (lo, hi) = (self.canvas.lo(), self.canvas.hi());
        // Every place the frame overlaps the image enough.
        let first = lo - n + overlap;
        let last = hi - overlap;
        let guess = self.pos + self.speed;
        let near = 24 + self.speed.abs() / 2;
        let around = (self.pos - n + overlap, self.pos + n - overlap);
        let ranges: &[(isize, isize, bool)] = if self.lost {
            &[(first, last, false)]
        } else {
            &[(guess - near, guess + near, true), (around.0, around.1, false), (first, last, false)]
        };
        for &(from, to, narrow) in ranges {
            let (from, to) = (from.max(first), to.min(last));
            if from > to {
                continue;
            }
            let scores: Vec<(isize, Score)> = (from..=to).filter_map(|p| self.score(seen, p).map(|s| (p, s))).collect();
            if let Some(p) = best(&scores, guess, self.enough(seen), SAME_SHARE) {
                // At the edge of a narrow search, a better place may lie beyond it.
                if !narrow || (p > from || from == first) && (p < to || to == last) {
                    return Some(p);
                }
            }
        }
        if self.lost {
            return None;
        }
        let whole = Self::whole(seen);
        let (from, to) = (around.0.max(first), around.1.min(last));
        let scores: Vec<(isize, Score)> = (from..=to).filter_map(|p| self.score(&whole, p).map(|s| (p, s))).collect();
        best(&scores, guess, self.enough(&whole), WHOLE_SHARE)
    }

    /// The rows a place must compare to be a proof: as many as the smallest overlap holds.
    fn enough(&self, seen: &Seen) -> u32 {
        let sampled = seen.sampled.len() as u32;
        sampled.min(MOVED_ROWS as u32).max((sampled as f32 * self.config.min_overlap) as u32)
    }

    /// How well the frame agrees with the image with its top row at `pos`.
    fn score(&self, seen: &Seen, pos: isize) -> Option<Score> {
        let mut score = Score::default();
        for &i in &seen.sampled {
            let Some(row) = self.canvas.get(pos + i as isize) else {
                continue;
            };
            let (a, b, m) = (&row.sig, &seen.sigs[i], &seen.moving[i]);
            let (mut sum, mut bands, mut agree) = (0u32, 0u32, 0u32);
            for k in 0..BANDS {
                let d = u32::from(a[k].abs_diff(b[k]));
                let m = u32::from(m[k]);
                sum += d.min(ROW_CAP) * m;
                bands += m;
                agree += u32::from(d <= BAND_SAME) * m;
            }
            score.rows += 1;
            // A band or two may differ: something floating over the content in one of them.
            score.same += u32::from(agree * 4 >= bands * 3);
            score.cost += sum / bands.max(1);
        }
        (score.rows > 0).then_some(score)
    }

    /// Gives the image the rows of `frame` placed at `pos`.
    fn lend(&mut self, frame: &Frame, layer: Option<&[u8]>, sigs: &[Sig], pos: isize) -> Push {
        let n = frame.height as usize;
        let max = self.config.max_height as usize;
        let (mut down, mut up, mut wanted) = (0u32, 0u32, 0u32);
        // Downwards first: the rows below the image come in order.
        for i in 0..n {
            let p = pos + i as isize;
            if p >= self.canvas.hi() {
                wanted += 1;
                if self.canvas.len() < max {
                    let row = self.row(frame, layer, sigs, i);
                    self.canvas.below.push(row);
                    down += 1;
                }
            } else if p >= self.canvas.lo() {
                self.improve(frame, layer, sigs, i, p);
            }
        }
        for i in (0..n).rev() {
            let p = pos + i as isize;
            if p < self.canvas.lo() {
                wanted += 1;
                if self.canvas.len() < max {
                    let row = self.row(frame, layer, sigs, i);
                    self.canvas.above.push(row);
                    up += 1;
                }
            }
        }
        if (down + up) < wanted {
            self.truncated = true;
        }
        match (down, up) {
            (0, 0) if wanted > 0 => Push::Full,
            (0, 0) => Push::Unchanged,
            (rows, 0) => Push::Added { rows, direction: Direction::Down },
            (_, rows) => Push::Added { rows, direction: Direction::Up },
        }
    }

    /// Row `i` of the frame replaces the image's row at `p` when it is seen in the middle half of the screen (the
    /// newest view of it there wins: a picture that has loaded since), or nearer the middle than before.
    fn improve(&mut self, frame: &Frame, layer: Option<&[u8]>, sigs: &[Sig], i: usize, p: isize) {
        let n = frame.height as usize;
        let seen = view(i, n);
        let layered = self.layered;
        let Some(row) = self.canvas.get_mut(p) else {
            return;
        };
        if seen * 2 > n as u32 && seen >= row.view {
            return;
        }
        row.pixels.copy_from_slice(frame.row(i));
        match (row.layer.as_mut(), layer.filter(|_| layered).map(|l| layer_row(l, n, i))) {
            (Some(kept), Some(new)) if kept.len() == new.len() => kept.copy_from_slice(new),
            (_, new) => row.layer = new.map(Into::into),
        }
        row.sig = sigs[i];
        row.view = seen;
    }

    fn assemble(&self) -> Option<(Frame, Option<Vec<u8>>)> {
        let (width, _) = self.size?;
        let mut data = Vec::with_capacity(self.canvas.len() * width as usize * 4);
        for row in self.canvas.rows() {
            data.extend_from_slice(&row.pixels);
        }
        let frame = Frame::new(width, self.canvas.len() as u32, data)?;
        let layer = self
            .layered
            .then(|| {
                let mut bytes = Vec::new();
                for row in self.canvas.rows() {
                    bytes.extend_from_slice(row.layer.as_deref()?);
                }
                Some(bytes)
            })
            .flatten();
        Some((frame, layer))
    }

    /// The image so far, for the live preview (downscaled to fit). Only the rows shown are read.
    pub fn preview(&self, max_width: u32, max_height: u32) -> Option<Frame> {
        let (width, _) = self.size?;
        let rows: Vec<&[u8]> = self.canvas.rows().map(|r| &*r.pixels).collect();
        Some(crate::frame::sampled(width, &rows, max_width, max_height))
    }

    /// The final image. `None` when no frame was pushed.
    pub fn finish(self) -> Option<Stitched> {
        let (frame, layer) = self.assemble()?;
        Some(Stitched { frame, layer, truncated: self.truncated })
    }
}

/// How far row `i` of `n` is from the middle of its frame.
fn view(i: usize, n: usize) -> u32 {
    (2 * i as i64 + 1 - n as i64).unsigned_abs() as u32
}

fn layer_row(layer: &[u8], height: usize, i: usize) -> &[u8] {
    let bytes = layer.len() / height;
    &layer[i * bytes..(i + 1) * bytes]
}

/// How one place agrees, over the rows compared.
#[derive(Debug, Clone, Copy, Default)]
struct Score {
    rows: u32,
    /// Rows that are the same row there.
    same: u32,
    /// Sum of the rows' mean differences, each band's capped at [`ROW_CAP`].
    cost: u32,
}

impl Score {
    fn mean(&self) -> f32 {
        self.cost as f32 / self.rows.max(1) as f32
    }
}

/// The place to take among `scores`: the best agreeing one, when it is clearly better than any other; among places
/// alike (content that repeats), the nearest to `guess`.
fn best(scores: &[(isize, Score)], guess: isize, enough: u32, share: f32) -> Option<isize> {
    let good: Vec<(isize, f32)> = scores
        .iter()
        .filter(|(_, s)| s.rows >= enough && s.same as f32 >= s.rows as f32 * share)
        .map(|(p, s)| (*p, s.mean()))
        .collect();
    let (top, top_mean) =
        good.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.abs_diff(guess).cmp(&b.0.abs_diff(guess))))?;
    // Places as good as the best, apart from its neighbours: the content repeats.
    let alike: Vec<isize> =
        good.iter().filter(|(p, m)| p.abs_diff(top) > 2 && *m < top_mean + MARGIN).map(|(p, _)| *p).collect();
    if alike.is_empty() {
        return Some(top);
    }
    alike.into_iter().chain([top]).min_by_key(|p| p.abs_diff(guess))
}

/// Whether row `i` differs from both rows next to it: a row of blank space (or of a flat colour) is the same
/// everywhere, and cannot tell where it is, even next to an edge.
fn detailed(sigs: &[Sig], i: usize) -> bool {
    let apart = |j: usize| {
        sigs.get(j).map(|o| o.iter().zip(&sigs[i]).map(|(a, b)| u32::from(a.abs_diff(*b))).max().unwrap_or(0))
    };
    [apart(i.wrapping_sub(1)), apart(i + 1)].into_iter().flatten().min().is_some_and(|d| d > DETAIL)
}

/// At most [`SAMPLED_ROWS`] of `rows`, evenly spread.
fn spread(rows: Vec<usize>) -> Vec<usize> {
    if rows.len() <= SAMPLED_ROWS {
        return rows;
    }
    (0..SAMPLED_ROWS).map(|k| rows[k * rows.len() / SAMPLED_ROWS]).collect()
}

/// The signature of every row of `frame`.
fn signatures(frame: &Frame) -> Vec<Sig> {
    let w = frame.width as usize;
    let bands = BANDS.min(w);
    // Sum contiguous bands, so the inner loop has no scatter or dependency on a per-column index. The boundaries
    // are the inverse of floor(x * bands / w), including widths not divisible by the number of bands.
    let edges: [usize; BANDS + 1] = std::array::from_fn(|k| (k.min(bands) * w).div_ceil(bands));
    (0..frame.height as usize)
        .map(|y| {
            let row = frame.row(y);
            std::array::from_fn(|k| {
                let [b, g, r, _] = channel_sums(&row[edges[k] * 4..edges[k + 1] * 4]);
                ((b + 2 * g + r) / (edges[k + 1] - edges[k]).max(1) as u32) as u16
            })
        })
        .collect()
}

/// The sum of each channel of BGRA `pixels`. Sixteen bytes are added at once in 16-bit lanes, which the compiler
/// turns into vector additions: a 4K frame's signatures take a third of the time of a pixel at a time.
fn channel_sums(pixels: &[u8]) -> [u32; 4] {
    let mut sums = [0_u32; 4];
    // 256 additions of a byte stay within 16 bits.
    for part in pixels.chunks(16 * 256) {
        let mut lanes = [0_u16; 16];
        let (blocks, rest) = part.as_chunks::<16>();
        for block in blocks {
            for (lane, v) in lanes.iter_mut().zip(block) {
                *lane += u16::from(*v);
            }
        }
        for (i, lane) in lanes.iter().enumerate() {
            sums[i % 4] += u32::from(*lane);
        }
        for (i, v) in rest.iter().enumerate() {
            sums[i % 4] += u32::from(*v);
        }
    }
    sums
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_band_signatures_match_the_column_mapping_at_every_width() {
        for w in [1, 2, 7, 31, 32, 33, 63, 64, 65, 1001, 1919, 1920] {
            let data = (0..w * 7 * 4).map(|i| (i * 71 % 251) as u8).collect();
            let frame = Frame::new(w, 7, data).unwrap();
            for (y, signature) in signatures(&frame).iter().enumerate() {
                let mut sums = [0_u32; BANDS];
                let mut counts = [0_u32; BANDS];
                for (x, px) in frame.row(y).as_chunks::<4>().0.iter().enumerate() {
                    let band = x * BANDS.min(w as usize) / w as usize;
                    sums[band] += u32::from(px[0]) + 2 * u32::from(px[1]) + u32::from(px[2]);
                    counts[band] += 1;
                }
                let expected: Sig = std::array::from_fn(|k| (sums[k] / counts[k].max(1)) as u16);
                assert_eq!(*signature, expected, "width {w}, row {y}");
            }
        }
    }
}
