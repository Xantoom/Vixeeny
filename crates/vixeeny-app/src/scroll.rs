// SPDX-License-Identifier: GPL-3.0-or-later
//! Scrolling capture: the user picked a zone and scrolls the content; this module follows the
//! zone frame by frame (each time the screen changes), assembles the frames and saves the long
//! image. The capture starts with the zone as it is: there is nothing to click before scrolling.
//!
//! The result opens in the editor (scrolling view, whole image selected) unless
//! `[scrolling] annotate` is off, in which case it is saved right away.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;
use vixeeny_capture::{CpuFrame, HdrCapture, HdrFrame, StreamTarget, VideoStream};
use vixeeny_common::config::Config;
use vixeeny_common::half;
use vixeeny_common::i18n::{Key, Lang, tr};
use vixeeny_common::ipc::ActionId;
use vixeeny_editor::{RgbaImage, Session};
use vixeeny_image::Bgra;
use vixeeny_platform::{HdrInfo, MonitorInfo, PhysicalRect};
use vixeeny_stitch::{Frame, Push, Stitched, Stitcher};
use vixeeny_ui::scroll_panel::{ScrollEvent, ScrollPanel, ScrollTexts};

use crate::still::Snapshot;

/// Logical width of the control window.
const PANEL_WIDTH: f32 = 260.0;
const PANEL_HEIGHT: f32 = 420.0;

fn texts(lang: Lang) -> ScrollTexts {
    ScrollTexts {
        title: tr(Key::ScrollTitle, lang).into(),
        intro: tr(Key::ScrollIntro, lang).into(),
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

/// A frame of RGBA half floats (an HDR screen): what is matched (its SDR rendering, BGRA), and
/// with `keep` what is kept with it (RGB half floats).
fn hdr_frame(frame: &CpuFrame, info: &HdrInfo, keep: bool) -> Option<(Frame, Option<Vec<u8>>)> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    let mut layer = keep.then(|| Vec::with_capacity(w * h * 6));
    for y in 0..h {
        let row = frame.data.get(y * frame.stride..)?.get(..w * 8)?;
        for px in row.as_chunks::<8>().0 {
            let c = |i: usize| half::to_f32(u16::from_le_bytes([px[i], px[i + 1]]));
            rgba.extend_from_slice(&[c(0), c(2), c(4), 1.0]);
            if let Some(layer) = &mut layer {
                layer.extend_from_slice(&px[..6]);
            }
        }
    }
    let bgra = crate::tonemap_hdr(&rgba, info);
    Some((Frame::new(frame.width, frame.height, bgra)?, layer))
}

/// The long picture as HDR, from the layer of RGB half floats.
fn hdr_picture(frame: &Frame, layer: &[u8], info: HdrInfo) -> HdrCapture {
    let rgba = layer
        .as_chunks::<6>()
        .0
        .iter()
        .flat_map(|px| {
            let c = |i: usize| half::to_f32(u16::from_le_bytes([px[i], px[i + 1]]));
            [c(0), c(2), c(4), 1.0]
        })
        .collect();
    HdrCapture {
        frame: HdrFrame {
            width: frame.width,
            height: frame.height,
            rgba,
        },
        info,
    }
}

/// BGRA → RGBA for the preview.
fn to_rgba(frame: &Frame) -> Vec<u8> {
    let mut out = Vec::with_capacity(frame.data.len());
    for px in frame.data.as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    out
}

/// Where to put the control window: beside the zone when there is room. It is left out of the
/// capture anyway, but should not hide what the user scrolls.
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
        zone.right() - i64::from(width)
    };
    (x as i32, y, width, height)
}

#[derive(Default)]
struct Shared {
    /// The control window is on screen and left out of the capture.
    shown: AtomicBool,
    /// A frame waits in the stream.
    arrived: AtomicBool,
    finish: AtomicBool,
    cancel: AtomicBool,
    /// Wakes the capture thread.
    told: (std::sync::Mutex<()>, std::sync::Condvar),
}

impl Shared {
    fn tell(&self, flag: &AtomicBool) {
        let _held = self.told.0.lock();
        flag.store(true, Ordering::Relaxed);
        self.told.1.notify_all();
    }

    /// Sleeps until one of `flags`, the finish or the cancel is set; clears `arrived`.
    fn wait_for(&self, flag: &AtomicBool) {
        if let Ok(held) = self.told.0.lock() {
            let _held = self.told.1.wait_while(held, |()| {
                ![flag, &self.finish, &self.cancel]
                    .iter()
                    .any(|f| f.load(Ordering::Relaxed))
            });
        }
        self.arrived.store(false, Ordering::Relaxed);
    }
}

