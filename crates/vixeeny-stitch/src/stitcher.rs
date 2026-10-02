// SPDX-License-Identifier: GPL-3.0-or-later
//! The assembling state machine.

use crate::frame::Frame;
pub use crate::shift::Direction;
use crate::shift::{self, Tolerance};

#[derive(Debug, Clone)]
pub struct Config {
    /// The assembled image never gets taller than this (plan 5.7: 30 000 px).
    pub max_height: u32,
    /// Smallest overlap between two frames, as a fraction of the scrolling part. Frames that
    /// overlap less are reported as [`Push::Lost`].
    pub min_overlap: f32,
    /// Per-row tolerance on the mean byte difference (0 = exact).
    pub row_tolerance: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_height: 30_000,
            min_overlap: 0.15,
            row_tolerance: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Push {
    /// The first frame.
    First,
    Added {
        rows: u32,
        direction: Direction,
    },
    /// Nothing moved since the previous frame.
    Unchanged,
    /// The content moved too far (or changed) to be matched: scroll back a little.
    Lost,
    /// The height limit is reached; nothing more is added.
    Full,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StitchError {
    #[error("frame size changed from {expected:?} to {got:?}")]
    SizeChanged {
        expected: (u32, u32),
        got: (u32, u32),
    },
}

pub struct Stitched {
    pub frame: Frame,
    /// The height limit cut the image.
    pub truncated: bool,
}

struct Running {
    prev: Frame,
    header: usize,
    footer: usize,
    header_rows: Vec<u8>,
    footer_rows: Vec<u8>,
    /// Rows added above the body, most recent last (so the last chunk is the topmost).
    above: Vec<Vec<u8>>,
    body: Vec<u8>,
    rows: usize,
    /// Top of the previous frame's scrolling part, relative to the first body row.
    pos: isize,
    /// Rows prepended above the first body row.
    above_rows: usize,
}

enum State {
    Empty,
    One(Frame),
    Running(Box<Running>),
}

pub struct Stitcher {
    config: Config,
    state: State,
    truncated: bool,
}

impl Stitcher {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            state: State::Empty,
            truncated: false,
        }
    }

    /// Height of the image assembled so far.
    pub fn height(&self) -> u32 {
        match &self.state {
            State::Empty => 0,
            State::One(f) => f.height,
            State::Running(r) => r.rows as u32,
        }
    }

    fn tolerance(&self) -> Tolerance {
        Tolerance {
            row_mean: self.config.row_tolerance,
        }
    }

    fn min_overlap(&self, region_rows: usize) -> usize {
        ((region_rows as f32 * self.config.min_overlap) as usize)
            .max(4)
            .min(region_rows.saturating_sub(1))
    }

    pub fn push(&mut self, frame: &Frame) -> Result<Push, StitchError> {
        let state = std::mem::replace(&mut self.state, State::Empty);
        let (state, outcome) = match state {
            State::Empty => (State::One(frame.clone()), Push::First),
            State::One(prev) => self.second(prev, frame)?,
            State::Running(run) => self.next(run, frame)?,
        };
        self.state = state;
        Ok(outcome)
    }

    fn check_size(prev: &Frame, frame: &Frame) -> Result<(), StitchError> {
        if (prev.width, prev.height) == (frame.width, frame.height) {
            Ok(())
        } else {
            Err(StitchError::SizeChanged {
                expected: (prev.width, prev.height),
                got: (frame.width, frame.height),
            })
        }
    }

    /// The second frame decides which rows are a sticky header/footer.
    fn second(&mut self, prev: Frame, frame: &Frame) -> Result<(State, Push), StitchError> {
        if let Err(e) = Self::check_size(&prev, frame) {
            self.state = State::One(prev);
            return Err(e);
        }
        let tol = self.tolerance();
        let h = prev.height as usize;
        let top = shift::fixed_rows(&prev, frame, true, tol);
        if top == h {
            return Ok((State::One(prev), Push::Unchanged));
        }
        let mut header = top;
        let mut footer = shift::fixed_rows(&prev, frame, false, tol).min(h - header);
        // Keep a scrolling part worth comparing.
        while h - header - footer < 8 && (header > 0 || footer > 0) {
            if header >= footer {
                header -= 1
            } else {
                footer -= 1
            }
        }
        let range = (header, h - footer);
        let found = shift::find(
            &prev,
            frame,
            range,
            tol,
            self.min_overlap(range.1 - range.0),
        );
        let s = match found {
            Err(()) => return Ok((State::One(prev), Push::Lost)),
            Ok(None) => return Ok((State::One(prev), Push::Unchanged)),
            Ok(Some(s)) => s,
        };
        // Rows that look fixed but also agree with the shift are content (blank margins).
        while header > 0 && agrees_with_shift(&prev, frame, header - 1, s) {
            header -= 1;
        }
        while footer > 0 && agrees_with_shift(&prev, frame, h - footer, s) {
            footer -= 1;
        }
        let rb = prev.row_bytes();
        let mut run = Running {
            header_rows: prev.data[..header * rb].to_vec(),
            footer_rows: Vec::new(),
            above: Vec::new(),
            body: prev.data[header * rb..(h - footer) * rb].to_vec(),
            rows: h,
            header,
            footer,
            prev: prev.clone(),
            pos: 0,
            above_rows: 0,
        };
        let outcome = self.apply(&mut run, frame, s);
        Ok((State::Running(Box::new(run)), outcome))
    }

