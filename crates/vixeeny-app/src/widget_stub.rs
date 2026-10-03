// SPDX-License-Identifier: GPL-3.0-or-later
//! The recording widget does not exist on Linux: neither X11 nor the Wayland portals can leave a window out of a
//! capture, so the pill would end up in the video (CA-REC-5). The recording goes on without it
//! (the tray icon and the shortcuts control it), and `record` only logs why there is no pill.

#![cfg_attr(not(feature = "ffmpeg"), allow(dead_code))]

use std::time::Duration;

use vixeeny_common::config::RecordingWidget;
use vixeeny_platform::MonitorInfo;

use crate::widget_math::FromWidget;

pub struct Widget;

impl Widget {
    pub fn spawn(
        _: &RecordingWidget,
        _: &str,
        _: &MonitorInfo,
        _: impl Fn(FromWidget) + Send + 'static,
    ) -> anyhow::Result<Self> {
        anyhow::bail!(
            "the recording widget is not available on Linux (it could not be kept out of the video)"
        )
    }

    #[allow(clippy::unused_self)]
    pub fn state(&mut self, _: bool, _: Duration) {}
}
