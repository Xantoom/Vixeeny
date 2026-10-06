// SPDX-License-Identifier: GPL-3.0-or-later
//! The visual tree of one window and the arithmetic of what goes where.
//!
//! ```text
//! root (window pixels)
//! ├─ content (image pixels: offset by the window's area and the scroll)
//! │  ├─ frozen tiles · annotated tiles
//! │  ├─ veil (4 rectangles around the zone, one opacity) · border (4) · handles (8)
//! │  └─ size label · magnifier · text field
//! ├─ scroll bar
//! └─ toolbar · style panel · tip
//! ```
//! Rectangles are 1×1 solid surfaces scaled by their visual: moving the zone moves visuals, it
//! draws nothing.

use std::cell::{Cell, RefCell};

use vixeeny_editor::{Rect, RgbaImage};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::DirectComposition::{
    DCOMPOSITION_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, DCOMPOSITION_BORDER_MODE_HARD,
    IDCompositionAnimation, IDCompositionEffectGroup, IDCompositionSurface, IDCompositionTarget,
    IDCompositionVisual, IDCompositionVisual2,
};
use windows::core::Result;
use windows_numerics::Matrix3x2;

use crate::gfx::Gfx;

/// Largest side of one picture surface (textures are limited to 16384; smaller tiles also keep
/// each upload short).
pub const TILE: u32 = 4096;
/// Where a hidden visual goes.
const PARKED: f32 = -1.0e6;

/// The parts of `area` (x, y, w, h in whole pixels) cut in tiles of at most `TILE` per side.
pub fn tiles(area: (u32, u32, u32, u32)) -> Vec<(u32, u32, u32, u32)> {
    let (x, y, w, h) = area;
    let mut out = Vec::new();
    let mut ty = 0;
    while ty < h {
        let th = TILE.min(h - ty);
        let mut tx = 0;
        while tx < w {
            let tw = TILE.min(w - tx);
            out.push((x + tx, y + ty, tw, th));
            tx += tw;
        }
        ty += th;
    }
    out
}

/// A rectangle in whole pixels: `(x, y, w, h)`, empty when `w` or `h` is 0.
pub type Px = (f32, f32, f32, f32);

