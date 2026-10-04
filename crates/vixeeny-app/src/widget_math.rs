// SPDX-License-Identifier: GPL-3.0-or-later
//! The pure parts of the recording widget: where it goes and how the two processes talk.
//! (The widget is a process of its own, `vixeeny-app --widget …`, so the action loop of the app
//! keeps answering the daemon while it is on screen.)
#![cfg_attr(not(feature = "ffmpeg"), allow(dead_code))]

/// The window's size at 96 DPI, in pixels: the bar (196 × 40) and 10 px around it for its
/// shadow.
pub const WIDTH: u32 = 216;
pub const HEIGHT: u32 = 60;
/// Gap from the window to the screen edge at 96 DPI (the bar is 16 px from it).
pub const MARGIN: u32 = 6;

/// Top-left corner and size, in physical pixels, of the widget in `corner` of a monitor
/// (`x, y, width, height` of the monitor in physical pixels; `dpi` 96 = 100 %).
pub fn corner_geometry(
    corner: &str,
    monitor: (i32, i32, u32, u32),
    dpi: u32,
) -> (i32, i32, u32, u32) {
    let scale = |v: u32| (u64::from(v) * u64::from(dpi.max(48)) / 96) as u32;
    let (w, h, m) = (scale(WIDTH), scale(HEIGHT), scale(MARGIN));
    let (mx, my, mw, mh) = monitor;
    let left = mx + m as i32;
    let right = mx + mw as i32 - w as i32 - m as i32;
    let top = my + m as i32;
    let bottom = my + mh as i32 - h as i32 - m as i32;
    let (x, y) = match corner {
        "top_right" => (right, top),
        "bottom_left" => (left, bottom),
        "bottom_right" => (right, bottom),
        _ => (left, top),
    };
    (x, y, w, h)
}

/// A message from the recorder to the widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToWidget {
    /// State and the recorded time so far (pauses excluded).
    State {
        paused: bool,
        elapsed_ms: u64,
    },
    Quit,
}

impl ToWidget {
    pub fn to_line(self) -> String {
        match self {
            Self::State { paused, elapsed_ms } => {
                format!(
                    "state {} {elapsed_ms}\n",
                    if paused { "paused" } else { "recording" }
                )
            }
            Self::Quit => "quit\n".into(),
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        let mut parts = line.split_whitespace();
        match parts.next()? {
            "quit" => Some(Self::Quit),
            "state" => {
                let paused = match parts.next()? {
                    "paused" => true,
                    "recording" => false,
                    _ => return None,
                };
                Some(Self::State {
                    paused,
                    elapsed_ms: parts.next()?.parse().ok()?,
                })
            }
            _ => None,
        }
    }
}

/// A button press, from the widget to the recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FromWidget {
    TogglePause,
    Stop,
}

impl FromWidget {
    pub fn to_line(self) -> &'static str {
        match self {
            Self::TogglePause => "pause\n",
            Self::Stop => "stop\n",
        }
    }

    pub fn parse(line: &str) -> Option<Self> {
        match line.trim() {
            "pause" => Some(Self::TogglePause),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_hug_the_edges_with_a_margin_scaled_by_dpi() {
        let monitor = (1920, 0, 2560, 1440); // a second monitor to the right
        assert_eq!(corner_geometry("top_left", monitor, 96), (1926, 6, 216, 60));
        assert_eq!(
            corner_geometry("top_right", monitor, 96),
            (1920 + 2560 - 216 - 6, 6, 216, 60)
        );
        assert_eq!(
            corner_geometry("bottom_left", monitor, 96),
            (1926, 1440 - 60 - 6, 216, 60)
        );
        assert_eq!(
            corner_geometry("bottom_right", monitor, 96),
            (1920 + 2560 - 222, 1440 - 66, 216, 60)
        );
        // 200 % scaling doubles everything; an unknown corner is the default one.
        assert_eq!(
            corner_geometry("???", (0, 0, 3840, 2160), 192),
            (12, 12, 432, 120)
        );
    }

    #[test]
    fn messages_round_trip() {
        for m in [
            ToWidget::State {
                paused: false,
                elapsed_ms: 0,
            },
            ToWidget::State {
                paused: true,
                elapsed_ms: 3_725_000,
            },
            ToWidget::Quit,
        ] {
            assert_eq!(ToWidget::parse(&m.to_line()), Some(m));
        }
        for b in [FromWidget::TogglePause, FromWidget::Stop] {
            assert_eq!(FromWidget::parse(b.to_line()), Some(b));
        }
        for bad in ["", "state", "state running 5", "state paused x", "dance"] {
            assert_eq!(ToWidget::parse(bad), None, "{bad}");
        }
        assert_eq!(FromWidget::parse("go"), None);
    }
}
