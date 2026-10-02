// SPDX-License-Identifier: GPL-3.0-or-later
//! CA-SCR-1: synthetic pages scrolled in many ways must reassemble pixel-exactly.

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