/// `r` snapped to whole pixels (its edges rounded).
pub fn snap(r: &Rect) -> Px {
    let (x0, y0) = (r.x.round(), r.y.round());
    let (x1, y1) = ((r.x + r.w).round(), (r.y + r.h).round());
    (x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// The veil over `area` around `hole` (both in image pixels): top, bottom, left, right. Without a
/// hole the top one covers everything.
pub fn veil_rects(area: Px, hole: Option<Px>) -> [Px; 4] {
    let (ax, ay, aw, ah) = area;
    let (ar, ab) = (ax + aw, ay + ah);
    let Some((hx, hy, hw, hh)) = hole else {
        return [
            area,
            (0.0, 0.0, 0.0, 0.0),
            (0.0, 0.0, 0.0, 0.0),
            (0.0, 0.0, 0.0, 0.0),
        ];
    };
    let top_end = hy.clamp(ay, ab);
    let bottom_start = (hy + hh).clamp(ay, ab);
    let left_end = hx.clamp(ax, ar);
    let right_start = (hx + hw).clamp(ax, ar);
    let middle = bottom_start - top_end;
    [
        (ax, ay, aw, top_end - ay),
        (ax, bottom_start, aw, ab - bottom_start),
        (ax, top_end, left_end - ax, middle),
        (right_start, top_end, ar - right_start, middle),
    ]
}

/// The zone's border, `width` pixels inside it: top, bottom, left, right.
pub fn border_rects(zone: Px, width: f32) -> [Px; 4] {
    let (x, y, w, h) = zone;
    let t = width.min(w / 2.0).min(h / 2.0).max(0.0);
    [
        (x, y, w, t),
        (x, y + h - t, w, t),
        (x, y + t, t, h - 2.0 * t),
        (x + w - t, y + t, t, h - 2.0 * t),
    ]
}

/// The centres of the eight handles, clockwise from the top-left corner.
pub fn handle_centres(zone: Px) -> [(f32, f32); 8] {
    let (x, y, w, h) = zone;
    let fx = [0.0, 0.5, 1.0, 1.0, 1.0, 0.5, 0.0, 0.0];
    let fy = [0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 0.5];
    std::array::from_fn(|i| (x + w * fx[i], y + h * fy[i]))
}

/// The rising of a value from `from` to `to` over `seconds`, decelerating (1 − (1 − t)³), as an
/// animation the compositor runs on its own.
pub fn ease_out(gfx: &Gfx, from: f32, to: f32, seconds: f64) -> Result<IDCompositionAnimation> {
    let d = seconds as f32;
    let delta = to - from;
    // SAFETY: plain calls on a new animation object.
    unsafe {
        let a = gfx.dcomp.CreateAnimation()?;
        a.AddCubic(
            0.0,
            from,
            3.0 * delta / d,
            -3.0 * delta / (d * d),
            delta / (d * d * d),
        )?;
        a.End(seconds, to)?;
        Ok(a)
    }
}

pub fn visual(gfx: &Gfx) -> Result<IDCompositionVisual2> {
    // SAFETY: plain creation call.
    unsafe { gfx.dcomp.CreateVisual() }
}

/// Adds `child` on top of `parent`'s children.
pub fn add(parent: &IDCompositionVisual2, child: &IDCompositionVisual2) -> Result<()> {
    // SAFETY: both visuals belong to the same device.
    unsafe { parent.AddVisual(child, true, None::<&IDCompositionVisual>) }
}

pub fn set_offset(v: &IDCompositionVisual2, x: f32, y: f32) -> Result<()> {
    // SAFETY: plain property calls.
    unsafe {
        v.SetOffsetX2(x)?;
        v.SetOffsetY2(y)
    }
}

/// A visual that shows `surface` scaled into rectangles (see [`place`]).
fn solid_visual(gfx: &Gfx, surface: &IDCompositionSurface) -> Result<IDCompositionVisual2> {
    let v = visual(gfx)?;
    // SAFETY: plain property calls on a new visual.
    unsafe {
        v.SetContent(surface)?;
        v.SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR)?;
        v.SetBorderMode(DCOMPOSITION_BORDER_MODE_HARD)?;
        v.SetTransform2(&Matrix3x2::scale(0.0, 0.0))?;
    }
    Ok(v)
}

/// Stretches a solid visual over `r` (nothing when it is empty).
pub fn place(v: &IDCompositionVisual2, r: Px) -> Result<()> {
    let (x, y, w, h) = r;
    let m = if w > 0.0 && h > 0.0 {
        Matrix3x2::scale(w, h) * Matrix3x2::translation(x, y)
    } else {
        Matrix3x2::scale(0.0, 0.0)
    };
    // SAFETY: plain property call.
    unsafe { v.SetTransform2(&m) }
}

/// A drawn piece: a visual and its surface, drawn again only when what it shows changes.
pub struct Piece<L> {
    pub visual: IDCompositionVisual2,
    /// Opacity, for the pieces that fade in.
    pub effect: Option<IDCompositionEffectGroup>,
    surface: RefCell<Option<(IDCompositionSurface, (u32, u32))>>,
    pub look: RefCell<Option<L>>,
    /// Where it is (window or image pixels), `None` while hidden.
    pub at: Cell<Option<(f32, f32)>>,
}

impl<L: PartialEq + Clone> Piece<L> {
    pub fn new(gfx: &Gfx, fades: bool) -> Result<Self> {
        let visual = visual(gfx)?;
        let effect = if fades {
            // SAFETY: plain creation and property calls.
            unsafe {
                let e = gfx.dcomp.CreateEffectGroup()?;
                visual.SetEffect(&e)?;
                Some(e)
            }
        } else {
            None
        };
        set_offset(&visual, PARKED, PARKED)?;
        Ok(Self {
            visual,
            effect,
            surface: RefCell::new(None),
            look: RefCell::new(None),
            at: Cell::new(None),
        })
    }

    /// Draws `look` if it is not what the piece shows already. `size` gives the surface size.
    pub fn show(
        &self,
        gfx: &Gfx,
        look: &L,
        size: impl FnOnce() -> Result<(u32, u32)>,
        draw: impl FnOnce(&crate::gfx::Canvas<'_>) -> Result<()>,
    ) -> Result<()> {
        if self.look.borrow().as_ref() == Some(look) {
            return Ok(());
        }
        let (w, h) = size()?;
        let mut slot = self.surface.borrow_mut();
        let reuse = slot.as_ref().is_some_and(|(_, s)| *s == (w, h));
        if !reuse {
            let surface = gfx.surface(w, h)?;
            // SAFETY: plain property call.
            unsafe { self.visual.SetContent(&surface)? };
            *slot = Some((surface, (w, h)));
        }
        if let Some((surface, _)) = slot.as_ref() {
            gfx.draw(surface, w, h, draw)?;
        }
        *self.look.borrow_mut() = Some(look.clone());
        Ok(())
    }

    /// Puts the piece at `(x, y)`; returns whether it was hidden until now.
    pub fn move_to(&self, x: f32, y: f32) -> Result<bool> {
        let was = self.at.replace(Some((x, y)));
        if was != Some((x, y)) {
            set_offset(&self.visual, x, y)?;
        }
        Ok(was.is_none())
    }

    pub fn hide(&self) -> Result<()> {
        if self.at.replace(None).is_some() {
            set_offset(&self.visual, PARKED, PARKED)?;
        }
        Ok(())
    }

    /// Fades the piece in from `rise` pixels lower, over `seconds` (instantly without motion).
    pub fn appear(&self, gfx: &Gfx, rise: f32, seconds: f64, motion: bool) -> Result<()> {
        let (Some(effect), Some((x, y))) = (&self.effect, self.at.get()) else {
            return Ok(());
        };
        let _ = x;
        // SAFETY: plain property calls with animations of the same device.
        unsafe {
            if motion {
                effect.SetOpacity(&ease_out(gfx, 0.0, 1.0, seconds)?)?;
                if rise != 0.0 {
                    self.visual
                        .SetOffsetY(&ease_out(gfx, y + rise, y, seconds)?)?;
                }
            } else {
                effect.SetOpacity2(1.0)?;
            }
        }
        Ok(())
    }
}

/// The pictures of an image part: one visual of tiles.
pub struct Picture {
    pub visual: IDCompositionVisual2,
}

impl Picture {
    pub fn new(gfx: &Gfx) -> Result<Self> {
        Ok(Self {
            visual: visual(gfx)?,
        })
    }

    /// Shows `img`'s `part` with its top-left corner at `(x, y)`.
    pub fn set(
        &self,
        gfx: &Gfx,
        img: &RgbaImage,
        part: (u32, u32, u32, u32),
        x: f32,
        y: f32,
    ) -> Result<()> {
        // SAFETY: plain tree calls on visuals of this device.
        unsafe { self.visual.RemoveAllVisuals()? };
        for tile in tiles(part) {
            let surface = gfx.picture(img, tile)?;
            let v = visual(gfx)?;
            // SAFETY: plain property calls on a new visual.
            unsafe {
                v.SetContent(&surface)?;
                v.SetBitmapInterpolationMode(
                    DCOMPOSITION_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                )?;
                v.SetBorderMode(DCOMPOSITION_BORDER_MODE_HARD)?;
            }
            set_offset(
                &v,
                x + (tile.0 - part.0) as f32,
                y + (tile.1 - part.1) as f32,
            )?;
            add(&self.visual, &v)?;
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        // SAFETY: plain tree call.
        unsafe { self.visual.RemoveAllVisuals() }
    }
}

/// Solid colours shared by every window.
pub struct Solids {
    pub black: IDCompositionSurface,
    pub accent: IDCompositionSurface,
    pub scrollbar: IDCompositionSurface,
    pub handle: IDCompositionSurface,
    pub handle_side: u32,
}

/// What a piece of the toolbar family shows.
pub use crate::paint::{BarLook, FieldLook, PanelLook};

#[derive(Debug, Clone, PartialEq)]
pub struct TipLook {
    pub button: usize,
    pub text: String,
}

/// An image a window shows, and where.
pub type Shown = (std::rc::Rc<RgbaImage>, (f32, f32));

/// One window: a monitor and the part of the image it shows.
pub struct Pane {
    pub hwnd: HWND,
    /// Physical position and size of the window.
    pub position: (i32, i32),
    pub size: (u32, u32),
    /// The part of the image it shows (image pixels).
    pub area: Rect,
    pub _target: IDCompositionTarget,
    pub _root: IDCompositionVisual2,
    pub content: IDCompositionVisual2,
    pub frozen: Picture,
    pub annotated: Picture,
    pub annotated_image: RefCell<Option<Shown>>,
    pub _veil_group: IDCompositionVisual2,
    pub veil_effect: IDCompositionEffectGroup,
    pub veil: [IDCompositionVisual2; 4],
    pub border: [IDCompositionVisual2; 4],
    pub handles: [IDCompositionVisual2; 8],
    pub handles_at: Cell<Option<Px>>,
    pub label: Piece<String>,
    pub magnifier: Piece<vixeeny_editor::MagnifierView>,
    pub field: Piece<FieldLook>,
    pub scrollbar: IDCompositionVisual2,
    pub toolbar: Piece<BarLook>,
    pub panel: Piece<PanelLook>,
    pub tip: Piece<TipLook>,
    /// The content's offset (scroll), to set it only when it changes.
    pub content_at: Cell<Option<(f32, f32)>>,
}

impl Pane {
    pub fn new(
        gfx: &Gfx,
        solids: &Solids,
        hwnd: HWND,
        position: (i32, i32),
        size: (u32, u32),
        area: Rect,
    ) -> Result<Self> {
        // SAFETY: plain creation call for a window of this thread.
        let target = unsafe { gfx.dcomp.CreateTargetForHwnd(hwnd, true)? };
        let root = visual(gfx)?;
        let content = visual(gfx)?;
        add(&root, &content)?;
        let frozen = Picture::new(gfx)?;
        add(&content, &frozen.visual)?;
        let annotated = Picture::new(gfx)?;
        add(&content, &annotated.visual)?;
        let veil_group = visual(gfx)?;
        // SAFETY: plain creation and property calls.
        let veil_effect = unsafe {
            let e = gfx.dcomp.CreateEffectGroup()?;
            e.SetOpacity2(0.0)?;
            veil_group.SetEffect(&e)?;
            e
        };
        add(&content, &veil_group)?;
        let veil = [0; 4].map(|_| solid_visual(gfx, &solids.black));
        let veil = veil.map(|v| v.ok());
        let veil: [IDCompositionVisual2; 4] = match veil {
            [Some(a), Some(b), Some(c), Some(d)] => [a, b, c, d],
            _ => {
                return Err(windows::core::Error::from_hresult(
                    windows::Win32::Foundation::E_FAIL,
                ));
            }
        };
        for v in &veil {
            add(&veil_group, v)?;
        }
        let mut border = Vec::with_capacity(4);
        for _ in 0..4 {
            let v = solid_visual(gfx, &solids.accent)?;
            add(&content, &v)?;
            border.push(v);
        }
        let mut handles = Vec::with_capacity(8);
        for _ in 0..8 {
            let v = visual(gfx)?;
            // SAFETY: plain property call.
            unsafe { v.SetContent(&solids.handle)? };
            set_offset(&v, PARKED, PARKED)?;
            add(&content, &v)?;
            handles.push(v);
        }
        let label = Piece::new(gfx, false)?;
        add(&content, &label.visual)?;
        let magnifier = Piece::new(gfx, false)?;
        add(&content, &magnifier.visual)?;
        let field = Piece::new(gfx, false)?;
        add(&content, &field.visual)?;
        let scrollbar = solid_visual(gfx, &solids.scrollbar)?;
        add(&root, &scrollbar)?;
        let toolbar = Piece::new(gfx, true)?;
        add(&root, &toolbar.visual)?;
        let panel = Piece::new(gfx, true)?;
        add(&root, &panel.visual)?;
        let tip = Piece::new(gfx, false)?;
        add(&root, &tip.visual)?;
        // SAFETY: plain property call.
        unsafe { target.SetRoot(&root)? };
        let (Ok(border), Ok(handles)) = (
            <[IDCompositionVisual2; 4]>::try_from(border),
            <[IDCompositionVisual2; 8]>::try_from(handles),
        ) else {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_FAIL,
            ));
        };
        Ok(Self {
            hwnd,
            position,
            size,
            area,
            _target: target,
            _root: root,
            content,
            frozen,
            annotated,
            annotated_image: RefCell::new(None),
            _veil_group: veil_group,
            veil_effect,
            veil,
            border,
            handles,
            handles_at: Cell::new(None),
            label,
            magnifier,
            field,
            scrollbar,
            toolbar,
            panel,
            tip,
            content_at: Cell::new(None),
        })
    }

    /// The window's area in whole image pixels.
    pub fn area_px(&self) -> Px {
        snap(&self.area)
    }

    /// Image pixels → window pixels.
    pub fn to_window(&self, x: f32, y: f32, scroll: f32) -> (f32, f32) {
        (x - self.area.x, y - self.area.y - scroll)
    }

    /// Window pixels → image pixels.
    pub fn to_image(&self, x: f32, y: f32, scroll: f32) -> (f32, f32) {
        (x + self.area.x, y + self.area.y + scroll)
    }

    /// Whether `r` (image pixels), grown by `margin`, touches this window's area.
    pub fn sees(&self, r: Px, margin: f32) -> bool {
        let (x, y, w, h) = self.area_px();
        r.0 - margin < x + w
            && r.0 + r.2 + margin > x
            && r.1 - margin < y + h
            && r.1 + r.3 + margin > y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_the_area_once() {
        let t = tiles((10, 20, 9000, 5000));
        assert_eq!(t.len(), 6);
        let covered: u64 = t.iter().map(|t| u64::from(t.2) * u64::from(t.3)).sum();
        assert_eq!(covered, 9000 * 5000);
        assert!(t.iter().all(|t| t.2 <= TILE && t.3 <= TILE));
        assert_eq!(t[0], (10, 20, 4096, 4096));
        assert_eq!(t[5], (10 + 8192, 20 + 4096, 808, 904));
    }

    #[test]
    fn the_veil_surrounds_the_hole() {
        let area = (0.0, 0.0, 100.0, 80.0);
        let [top, bottom, left, right] = veil_rects(area, Some((10.0, 20.0, 30.0, 40.0)));
        assert_eq!(top, (0.0, 0.0, 100.0, 20.0));
        assert_eq!(bottom, (0.0, 60.0, 100.0, 20.0));
        assert_eq!(left, (0.0, 20.0, 10.0, 40.0));
        assert_eq!(right, (40.0, 20.0, 60.0, 40.0));
        let total: f32 = [top, bottom, left, right].iter().map(|r| r.2 * r.3).sum();
        assert_eq!(total, 100.0 * 80.0 - 30.0 * 40.0);
    }

    #[test]
    fn a_hole_on_another_window_leaves_this_one_veiled() {
        // The window shows x 100–200; the zone is at x 0–50.
        let area = (100.0, 0.0, 100.0, 80.0);
        let rects = veil_rects(area, Some((0.0, 10.0, 50.0, 20.0)));
        let total: f32 = rects.iter().map(|r| r.2.max(0.0) * r.3.max(0.0)).sum();
        assert_eq!(total, 100.0 * 80.0);
        assert!(rects.iter().all(|r| r.2 >= 0.0 && r.3 >= 0.0));
    }

    #[test]
    fn no_hole_veils_everything() {
        let area = (0.0, 0.0, 10.0, 10.0);
        assert_eq!(veil_rects(area, None)[0], area);
    }

    #[test]
    fn the_border_is_inside_the_zone() {
        let [top, bottom, left, right] = border_rects((10.0, 10.0, 100.0, 50.0), 2.0);
        assert_eq!(top, (10.0, 10.0, 100.0, 2.0));
        assert_eq!(bottom, (10.0, 58.0, 100.0, 2.0));
        assert_eq!(left, (10.0, 12.0, 2.0, 46.0));
        assert_eq!(right, (108.0, 12.0, 2.0, 46.0));
    }

    #[test]
    fn snapping_rounds_the_edges() {
        let r = Rect::new(10.4, 10.6, 20.2, 5.0);
        assert_eq!(snap(&r), (10.0, 11.0, 21.0, 5.0));
    }

    #[test]
    fn handles_go_round_the_zone() {
        let h = handle_centres((0.0, 0.0, 10.0, 20.0));
        assert_eq!(h[0], (0.0, 0.0));
        assert_eq!(h[3], (10.0, 10.0));
        assert_eq!(h[4], (10.0, 20.0));
        assert_eq!(h[7], (0.0, 10.0));
    }
}
