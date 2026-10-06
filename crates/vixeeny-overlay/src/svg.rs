// SPDX-License-Identifier: GPL-3.0-or-later
//! The subset of SVG path data the icons use: move, line, horizontal, vertical, cubic Bézier and
//! close, absolute or relative. Parsed into figures that Direct2D turns into a geometry.

/// One drawing step, in absolute coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    Line(f32, f32),
    Cubic([f32; 6]),
}

/// A closed or open run of segments from `start`.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    pub start: (f32, f32),
    pub segments: Vec<Segment>,
    pub closed: bool,
}

/// The numbers of `data`, split on commas, spaces, signs and second decimal points.
struct Tokens<'a> {
    rest: &'a str,
}

impl Tokens<'_> {
    fn skip_separators(&mut self) {
        self.rest = self
            .rest
            .trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
    }

    fn command(&mut self) -> Option<char> {
        self.skip_separators();
        let c = self.rest.chars().next()?;
        if c.is_ascii_alphabetic() {
            self.rest = &self.rest[1..];
            Some(c)
        } else {
            None
        }
    }

    fn number(&mut self) -> Option<f32> {
        self.skip_separators();
        let bytes = self.rest.as_bytes();
        let mut end = 0;
        if matches!(bytes.first(), Some(b'-' | b'+')) {
            end = 1;
        }
        let mut dot = false;
        let mut exponent = false;
        while let Some(&b) = bytes.get(end) {
            match b {
                b'0'..=b'9' => {}
                b'.' if !dot && !exponent => dot = true,
                b'e' | b'E' if !exponent => {
                    exponent = true;
                    if matches!(bytes.get(end + 1), Some(b'-' | b'+')) {
                        end += 1;
                    }
                }
                _ => break,
            }
            end += 1;
        }
        let value = self.rest[..end].parse().ok()?;
        self.rest = &self.rest[end..];
        Some(value)
    }

    fn at_number(&mut self) -> bool {
        self.skip_separators();
        self.rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
    }
}

/// Parses `data`; `None` when it uses something else than M, L, H, V, C, Z.
pub fn parse(data: &str) -> Option<Vec<Figure>> {
    let mut tokens = Tokens { rest: data };
    let mut figures: Vec<Figure> = Vec::new();
    let mut at = (0.0f32, 0.0f32);
    let mut command = tokens.command()?;
    loop {
        let relative = command.is_ascii_lowercase();
        let base = if relative { at } else { (0.0, 0.0) };
        match command.to_ascii_uppercase() {
            'M' => {
                let (x, y) = (tokens.number()? + base.0, tokens.number()? + base.1);
                at = (x, y);
                figures.push(Figure {
                    start: at,
                    segments: Vec::new(),
                    closed: false,
                });
                // Further pairs after a move are lines.
                command = if relative { 'l' } else { 'L' };
                if !tokens.at_number() {
                    command = tokens.command()?;
                }
                continue;
            }
            'L' => {
                at = (tokens.number()? + base.0, tokens.number()? + base.1);
                figures.last_mut()?.segments.push(Segment::Line(at.0, at.1));
            }
            'H' => {
                at.0 = tokens.number()? + base.0;
                figures.last_mut()?.segments.push(Segment::Line(at.0, at.1));
            }
            'V' => {
                at.1 = tokens.number()? + base.1;
                figures.last_mut()?.segments.push(Segment::Line(at.0, at.1));
            }
            'C' => {
                let mut c = [0.0f32; 6];
                for (i, v) in c.iter_mut().enumerate() {
                    *v = tokens.number()? + if i % 2 == 0 { base.0 } else { base.1 };
                }
                at = (c[4], c[5]);
                figures.last_mut()?.segments.push(Segment::Cubic(c));
            }
            'Z' => {
                let figure = figures.last_mut()?;
                figure.closed = true;
                at = figure.start;
                match tokens.command() {
                    Some(c) => {
                        command = c;
                        continue;
                    }
                    None if tokens.rest.trim().is_empty() => return Some(figures),
                    None => return None,
                }
            }
            _ => return None,
        }
        if tokens.at_number() {
            continue; // the same command again
        }
        match tokens.command() {
            Some(c) => command = c,
            None if tokens.rest.trim().is_empty() => return Some(figures),
            None => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_commands_of_the_icons() {
        let figures = parse("M2 3.5L4-1H10V8C1 2 3 4 5 6ZM1,1 2,2").expect("valid path");
        assert_eq!(figures.len(), 2);
        assert_eq!(figures[0].start, (2.0, 3.5));
        assert_eq!(
            figures[0].segments,
            [
                Segment::Line(4.0, -1.0),
                Segment::Line(10.0, -1.0),
                Segment::Line(10.0, 8.0),
                Segment::Cubic([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            ]
        );
        assert!(figures[0].closed);
        assert_eq!(figures[1].segments, [Segment::Line(2.0, 2.0)]);
        assert!(!figures[1].closed);
    }

    #[test]
    fn relative_commands_add_to_the_current_point() {
        let figures = parse("m1 1l2 0v3h-1z").expect("valid path");
        assert_eq!(
            figures[0].segments,
            [
                Segment::Line(3.0, 1.0),
                Segment::Line(3.0, 4.0),
                Segment::Line(2.0, 4.0),
            ]
        );
    }

    #[test]
    fn numbers_split_on_a_second_decimal_point() {
        let figures = parse("M.5.25L1.5.75").expect("valid path");
        assert_eq!(figures[0].start, (0.5, 0.25));
        assert_eq!(figures[0].segments, [Segment::Line(1.5, 0.75)]);
    }

    #[test]
    fn every_icon_parses() {
        for (name, data) in crate::icons::ALL {
            assert!(parse(data).is_some_and(|f| !f.is_empty()), "{name}");
        }
    }

    #[test]
    fn unknown_commands_are_refused() {
        assert_eq!(parse("M0 0A1 1 0 0 0 2 2"), None);
    }
}
