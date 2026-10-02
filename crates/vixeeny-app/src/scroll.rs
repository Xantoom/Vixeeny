// SPDX-License-Identifier: GPL-3.0-or-later
//! Scrolling capture (plan 5.7): the user picked a zone and scrolls the content; this module
//! captures the zone about 15 times a second, assembles the frames and saves the long image.
//!
//! Deviation from the plan: the result is saved (and copied, if configured) right away. The
//! annotation overlay is a frozen screen and cannot show an image taller than it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use vixeeny_capture::{CaptureOptions, CaptureTarget, Capturer, CpuFrame, WgcBackend};
use vixeeny_common::config::Config;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::ActionId;
use vixeeny_image::Bgra;
use vixeeny_platform::{MonitorInfo, PhysicalRect};
use vixeeny_stitch::{Frame, Push, Stitched, Stitcher};
use vixeeny_ui::scroll_panel::{ScrollEvent, ScrollPanel, ScrollTexts};

use crate::still::Snapshot;

const FRAME_INTERVAL: Duration = Duration::from_millis(66);
const PREVIEW_INTERVAL: Duration = Duration::from_millis(250);
/// Logical width of the control window.
const PANEL_WIDTH: f32 = 260.0;
const PANEL_HEIGHT: f32 = 420.0;

fn texts(lang: Lang) -> ScrollTexts {
    ScrollTexts {
        title: tr(Key::ScrollTitle, lang).into(),
        intro: tr(Key::ScrollIntro, lang).into(),
        start: tr(Key::ScrollStart, lang).into(),
        finish: tr(Key::ScrollFinish, lang).into(),
        cancel: tr(Key::ScrollCancel, lang).into(),
    }
}

fn tight(frame: &CpuFrame) -> Option<Frame> {
    let mut data = Vec::with_capacity(frame.width as usize * frame.height as usize * 4);
    for y in 0..frame.height {
        data.extend_from_slice(frame.row(y));
    }
    Frame::new(frame.width, frame.height, data)
}

/// BGRA → RGBA for the preview.
fn to_rgba(frame: &Frame) -> Vec<u8> {
    let mut out = Vec::with_capacity(frame.data.len());
    for px in frame.data.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    out
}

/// Where to put the control window so that it does not hide the zone (the capture would see it).
fn panel_rect(monitor: &PhysicalRect, zone: &PhysicalRect, scale: f32) -> (i32, i32, u32, u32) {
    let width = (PANEL_WIDTH * scale) as u32;
    let height = ((PANEL_HEIGHT * scale) as u32)
        .max(zone.height.min(monitor.height))
        .min(monitor.height);
    let gap = (12.0 * scale) as i64;
    let y = i64::from(zone.y)
        .min(monitor.bottom() - i64::from(height))
        .max(i64::from(monitor.y)) as i32;
    let right = zone.right() + gap;
    let left = i64::from(zone.x) - gap - i64::from(width);
    let x = if right + i64::from(width) <= monitor.right() {
        right
    } else if left >= i64::from(monitor.x) {
        left
    } else {
        tracing::warn!("no room beside the zone: the control window will be captured");
        zone.right() - i64::from(width)
    };
    (x as i32, y, width, height)
}

struct Shared {
    started: AtomicBool,
    finish: AtomicBool,
    cancel: AtomicBool,
}

/// Captures until told to finish. Returns the assembled image.
fn capture_loop(
    shared: &Shared,
    monitors: Vec<MonitorInfo>,
    target: CaptureTarget,
    max_height: u32,
    lang: Lang,
    preview: &vixeeny_ui::scroll_panel::ScrollHandle,
    preview_box: (u32, u32),
) -> anyhow::Result<Option<Stitched>> {
    let mut capturer = Capturer::new(WgcBackend::new()?, monitors);
    let options = CaptureOptions {
        show_cursor: false,
        tonemap: None,
    };
    let mut stitcher = Stitcher::new(vixeeny_stitch::Config {
        max_height,
        ..Default::default()
    });
    let mut last_preview = Instant::now() - PREVIEW_INTERVAL;
    let mut lost = false;
    let mut warned_full = false;
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if shared.finish.load(Ordering::Relaxed) {
            return Ok(stitcher.finish());
        }
        if !shared.started.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(30));
            continue;
        }
        let tick = Instant::now();
        let grabbed = capturer.grab(&target, options)?;
        let frame = tight(&grabbed).context("unexpected frame")?;
        match stitcher.push(&frame)? {
            Push::Lost => lost = true,
            Push::Added { .. } | Push::First => lost = false,
            Push::Full if !warned_full => {
                warned_full = true;
                let msg = tr(Key::ScrollTruncated, lang)
                    .replace("{height}", &stitcher.height().to_string());
                preview.show_status(msg);
            }
            Push::Full | Push::Unchanged => {}
        }
        if last_preview.elapsed() >= PREVIEW_INTERVAL && !warned_full {
            last_preview = Instant::now();
            if let Some(p) = stitcher.preview(preview_box.0, preview_box.1) {
                let status = if lost {
                    tr(Key::ScrollLost, lang).to_owned()
                } else {
                    tr(Key::ScrollCapturing, lang)
                        .replace("{height}", &stitcher.height().to_string())
                };
                preview.show_preview(p.width, p.height, to_rgba(&p), status);
            }
        }
        if let Some(rest) = FRAME_INTERVAL.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

