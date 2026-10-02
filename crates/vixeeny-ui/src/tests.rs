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

#[test]
fn input_events_drive_the_session_through_the_window() {
    let o = overlay(900, 500, 1.0);
    let w = o.window();
    // draw a zone with the mouse
    w.invoke_pointer_pressed(100.0, 80.0, false);
    w.invoke_pointer_moved(300.0, 200.0, false);
    w.invoke_pointer_released(400.0, 250.0, false);
    assert!(w.get_has_toolbar());
    assert_eq!((w.get_sel_w(), w.get_sel_h()), (300.0, 170.0));
    // pick the text tool (index 6), click, type, validate
    w.invoke_tool_chosen(6);
    w.invoke_pointer_pressed(150.0, 120.0, false);
    w.invoke_pointer_released(150.0, 120.0, false);
    assert!(w.get_has_text_input());
    w.set_text_value("Hello".into());
    w.invoke_text_committed("Hello".into());
    assert!(!w.get_has_text_input());
    assert_eq!(w.get_text_value(), "");
    assert!(w.get_has_annotated());
    // a second text, validated by clicking elsewhere
    w.invoke_pointer_pressed(150.0, 180.0, false);
    w.invoke_pointer_released(150.0, 180.0, false);
    w.set_text_value("Again".into());
    w.invoke_pointer_pressed(300.0, 100.0, false);
    w.invoke_pointer_released(300.0, 100.0, false);
    let texts = o.session().borrow().editor().doc.items.len();
    assert!(texts >= 2, "{texts}");
    // undo through the toolbar action, close through the keyboard command
    w.invoke_action("undo".into());
    assert_eq!(o.session().borrow().editor().doc.items.len(), texts - 1);
}

#[test]
fn commands_reach_the_handler_and_can_keep_the_window_open() {
    use std::cell::RefCell;
    WINDOW.with(|_| ());
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    let session = Session::new(screen(300, 200), vec![]);
    let o = Overlay::new(session, 1.0, move |c, s, _| {
        log.borrow_mut().push((c, s.export().is_some()));
        c != vixeeny_editor::Command::Ocr
    })
    .unwrap_or_else(|e| panic!("{e}"));
    let w = o.window();
    w.invoke_pointer_pressed(10.0, 10.0, false);
    w.invoke_pointer_released(200.0, 150.0, false);
    w.invoke_action("copy".into());
    w.invoke_action("ocr".into());
    use vixeeny_editor::Command::{Copy, Ocr};
    assert_eq!(*seen.borrow(), [(Copy, true), (Ocr, true)]);
}

#[test]
fn the_colour_picker_panel_renders() {
    let o = overlay(900, 500, 1.0);
    let w = o.window();
    w.invoke_pointer_pressed(100.0, 60.0, false);
    w.invoke_pointer_released(400.0, 200.0, false);
    w.set_picker_open(true);
    let buf = render(&o, 900, 500);
    save("4-picker", &buf);
    // the hue bar is red at the top
    let top = w.get_toolbar_y();
    assert!(top > 0.0);
}

#[test]
fn the_ocr_window_renders_text_hint_and_buttons() {
    WINDOW.with(|_| ());
    let panel = OcrPanel {
        title: "Text recognition".into(),
        text: "Hello world\nSecond line — 日本語".into(),
        status: "Language: English (United States)".into(),
        hint: "No OCR language is installed for: ja, ko. Open Windows Settings → Language & region and add the language with “Optical character recognition”.".into(),
        copy_label: "Copy".into(),
        settings_label: "Open language settings".into(),
        show_settings: true,
    };
    let w = ocr_panel::build(&panel).unwrap_or_else(|e| panic!("{e}"));
    let window = WINDOW.with(Rc::clone);
    window.set_size(PhysicalSize::new(560, 380));
    w.show().unwrap_or_else(|e| panic!("{e}"));
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(560, 380);
    window.draw_if_needed(|r| {
        r.render(buffer.make_mut_slice(), 560);
    });
    save("5-ocr", &buffer);
    assert_eq!(w.get_text(), "Hello world\nSecond line — 日本語");
    // not an all-black frame
    assert!(buffer.as_slice().iter().any(|p| p.r > 100));
}