/// Follows the zone until told to finish. Returns the assembled image, and the levels of its
/// HDR layer when HDR is kept (`keep_hdr`) on an HDR screen.
#[allow(clippy::too_many_arguments)]
fn capture_loop(
    shared: &Arc<Shared>,
    monitor: MonitorInfo,
    rect: PhysicalRect,
    keep_hdr: bool,
    max_height: u32,
    lang: Lang,
    preview: &vixeeny_ui::scroll_panel::ScrollHandle,
    preview_box: (u32, u32),
) -> anyhow::Result<Option<(Stitched, Option<HdrInfo>)>> {
    // The control window must be out of the capture before the first frame.
    shared.wait_for(&shared.shown);
    // An HDR screen is read as it is and tone-mapped here, as every other capture is.
    let hdr_info = vixeeny_platform::hdr_info(monitor.id);
    let target = StreamTarget::Region { monitor, rect };
    let told = shared.clone();
    let stream = VideoStream::start_notify(
        &target,
        false,
        hdr_info.is_some(),
        Box::new(move || told.tell(&told.arrived)),
    )?;
    let buffers = stream.buffers();
    let mut stitcher = Stitcher::new(vixeeny_stitch::Config {
        max_height,
        ..Default::default()
    });
    let mut lost = false;
    let mut warned_full = false;
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        // Every frame, in order: the smaller the step between two, the surer the match.
        let mut changed = false;
        while let Some(captured) = stream.recv(Duration::ZERO)? {
            let pushed = match &hdr_info {
                Some(info) if captured.hdr => hdr_frame(&captured.frame, info, keep_hdr),
                _ => tight(&captured.frame).map(|f| (f, None)),
            };
            buffers.give(captured.frame.data);
            let (frame, layer) = pushed.context("unexpected frame")?;
            match stitcher.push_layered(&frame, layer.as_deref())? {
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
            changed = true;
        }
        if shared.finish.load(Ordering::Relaxed) {
            let info = hdr_info.filter(|_| keep_hdr);
            return Ok(stitcher.finish().map(|s| (s, info)));
        }
        if changed
            && !warned_full
            && let Some(p) = stitcher.preview(preview_box.0, preview_box.1)
        {
            let status = if lost {
                tr(Key::ScrollLost, lang).to_owned()
            } else {
                tr(Key::ScrollCapturing, lang).replace("{height}", &stitcher.height().to_string())
            };
            preview.show_preview(p.width, p.height, to_rgba(&p), status);
        }
        shared.wait_for(&shared.arrived);
    }
}

pub fn run(
    config: &Config,
    snapshot: Snapshot,
    zone: PhysicalRect,
    scale: f32,
) -> anyhow::Result<()> {
    let lang = crate::lang(&config.general.language);
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
    let panel = ScrollPanel::new(&texts(lang)).map_err(|e| anyhow::anyhow!("window: {e}"))?;
    let (px, py, pw, ph) = panel_rect(&monitor.rect, &zone, scale);
    panel.set_position(px, py, pw, ph);

    let shared = Arc::new(Shared::default());
    // Out of the capture as soon as Windows made the window.
    let excluded = shared.clone();
    vixeeny_ui::theme::when_native(panel.window(), move |handle| {
        if let Err(e) = vixeeny_platform::exclude_from_capture(vixeeny_platform::WindowId(handle)) {
            tracing::warn!("the control window may show in the capture: {e}");
        }
        excluded.tell(&excluded.shown);
    });
    let handle = panel.handle();
    let preview_box = ((PANEL_WIDTH * scale) as u32, ph);
    let worker = {
        let shared = shared.clone();
        let monitor = monitor.clone();
        let max_height = config.scrolling.max_height.max(100);
        let keep_hdr = crate::keep_hdr(config) && crate::image_output(config).0.holds_hdr();
        std::thread::Builder::new()
            .name("scroll-capture".into())
            .spawn(move || {
                capture_loop(
                    &shared,
                    monitor,
                    relative,
                    keep_hdr,
                    max_height,
                    lang,
                    &handle,
                    preview_box,
                )
            })?
    };

    let events = shared.clone();
    let ran = panel.run(move |event| match event {
        ScrollEvent::Finish => events.tell(&events.finish),
        ScrollEvent::Cancel => events.tell(&events.cancel),
    });
    // Whatever way the window went away, the worker must stop.
    if !shared.finish.load(Ordering::Relaxed) {
        shared.tell(&shared.cancel);
    }
    let result = worker
        .join()
        .map_err(|_| anyhow::anyhow!("the capture thread panicked"))?;
    ran.map_err(|e| anyhow::anyhow!("window: {e}"))?;
    let Some((stitched, hdr_info)) = result? else {
        tracing::debug!("scrolling capture cancelled");
        return Ok(());
    };
    if stitched.truncated {
        tracing::warn!("scrolling capture cut at {} px", stitched.frame.height);
    }
    let frame = stitched.frame;
    let hdr = stitched
        .layer
        .zip(hdr_info)
        .map(|(layer, info)| hdr_picture(&frame, &layer, info));
    if config.scrolling.annotate {
        match annotate(
            config,
            snapshot.clone(),
            &monitor,
            scale,
            &frame,
            hdr.as_ref(),
        ) {
            Ok(()) => return Ok(()),
            // Saving what was captured beats losing it.
            Err(e) => tracing::error!("editor: {e:#}, saving directly"),
        }
    }
    let bitmap = Bgra::new(
        frame.width,
        frame.height,
        frame.width as usize * 4,
        &frame.data,
    );
    let path = crate::region::save_bgra(
        config,
        &snapshot,
        ActionId::CaptureScrolling,
        &bitmap,
        hdr.as_ref(),
        None,
    )?;
    tracing::info!("scrolling capture saved ({}x{})", frame.width, frame.height);
    tracing::debug!("saved {}", path.display());
    Ok(())
}

