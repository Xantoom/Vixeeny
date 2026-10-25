// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthetic pages scrolled in many ways must reassemble pixel-exactly.

use super::*;

const W: u32 = 64;

/// Deterministic pseudo-random bytes.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u8
    }
}

/// A page of `rows` rows; `blank` row ranges are plain white.
fn page(rows: usize, blank: &[(usize, usize)]) -> Vec<u8> {
    let mut rng = Rng(7);
    let mut data = Vec::new();
    for y in 0..rows {
        let white = blank.iter().any(|&(a, b)| (a..b).contains(&y));
        for _ in 0..W {
            let v = if white { 255 } else { rng.next() };
            data.extend_from_slice(&[v, v ^ 0x55, v.wrapping_add(31), 255]);
        }
    }
    data
}

struct Screen<'a> {
    page: &'a [u8],
    header: Vec<u8>,
    footer: Vec<u8>,
    height: usize,
}

impl Screen<'_> {
    /// The frame showing the page at `offset`, below the header and above the footer.
    fn frame(&self, offset: usize) -> Frame {
        let rb = W as usize * 4;
        let (h, f) = (self.header.len() / rb, self.footer.len() / rb);
        let mut data = self.header.clone();
        data.extend_from_slice(&self.page[offset * rb..(offset + self.height - h - f) * rb]);
        data.extend_from_slice(&self.footer);
        Frame::new(W, self.height as u32, data).unwrap()
    }
}

fn screen(page: &[u8], height: usize, h: usize, f: usize) -> Screen<'_> {
    let rb = W as usize * 4;
    let mut rng = Rng(99);
    let header = (0..h * rb).map(|_| rng.next()).collect();
    let footer = (0..f * rb).map(|_| rng.next()).collect();
    Screen {
        page,
        header,
        footer,
        height,
    }
}

fn run(screen: &Screen<'_>, offsets: &[usize]) -> Stitched {
    let mut s = Stitcher::new(Config::default());
    for &o in offsets {
        s.push(&screen.frame(o)).unwrap();
    }
    s.finish().unwrap()
}

/// header ++ page[h .. last+H-f) ++ footer for a monotonic scroll from 0.
fn expected(screen: &Screen<'_>, last: usize) -> Vec<u8> {
    let rb = W as usize * 4;
    let (h, f) = (screen.header.len() / rb, screen.footer.len() / rb);
    let mut v = screen.header.clone();
    v.extend_from_slice(&screen.page[..(last + screen.height - h - f) * rb]);
    v.extend_from_slice(&screen.footer);
    v
}

#[test]
fn plain_page_small_and_large_steps() {
    let p = page(600, &[]);
    let s = screen(&p, 100, 0, 0);
    for step in [1, 3, 17, 40, 70] {
        let offsets: Vec<usize> = (0..).map(|i| i * step).take_while(|&o| o <= 500).collect();
        let last = *offsets.last().unwrap();
        let out = run(&s, &offsets);
        assert_eq!(out.frame.data, expected(&s, last), "step {step}");
        assert!(!out.truncated);
    }
}

#[test]
fn sticky_header_and_footer_kept_once() {
    let p = page(600, &[]);
    let s = screen(&p, 120, 12, 8);
    let offsets: Vec<usize> = (0..=480).step_by(25).collect();
    let out = run(&s, &offsets);
    assert_eq!(out.frame.data, expected(&s, 475));
}

#[test]
fn blank_bands_do_not_confuse() {
    let p = page(600, &[(90, 140), (300, 330)]);
    let s = screen(&p, 100, 0, 0);
    let offsets: Vec<usize> = (0..=500).step_by(30).collect();
    let out = run(&s, &offsets);
    assert_eq!(out.frame.data, expected(&s, 480));
}

#[test]
fn scrolling_up_prepends() {
    let p = page(400, &[]);
    let s = screen(&p, 100, 0, 0);
    let offsets = [300, 270, 240, 200, 150, 100];
    let out = run(&s, &offsets);
    let rb = W as usize * 4;
    assert_eq!(out.frame.data, p[100 * rb..400 * rb]);
}

#[test]
fn down_then_up_does_not_duplicate() {
    let p = page(400, &[]);
    let s = screen(&p, 100, 0, 0);
    let out = run(&s, &[0, 30, 60, 90, 60, 30, 0]);
    let rb = W as usize * 4;
    assert_eq!(out.frame.data, p[..190 * rb]);
}