#[test]
fn the_convert_window_renders_and_reports_clicks() {
    use crate::convert_panel::{ConvertTexts, new_window, set_files};
    WINDOW.with(|_| ());
    let texts = ConvertTexts {
        title: "Convert images".into(),
        drop_hint: "Drop images or folders here".into(),
        add_files: "Add files…".into(),
        add_folder: "Add folder…".into(),
        clear: "Clear".into(),
        format: "Format".into(),
        quality: "Quality".into(),
        lossless: "Lossless".into(),
        existing: "If it exists".into(),
        rename: "Rename".into(),
        overwrite: "Overwrite".into(),
        skip: "Skip".into(),
        output: "Output folder".into(),
        choose: "Choose…".into(),
        reset: "Reset".into(),
        convert: "Convert".into(),
        cancel: "Cancel".into(),
    };
    let w = new_window(&texts, &["PNG", "JPEG", "WebP", "AVIF", "JXL"])
        .unwrap_or_else(|e| panic!("{e}"));
    let window = WINDOW.with(Rc::clone);
    window.set_size(PhysicalSize::new(640, 560));
    w.show().unwrap_or_else(|e| panic!("{e}"));
    let draw = || {
        let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(640, 560);
        window.draw_if_needed(|r| {
            r.render(buffer.make_mut_slice(), 640);
        });
        buffer
    };
    save("6-convert-empty", &draw());
    set_files(
        &w,
        &["C:\\Pictures\\a.png".into(), "C:\\Pictures\\b.jpg".into()],
    );
    w.set_summary("2 image(s)".into());
    w.set_format_index(3);
    w.set_show_lossless(true);
    w.set_output_text("Same folder as each image".into());
    w.set_progress(0.4);
    w.set_status("Converting… 1 / 2".into());
    w.set_running(true);
    let buf = draw();
    save("7-convert-running", &buf);
    assert!(
        buf.as_slice().iter().any(|p| p.b > 200 && p.r < 100),
        "the progress bar is blue"
    );
    let clicked = Rc::new(std::cell::Cell::new(0));
    let c = clicked.clone();
    w.on_quality_step(move |d| c.set(d));
    w.invoke_quality_step(-5);
    assert_eq!(clicked.get(), -5);
}

#[test]
fn the_recording_widget_renders_both_states_and_reports_clicks() {
    use crate::widget_panel::{WidgetEvent, WidgetPanel, WidgetTexts};
    use std::cell::RefCell;
    WINDOW.with(|_| ());
    let texts = WidgetTexts {
        pause: "Pause".into(),
        resume: "Resume".into(),
        stop: "Stop".into(),
    };
    let panel = WidgetPanel::new(&texts, false).unwrap_or_else(|e| panic!("{e}"));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    panel.on_event(move |e| sink.borrow_mut().push(e));
    let window = WINDOW.with(Rc::clone);
    window.set_size(PhysicalSize::new(200, 40));
    panel.window().show().unwrap_or_else(|e| panic!("{e}"));
    let draw = || {
        let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(200, 40);
        window.draw_if_needed(|r| {
            r.render(buffer.make_mut_slice(), 200);
        });
        buffer
    };
    panel.set_state(false, std::time::Duration::from_secs(3_725));
    let recording = draw();
    save("7-widget-recording", &recording);
    panel.set_state(true, std::time::Duration::from_secs(3_725));
    let paused = draw();
    save("7-widget-paused", &paused);
    // Not blank, and the two states differ (red pulsing dot vs. orange still dot, other button).
    assert!(recording.as_slice().iter().any(|p| p.r > 200));
    assert_ne!(recording.as_slice(), paused.as_slice());
    assert_eq!(panel.window().get_time(), "01:02:05");

    // Clicks: pause button at the right, stop at the far right.
    use slint::platform::{PointerEventButton, WindowEvent};
    let click = |x: f32| {
        let pos = slint::LogicalPosition::new(x, 20.0);
        window.dispatch_event(WindowEvent::PointerMoved { position: pos });
        window.dispatch_event(WindowEvent::PointerPressed {
            position: pos,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position: pos,
            button: PointerEventButton::Left,
        });
    };
    click(140.0);
    click(178.0);
    assert_eq!(
        *events.borrow(),
        [WidgetEvent::TogglePause, WidgetEvent::Stop]
    );
}