/// Shows the long image in the editor, on the monitor it was captured from.
fn annotate(
    config: &Config,
    snapshot: Snapshot,
    monitor: &MonitorInfo,
    scale: f32,
    frame: &Frame,
    hdr: Option<&HdrCapture>,
) -> anyhow::Result<()> {
    let hdr = hdr.cloned();
    let base = RgbaImage::from_bgra(
        frame.width,
        frame.height,
        frame.width as usize * 4,
        &frame.data,
    )
    .context("unexpected stitched image")?;
    let mut session = Session::new(base, Vec::new());
    session.select_all();
    session.dim = 0.0;
    let config = config.clone();
    let tips = crate::region::toolbar_tips(crate::lang(&config.general.language));
    let screen = vixeeny_overlay::Screen {
        position: (monitor.rect.x, monitor.rect.y),
        size: (monitor.rect.width, monitor.rect.height),
        area: vixeeny_editor::Rect::new(0.0, 0.0, frame.width as f32, frame.height as f32),
    };
    let screens = [screen];
    let overlay = vixeeny_overlay::Overlay::on_screens(
        session,
        scale,
        &screens,
        crate::overlay_look(),
        move |command, session| {
            crate::region::output_command(
                &config,
                &snapshot,
                ActionId::CaptureScrolling,
                command,
                session,
                hdr.as_ref(),
            )
            .unwrap_or(false)
        },
    )
    .map_err(|e| anyhow::anyhow!("cannot create the editor window: {e}"))?;
    overlay.set_tips(&tips);
    overlay.set_scrolling(true);
    overlay
        .run((monitor.rect.x, monitor.rect.y))
        .map_err(|e| anyhow::anyhow!("editor window: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The HDR layer carried by the stitcher gives back the captured light, beside an SDR
    /// rendering of the same size.
    #[test]
    fn hdr_layer_round_trips() {
        let info = HdrInfo {
            sdr_white_nits: 240.0,
            peak_nits: 1000.0,
        };
        let rgba: Vec<f32> = (0..8 * 4)
            .flat_map(|i| [i as f32 * 0.25, 1.0, 6.5, 1.0])
            .collect();
        // Rows of 8 pixels, 16 bytes of padding after each.
        let stride = 8 * 8 + 16;
        let mut data = vec![0u8; stride * 4];
        for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let at = (i / 8) * stride + (i % 8) * 8;
            for (c, v) in px.iter().enumerate() {
                data[at + c * 2..at + c * 2 + 2].copy_from_slice(&half::from_f32(*v).to_le_bytes());
            }
        }
        let captured = CpuFrame::from_raw(8, 4, stride, data).unwrap();
        let (frame, layer) = hdr_frame(&captured, &info, true).unwrap();
        let layer = layer.unwrap();
        assert_eq!((frame.width, frame.height), (8, 4));
        assert_eq!(layer.len(), 8 * 4 * 6);
        let back = hdr_picture(&frame, &layer, info);
        assert_eq!(back.info, info);
        for (a, b) in back.frame.rgba.iter().zip(&rgba) {
            assert!((a - b).abs() <= b.abs() * 1e-3, "{a} vs {b}");
        }
        // Not kept: only what is matched.
        assert!(hdr_frame(&captured, &info, false).unwrap().1.is_none());
    }
}