    fn next(&mut self, mut run: Box<Running>, frame: &Frame) -> Result<(State, Push), StitchError> {
        if let Err(e) = Self::check_size(&run.prev, frame) {
            self.state = State::Running(run);
            return Err(e);
        }
        let tol = self.tolerance();
        let h = frame.height as usize;
        let range = (run.header, h - run.footer);
        match shift::find(
            &run.prev,
            frame,
            range,
            tol,
            self.min_overlap(range.1 - range.0),
        ) {
            Err(()) => Ok((State::Running(run), Push::Lost)),
            Ok(None) => Ok((State::Running(run), Push::Unchanged)),
            Ok(Some(s)) => {
                let outcome = self.apply(&mut run, frame, s);
                Ok((State::Running(run), outcome))
            }
        }
    }

    /// Adds the rows `frame` brings beyond `run.prev`.
    fn apply(&mut self, run: &mut Running, frame: &Frame, s: shift::Shift) -> Push {
        let h = frame.height as usize;
        let rb = frame.row_bytes();
        let n = h - run.header - run.footer;
        let body_rows = run.body.len() / rb;
        let pos = match s.direction {
            Direction::Down => run.pos + s.rows as isize,
            Direction::Up => run.pos - s.rows as isize,
        };
        // Rows of this frame's scrolling part that extend the assembled image.
        let (wanted, up) = match s.direction {
            Direction::Down => (
                (pos + n as isize - body_rows as isize).max(0) as usize,
                false,
            ),
            Direction::Up => ((-(run.above_rows as isize) - pos).max(0) as usize, true),
        };
        let room = (self.config.max_height as usize).saturating_sub(run.rows);
        let take = wanted.min(room);
        if take < wanted {
            self.truncated = true;
        }
        if take > 0 {
            if up {
                let from = run.header;
                run.above
                    .push(frame.data[(from + wanted - take) * rb..(from + wanted) * rb].to_vec());
                run.above_rows += take;
            } else {
                let from = h - run.footer - wanted;
                run.body
                    .extend_from_slice(&frame.data[from * rb..(from + take) * rb]);
            }
            run.rows += take;
        }
        // When truncated, later frames keep tracking the true position.
        run.pos = pos;
        run.footer_rows = frame.data[(h - run.footer) * rb..].to_vec();
        run.prev = frame.clone();
        if take == 0 && wanted > 0 {
            Push::Full
        } else if take == 0 {
            Push::Unchanged
        } else {
            Push::Added {
                rows: take as u32,
                direction: s.direction,
            }
        }
    }

    fn assemble(&self) -> Option<Frame> {
        match &self.state {
            State::Empty => None,
            State::One(f) => Some(f.clone()),
            State::Running(r) => {
                let w = r.prev.width;
                let mut data = Vec::with_capacity(r.rows * r.prev.row_bytes());
                data.extend_from_slice(&r.header_rows);
                for chunk in r.above.iter().rev() {
                    data.extend_from_slice(chunk);
                }
                data.extend_from_slice(&r.body);
                data.extend_from_slice(&r.footer_rows);
                Frame::new(w, (data.len() / r.prev.row_bytes()) as u32, data)
            }
        }
    }

    /// The image so far, for the live preview (downscaled to fit).
    pub fn preview(&self, max_width: u32, max_height: u32) -> Option<Frame> {
        self.assemble().map(|f| f.thumbnail(max_width, max_height))
    }

    /// The final image. `None` when no frame was pushed.
    pub fn finish(self) -> Option<Stitched> {
        let frame = self.assemble()?;
        Some(Stitched {
            frame,
            truncated: self.truncated,
        })
    }
}

/// Whether row `y` of `new` equals the row of `prev` it would have come from under shift `s`.
fn agrees_with_shift(prev: &Frame, new: &Frame, y: usize, s: shift::Shift) -> bool {
    let from = match s.direction {
        Direction::Down => y + s.rows,
        Direction::Up => match y.checked_sub(s.rows) {
            Some(v) => v,
            None => return false,
        },
    };
    from < prev.height as usize && prev.row(from) == new.row(y)
}