#[test]
fn identical_frames_are_unchanged() {
    let p = page(300, &[]);
    let s = screen(&p, 100, 0, 0);
    let mut st = Stitcher::new(Config::default());
    assert_eq!(st.push(&s.frame(0)).unwrap(), Push::First);
    assert_eq!(st.push(&s.frame(0)).unwrap(), Push::Unchanged);
    assert_eq!(
        st.push(&s.frame(20)).unwrap(),
        Push::Added {
            rows: 20,
            direction: Direction::Down
        }
    );
    assert_eq!(st.push(&s.frame(20)).unwrap(), Push::Unchanged);
    assert_eq!(st.height(), 120);
}

#[test]
fn jump_without_overlap_is_lost_and_recoverable() {
    let p = page(600, &[]);
    let s = screen(&p, 100, 0, 0);
    let mut st = Stitcher::new(Config::default());
    st.push(&s.frame(0)).unwrap();
    st.push(&s.frame(20)).unwrap();
    assert_eq!(st.push(&s.frame(400)).unwrap(), Push::Lost);
    assert_eq!(st.height(), 120);
    st.push(&s.frame(50)).unwrap();
    let out = st.finish().unwrap();
    assert_eq!(out.frame.data, expected(&s, 50));
}

#[test]
fn noisy_frames_still_match() {
    let p = page(400, &[]);
    let s = screen(&p, 100, 0, 0);
    let mut rng = Rng(5);
    let mut st = Stitcher::new(Config::default());
    for o in [0usize, 25, 50, 75, 100] {
        let mut f = s.frame(o);
        for b in &mut f.data {
            *b = b.saturating_add(rng.next() % 3);
        }
        st.push(&f).unwrap();
    }
    assert_eq!(st.height(), 200);
}

#[test]
fn height_limit_truncates() {
    let p = page(600, &[]);
    let s = screen(&p, 100, 0, 0);
    let mut st = Stitcher::new(Config {
        max_height: 150,
        ..Config::default()
    });
    let mut last = Push::First;
    for o in [0, 30, 60, 90, 120] {
        last = st.push(&s.frame(o)).unwrap();
    }
    assert_eq!(last, Push::Full);
    let out = st.finish().unwrap();
    assert!(out.truncated);
    assert_eq!(out.frame.height, 150);
}

#[test]
fn size_change_is_an_error() {
    let p = page(300, &[]);
    let s = screen(&p, 100, 0, 0);
    let mut st = Stitcher::new(Config::default());
    st.push(&s.frame(0)).unwrap();
    let other = screen(&p, 90, 0, 0);
    assert!(matches!(
        st.push(&other.frame(0)),
        Err(StitchError::SizeChanged { .. })
    ));
    // Still usable afterwards.
    st.push(&s.frame(10)).unwrap();
    assert_eq!(st.height(), 110);
}

#[test]
fn preview_fits_the_box() {
    let p = page(300, &[]);
    let s = screen(&p, 100, 0, 0);
    let mut st = Stitcher::new(Config::default());
    st.push(&s.frame(0)).unwrap();
    st.push(&s.frame(100)).unwrap();
    let t = st.preview(32, 50).unwrap();
    assert!(t.width <= 32 && t.height <= 50);
}

#[test]
fn the_preview_shows_the_assembled_image() {
    let p = page(400, &[(120, 140)]);
    let s = screen(&p, 100, 12, 8);
    let mut st = Stitcher::new(Config::default());
    for o in [100, 130, 170, 60, 20, 200] {
        st.push(&s.frame(o)).unwrap();
    }
    let preview = st.preview(16, 60).unwrap();
    let whole = st.finish().unwrap().frame;
    let rows: Vec<&[u8]> = (0..whole.height as usize).map(|y| whole.row(y)).collect();
    assert_eq!(preview, crate::frame::sampled(W, &rows, 16, 60));
    assert!(preview.width <= 16 && preview.height <= 60);
}

/// A layer of another size per pixel: each pixel's bytes twice.
fn doubled(data: &[u8]) -> Vec<u8> {
    data.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [*p, *p].concat())
        .collect()
}

