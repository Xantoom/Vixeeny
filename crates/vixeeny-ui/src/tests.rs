//! Headless rendering with Slint's software renderer: checks layouts without a display.
//! Set `VIXEENY_SCREENSHOTS=<dir>` to also write the rendered frames as PNG files.

use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel, SharedPixelBuffer};
use vixeeny_editor::{Color, Modifiers, Point, RgbaImage, Session, Tool};

use super::*;

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

thread_local! {
    static WINDOW: Rc<MinimalSoftwareWindow> = {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        let _ = slint::platform::set_platform(Box::new(Headless(w.clone())));
        w
    };
}

fn screen(w: u32, h: u32) -> RgbaImage {
    // a gradient with a few "windows", so the veil and the zone are easy to see
    let mut img = RgbaImage::filled(w, h, Color::rgb(0, 0, 0));
    for y in 0..h {
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            let (r, g, b) = if (60..300).contains(&x) && (50..220).contains(&y) {
                (240, 240, 245)
            } else {
                ((x * 255 / w) as u8, (y * 255 / h) as u8, 120)
            };
            img.data[o..o + 3].copy_from_slice(&[r, g, b]);
        }
    }
    img
}

fn render(overlay: &Overlay, w: u32, h: u32) -> SharedPixelBuffer<Rgb8Pixel> {
    let window = WINDOW.with(Rc::clone);
    window.set_size(PhysicalSize::new(w, h));
    overlay.window().show().unwrap_or_else(|e| panic!("{e}"));
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    window.draw_if_needed(|renderer| {
        renderer.render(buffer.make_mut_slice(), w as usize);
    });
    buffer
}

fn save(name: &str, buf: &SharedPixelBuffer<Rgb8Pixel>) {
    let Ok(dir) = std::env::var("VIXEENY_SCREENSHOTS") else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let Ok(file) = std::fs::File::create(std::path::Path::new(&dir).join(format!("{name}.png")))
    else {
        return;
    };
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), buf.width(), buf.height());
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let Ok(mut writer) = enc.write_header() else {
        return;
    };
    let bytes: Vec<u8> = buf
        .as_slice()
        .iter()
        .flat_map(|p| [p.r, p.g, p.b])
        .collect();
    let _ = writer.write_image_data(&bytes);
}

fn overlay(w: u32, h: u32, scale: f32) -> Overlay {
    WINDOW.with(|_| ()); // installs the platform before any component exists
    let session = Session::new(
        screen(w, h),
        vec![vixeeny_editor::Rect::new(60.0, 50.0, 240.0, 170.0)],
    );
    Overlay::new(session, scale, |_, _, _| true).unwrap_or_else(|e| panic!("{e}"))
}

const NO: Modifiers = Modifiers { shift: false };

fn p(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

#[test]
fn before_selecting_the_magnifier_and_window_highlight_show() {
    let o = overlay(640, 360, 1.0);
    o.session().borrow_mut().pointer_move(p(100.0, 100.0), NO);
    o.refresh();
    let buf = render(&o, 640, 360);
    save("1-hover", &buf);
    assert!(o.window().get_has_magnifier());
    assert!(o.window().get_has_hover());
    // the veil darkens the area outside the window under the cursor
    let px = |x: usize, y: usize| buf.as_slice()[y * 640 + x];
    assert!(px(500, 300).g < 140, "{:?}", px(500, 300));
}

#[test]
fn a_settled_zone_shows_handles_and_the_toolbar() {
    let o = overlay(900, 500, 1.0);
    {
        let s = o.session();
        let mut s = s.borrow_mut();
        s.pointer_down(p(100.0, 80.0), NO);
        s.pointer_move(p(400.0, 250.0), NO);
        s.pointer_up(p(400.0, 250.0), NO);
        s.choose_tool(Some(Tool::Rect));
        s.set_color(Color::PALETTE[4]);
        s.set_filled(false);
        s.pointer_down(p(150.0, 120.0), NO);
        s.pointer_move(p(300.0, 200.0), NO);
        s.pointer_up(p(300.0, 200.0), NO);
        s.choose_tool(Some(Tool::Arrow));
        s.set_color(Color::PALETTE[0]);
        s.pointer_down(p(120.0, 230.0), NO);
        s.pointer_move(p(280.0, 150.0), NO);
        s.pointer_up(p(280.0, 150.0), NO);
        s.choose_tool(Some(Tool::Marker));
        s.pointer_down(p(350.0, 120.0), NO);
        s.pointer_up(p(350.0, 120.0), NO);
        s.choose_tool(Some(Tool::Rect));
    }
    o.refresh();
    let buf = render(&o, 900, 500);
    save("2-toolbar", &buf);
    assert!(o.window().get_has_toolbar());
    assert!(o.window().get_has_annotated());
    assert_eq!(o.window().get_tool(), 4);
}

#[test]
fn the_toolbar_scales_with_the_ui_scale() {
    let o = overlay(1400, 800, 2.0);
    {
        let s = o.session();
        let mut s = s.borrow_mut();
        s.pointer_down(p(100.0, 80.0), NO);
        s.pointer_up(p(500.0, 300.0), NO);
    }
    o.refresh();
    let buf = render(&o, 1400, 800);
    save("3-toolbar-2x", &buf);
    assert!(o.window().get_toolbar_y() > 300.0);
}

#[test]
fn key_mapping() {
    use vixeeny_editor::Key;
    let m = |t: &str, c, s| overlay::map_key(t, c, s).map(|k| k.key);
    assert_eq!(m("\u{1b}", false, false), Some(Key::Escape));
    assert_eq!(m("c", true, false), Some(Key::Char('c')));
    assert_eq!(m("\u{3}", true, false), Some(Key::Char('c'))); // Ctrl+C as a control character
    assert_eq!(m("\u{1b}", true, false), Some(Key::Escape));
    assert_eq!(m("", false, false), None);
    assert_eq!(overlay::tool_index(Some(Tool::Pen)), 1);
    assert_eq!(overlay::tool_index(None), 12);
}