pub fn run(
    config: &Config,
    snapshot: Snapshot,
    zone: PhysicalRect,
    scale: f32,
) -> anyhow::Result<()> {
    let lang = Lang::resolve(&config.general.language, None);
    let monitor = vixeeny_platform::monitor_for_rect(&snapshot.monitors, &zone)
        .context("no monitor for the zone")?
        .clone();
    // The zone, relative to its monitor and clipped to it.
    let x0 = i64::from(zone.x).max(i64::from(monitor.rect.x));
    let y0 = i64::from(zone.y).max(i64::from(monitor.rect.y));
    let x1 = zone.right().min(monitor.rect.right());
    let y1 = zone.bottom().min(monitor.rect.bottom());
    anyhow::ensure!(x1 > x0 && y1 > y0, "the zone is empty");
    let relative = PhysicalRect::new(
        (x0 - i64::from(monitor.rect.x)) as i32,
        (y0 - i64::from(monitor.rect.y)) as i32,
        (x1 - x0) as u32,
        (y1 - y0) as u32,
    );
    let target = CaptureTarget::Region {
        monitor: monitor.id,
        rect: relative,
    };

    let panel = ScrollPanel::new(&texts(lang)).map_err(|e| anyhow::anyhow!("window: {e}"))?;
    let (px, py, pw, ph) = panel_rect(&monitor.rect, &zone, scale);
    panel.set_position(px, py, pw, ph);

    let shared = Arc::new(Shared {
        started: AtomicBool::new(false),
        finish: AtomicBool::new(false),
        cancel: AtomicBool::new(false),
    });
    let handle = panel.handle();
    let preview_box = ((PANEL_WIDTH * scale) as u32, ph);
    let worker = {
        let shared = shared.clone();
        let monitors = snapshot.monitors.clone();
        let max_height = config.scrolling.max_height.max(100);
        std::thread::Builder::new()
            .name("scroll-capture".into())
            .spawn(move || {
                capture_loop(
                    &shared,
                    monitors,
                    target,
                    max_height,
                    lang,
                    &handle,
                    preview_box,
                )
            })?
    };

    let events = shared.clone();
    let ran = panel.run(move |event| match event {
        ScrollEvent::Start => events.started.store(true, Ordering::Relaxed),
        ScrollEvent::Finish => events.finish.store(true, Ordering::Relaxed),
        ScrollEvent::Cancel => events.cancel.store(true, Ordering::Relaxed),
    });
    // Whatever way the window went away, the worker must stop.
    if !shared.finish.load(Ordering::Relaxed) {
        shared.cancel.store(true, Ordering::Relaxed);
    }
    let result = worker
        .join()
        .map_err(|_| anyhow::anyhow!("the capture thread panicked"))?;
    ran.map_err(|e| anyhow::anyhow!("window: {e}"))?;
    let Some(stitched) = result? else {
        tracing::info!("scrolling capture cancelled");
        return Ok(());
    };
    if stitched.truncated {
        tracing::warn!("scrolling capture cut at {} px", stitched.frame.height);
    }
    let frame = stitched.frame;
    let bitmap = Bgra::new(
        frame.width,
        frame.height,
        frame.width as usize * 4,
        &frame.data,
    );
    let path =
        crate::region::save_bgra(config, &snapshot, ActionId::CaptureScrolling, &bitmap, None)?;
    tracing::info!(
        "saved {} ({}x{})",
        path.display(),
        frame.width,
        frame.height
    );
    if config.image.copy_to_clipboard
        && let Err(e) = crate::region::copy_bgra(&bitmap)
    {
        tracing::warn!("clipboard: {e:#}");
    }
    Ok(())
}