#[test]
fn a_layer_follows_the_rows_kept() {
    let p = page(400, &[(120, 140)]);
    let s = screen(&p, 100, 12, 8);
    let mut st = Stitcher::new(Config::default());
    for o in [0, 30, 70, 50, 110, 160, 200, 170] {
        let frame = s.frame(o);
        st.push_layered(&frame, Some(&doubled(&frame.data)))
            .unwrap();
    }
    let out = st.finish().unwrap();
    assert_eq!(out.frame.data, expected(&s, 200));
    assert_eq!(out.layer.unwrap(), doubled(&out.frame.data));
    // One frame without its layer: no layer at all rather than a wrong one.
    let mut st = Stitcher::new(Config::default());
    for (i, o) in [0, 30, 60].into_iter().enumerate() {
        let frame = s.frame(o);
        let layer = doubled(&frame.data);
        st.push_layered(&frame, (i != 1).then_some(layer.as_slice()))
            .unwrap();
    }
    assert!(st.finish().unwrap().layer.is_none());
}

/// A page of text: lines of words (dark dots on white), blank lines between paragraphs.
fn text_page(width: usize, rows: usize) -> Vec<u8> {
    let mut rng = Rng(11);
    let mut data = Vec::with_capacity(width * rows * 4);
    let mut y = 0;
    while y < rows {
        // A paragraph of 2 to 6 lines of 18 rows (12 of letters), then a blank gap.
        let lines = 2 + rng.next() as usize % 5;
        for _ in 0..lines {
            let mut words = Vec::new();
            let mut x = 8;
            while x < width - 40 {
                let w = 12 + rng.next() as usize % 40;
                words.push((x, (x + w).min(width - 8)));
                x += w + 6;
            }
            for r in 0..18 {
                for x in 0..width {
                    let ink = r < 12
                        && words.iter().any(|&(a, b)| (a..b).contains(&x))
                        && rng.next().is_multiple_of(3);
                    let v = if ink { 30 } else { 255 };
                    data.extend_from_slice(&[v, v, v, 255]);
                }
            }
            y += 18;
        }
        let gap = 10 + rng.next() as usize % 60;
        data.extend(std::iter::repeat_n(255, gap * width * 4));
        y += gap;
    }
    data.truncate(width * rows * 4);
    data
}

#[test]
fn fast_scrolls_over_text_keep_the_page_and_nothing_that_floats() {
    const PW: usize = 320;
    const H: usize = 200;
    let rows = 1200;
    let page = text_page(PW, rows);
    let rb = PW * 4;
    // The page's top stays as a header; a button floats bottom right; a scroll bar on the right.
    let frame = |o: usize| {
        let mut data = page[..20 * rb].to_vec();
        data.extend_from_slice(&page[(o + 20) * rb..(o + H) * rb]);
        for y in 0..H {
            for x in PW - 6..PW {
                let thumb =
                    (o * (H - 40) / (rows - H)..o * (H - 40) / (rows - H) + 40).contains(&y);
                let v = if thumb { 120 } else { 235 };
                data[(y * PW + x) * 4..(y * PW + x) * 4 + 4].copy_from_slice(&[v, v, v, 255]);
            }
            if (H - 40..H - 20).contains(&y) && o > 100 {
                for x in PW - 40..PW - 16 {
                    data[(y * PW + x) * 4..(y * PW + x) * 4 + 4]
                        .copy_from_slice(&[200, 80, 20, 255]);
                }
            }
        }
        Frame::new(PW as u32, H as u32, data).unwrap()
    };
    // Jumps of up to three quarters of the zone are followed.
    let mut st = Stitcher::new(Config::default());
    for o in [
        0, 37, 120, 260, 410, 380, 300, 450, 600, 740, 700, 850, 1000,
    ] {
        assert_ne!(st.push(&frame(o)).unwrap(), Push::Lost, "{o}");
    }
    assert_eq!(st.height() as usize, rows);
    // Up to half the zone a frame (what a capture at 60 frames a second sees of a very fast
    // scroll), every row is also seen in the middle of the screen, away from what floats.
    let mut st = Stitcher::new(Config::default());
    for o in [
        0, 37, 120, 210, 300, 260, 180, 280, 380, 470, 570, 660, 740, 700, 800, 900, 1000,
    ] {
        assert_ne!(st.push(&frame(o)).unwrap(), Push::Lost, "{o}");
    }
    let out = st.finish().unwrap().frame;
    assert_eq!(out.height as usize, rows);
    // The page, but its scroll bar, and the last screen's lower part (the button is there).
    let cols = (PW - 6) * 4;
    for y in 0..rows - 50 {
        assert!(out.row(y)[..cols] == page[y * rb..y * rb + cols], "row {y}");
    }
}
