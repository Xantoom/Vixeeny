// SPDX-License-Identifier: GPL-3.0-or-later
//! Scrolling capture: the user picked a zone and scrolls the content; this module follows the
//! zone frame by frame (each time the screen changes), assembles the frames and saves the long
//! image. The capture starts with the zone as it is: there is nothing to click before scrolling.
//!
//! The result opens in the editor (scrolling view, whole image selected) unless
//! `[scrolling] annotate` is off, in which case it is saved right away.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

/// The live preview is shown at most this often (about 30 times a second).
const PREVIEW_EVERY: Duration = Duration::from_millis(33);
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

/// An HDR screen: its levels, and a quick SDR rendering of its half floats for matching and
/// for the preview (the picture itself is tone-mapped once, at the end).
struct HdrScreen {
    info: HdrInfo,
    /// Every half float (by its bits) as an sRGB byte.
    quick: Box<[u8]>,
}

impl HdrScreen {
    fn new(info: HdrInfo) -> Self {
        // scRGB: 1.0 is 80 nits; the SDR white of the screen is white.
        let scale = 80.0 / info.sdr_white_nits.max(1.0);
        let quick = (0..=u16::MAX)
            .map(|bits| {
                let v = (half::to_f32(bits) * scale).clamp(0.0, 1.0);
                let v = if v.is_nan() {
                    0.0
                } else if v <= 0.003_130_8 {
                    v * 12.92
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                };
                (v * 255.0 + 0.5) as u8
            })
            .collect();
        Self { info, quick }
    }

    /// A frame of RGBA half floats: its quick rendering (BGRA), and its rows as they are.
    fn frame(&self, frame: &CpuFrame) -> Option<(Frame, Vec<u8>)> {
        let (w, h) = (frame.width as usize, frame.height as usize);
        let mut bgra = Vec::with_capacity(w * h * 4);
        let mut layer = Vec::with_capacity(w * h * 8);
        for y in 0..h {
            let row = frame.data.get(y * frame.stride..)?.get(..w * 8)?;
            layer.extend_from_slice(row);
            for px in row.as_chunks::<8>().0 {
                let c = |i: usize| self.quick[usize::from(u16::from_le_bytes([px[i], px[i + 1]]))];
                bgra.extend_from_slice(&[c(4), c(2), c(0), 255]);
            }
        }
        Some((Frame::new(frame.width, frame.height, bgra)?, layer))
    }
}

/// The light of `rows` rows of RGBA half floats, from row `from`.
fn light(layer: &[u8], width: usize, from: usize, rows: usize) -> Vec<f32> {
    layer[from * width * 8..(from + rows) * width * 8]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| half::to_f32(u16::from_le_bytes(*c)))
        .collect()
}

/// The long picture tone-mapped as every other capture is, a band of rows at a time.
fn tonemapped(layer: &[u8], width: u32, height: u32, info: &HdrInfo) -> Option<Frame> {
    const BAND: usize = 256;
    let (w, h) = (width as usize, height as usize);
    let mut bgra = Vec::with_capacity(w * h * 4);
    for from in (0..h).step_by(BAND) {
        let rows = BAND.min(h - from);
        bgra.extend_from_slice(&crate::tonemap_hdr(&light(layer, w, from, rows), info));
    }
    Frame::new(width, height, bgra)
}

