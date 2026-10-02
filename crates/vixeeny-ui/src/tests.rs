//! Headless rendering with Slint's software renderer, to check layouts without a display.

use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel, SharedPixelBuffer};

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

fn new_ui() -> EditorWindow {
    WINDOW.with(|_| ()); // installs the platform before any component exists
    EditorWindow::new().unwrap_or_else(|e| panic!("{e}"))
}

fn render(ui: &EditorWindow, w: u32, h: u32) -> SharedPixelBuffer<Rgb8Pixel> {
    let window = WINDOW.with(Rc::clone);
    window.set_size(PhysicalSize::new(w, h));
    ui.show().unwrap_or_else(|e| panic!("{e}"));
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    window.draw_if_needed(|renderer| {
        renderer.render(buffer.make_mut_slice(), w as usize);
    });
    buffer
}

#[test]
fn the_overlay_draws_the_selection_border() {
    let ui = new_ui();
    ui.set_image_width(200);
    ui.set_image_height(100);
    ui.set_has_selection(true);
    ui.set_sel_x(50.0);
    ui.set_sel_y(20.0);
    ui.set_sel_w(100.0);
    ui.set_sel_h(60.0);
    let buf = render(&ui, 200, 100);
    let px = |x: usize, y: usize| buf.as_slice()[y * 200 + x];
    // border on the selection edge, dimmed black elsewhere
    assert!(px(50, 40).b > 150, "{:?}", px(50, 40));
    assert_eq!(px(5, 5).r, 0);
}