/// The long picture as HDR, from its rows of RGBA half floats.
fn hdr_picture(width: u32, height: u32, layer: &[u8], info: HdrInfo) -> HdrCapture {
    let mut rgba = light(layer, width as usize, 0, height as usize);
    for px in rgba.as_chunks_mut::<4>().0 {
        px[3] = 1.0;
    }
    HdrCapture {
        frame: HdrFrame {
            width,
            height,
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
fn panel_rect(monitor: &PhysicalRect, zone: &PhysicalRect, scale: f32) -> PhysicalRect {
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
    PhysicalRect::new(x as i32, y, width, height)
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

    /// Sleeps until `flag`, the finish or the cancel is set (or `timeout` passed); clears
    /// `arrived`.
    fn wait_for(&self, flag: &AtomicBool, timeout: Option<Duration>) {
        let idle = |(): &mut ()| {
            ![flag, &self.finish, &self.cancel]
                .iter()
                .any(|f| f.load(Ordering::Relaxed))
        };
        if let Ok(held) = self.told.0.lock() {
            match timeout {
                Some(t) => drop(self.told.1.wait_timeout_while(held, t, idle)),
                None => drop(self.told.1.wait_while(held, idle)),
            }
        }
        self.arrived.store(false, Ordering::Relaxed);
    }
}

/// Follows the zone until told to finish. Returns the assembled image, and on an HDR screen
/// its levels (the image's layer then holds the rows as captured, RGBA half floats).
fn capture_loop(
    shared: &Arc<Shared>,
    monitor: MonitorInfo,
    rect: PhysicalRect,
    max_height: u32,
    lang: Lang,
    preview: &vixeeny_ui::scroll_panel::ScrollHandle,
    preview_box: (u32, u32),
) -> anyhow::Result<Option<(Stitched, Option<HdrInfo>)>> {
    // The control window must be out of the capture before the first frame.
    shared.wait_for(&shared.shown, None);
    // An HDR screen is read as it is and tone-mapped here, as every other capture is.
    let hdr = vixeeny_platform::hdr_info(monitor.id).map(HdrScreen::new);
    let target = StreamTarget::Region { monitor, rect };
    let told = shared.clone();
    let stream = VideoStream::start_notify(
        &target,
        false,
        hdr.is_some(),
        Box::new(move || told.tell(&told.arrived)),
    )?;
    let buffers = stream.buffers();
    let mut stitcher = Stitcher::new(vixeeny_stitch::Config {
        max_height,
        ..Default::default()
    });
    let mut warned_full = false;
    // The preview is shown at most this often; the last change always shows.
    let mut shown: Option<Instant> = None;
    let mut unshown = false;
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        // Every frame, in order: the smaller the step between two, the surer the match.
        let mut changed = false;
        while let Some(captured) = stream.recv(Duration::ZERO)? {
            let pushed = match &hdr {
                Some(hdr) if captured.hdr => hdr.frame(&captured.frame).map(|(f, l)| (f, Some(l))),
                _ => tight(&captured.frame).map(|f| (f, None)),
            };
            buffers.give(captured.frame.data);
            let (frame, layer) = pushed.context("unexpected frame")?;
            match stitcher.push_layered(&frame, layer.as_deref())? {
                Push::Full if !warned_full => {
                    warned_full = true;
                    let msg = tr(Key::ScrollTruncated, lang)
                        .replace("{height}", &stitcher.height().to_string());
                    preview.show_status(msg);
                }
                // A frame that cannot be placed is found again as soon as it can.
                _ => {}
            }
            changed = true;
        }
        if shared.finish.load(Ordering::Relaxed) {
            return Ok(stitcher.finish().map(|s| (s, hdr.map(|h| h.info))));
        }
        unshown |= changed && !warned_full;
        let due = shown.map_or(Duration::ZERO, |t| {
            PREVIEW_EVERY.saturating_sub(t.elapsed())
        });
        if unshown && due.is_zero() {
            unshown = false;
            shown = Some(Instant::now());
            if let Some(p) = stitcher.preview(preview_box.0, preview_box.1) {
                let status = tr(Key::ScrollCapturing, lang)
                    .replace("{height}", &stitcher.height().to_string());
                preview.show_preview(p.width, p.height, to_rgba(&p), status);
            }
        }
        // A preview held back shows once its time comes, even if nothing else arrives.
        shared.wait_for(&shared.arrived, unshown.then_some(due));
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
    let at = panel_rect(&monitor.rect, &zone, scale);
    panel.set_position(at.x, at.y, at.width, at.height);

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
    let preview_box = ((PANEL_WIDTH * scale) as u32, at.height);
    let worker = {
        let shared = shared.clone();
        let monitor = monitor.clone();
        let max_height = config.scrolling.max_height.max(100);
        std::thread::Builder::new()
            .name("scroll-capture".into())
            .spawn(move || {
                capture_loop(
                    &shared,
                    monitor,
                    relative,
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
    let mut frame = stitched.frame;
    let mut hdr = None;
    // On an HDR screen, the rows as captured: tone-mapped as every other capture is, and kept
    // as HDR when the picture holds it.
    if let (Some(layer), Some(info)) = (stitched.layer, hdr_info) {
        let (w, h) = (frame.width, frame.height);
        frame = tonemapped(&layer, w, h, &info).context("unexpected stitched image")?;
        let keep = crate::keep_hdr(config) && crate::image_output(config).0.holds_hdr();
        hdr = keep.then(|| hdr_picture(w, h, &layer, info));
    }
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

    /// An HDR frame keeps its light as captured, beside a quick SDR rendering of the same size
    /// whose white is the screen's SDR white.
    #[test]
    fn hdr_rows_round_trip() {
        let info = HdrInfo {
            sdr_white_nits: 240.0,
            peak_nits: 1000.0,
        };
        let white = 240.0 / 80.0;
        let rgba: Vec<f32> = (0..8 * 4)
            .flat_map(|i| [i as f32 * 0.25, white, 6.5, 1.0])
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
        let screen = HdrScreen::new(info);
        let (frame, layer) = screen.frame(&captured).unwrap();
        assert_eq!((frame.width, frame.height), (8, 4));
        assert_eq!(layer.len(), 8 * 4 * 8);
        // Green at the SDR white is full; blue above it too; red 0 is black.
        assert_eq!(&frame.data[..4], &[255, 255, 0, 255]);
        let back = hdr_picture(8, 4, &layer, info);
        assert_eq!(back.info, info);
        for (a, b) in back.frame.rgba.iter().zip(&rgba) {
            assert!((a - b).abs() <= b.abs() * 1e-3, "{a} vs {b}");
        }
        let sdr = tonemapped(&layer, 8, 4, &info).unwrap();
        assert_eq!(sdr.data.len(), 8 * 4 * 4);
    }
}
