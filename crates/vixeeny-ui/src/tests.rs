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
    let o = Overlay::new(session, scale, |_, _, _| true).unwrap_or_else(|e| panic!("{e}"));
    // Screenshots show the settled look, not the first frame of the fade.
    o.window().set_fade_in(false);
    o
}

const NO: Modifiers = Modifiers { shift: false };

fn p(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

#[test]
fn before_selecting_only_the_window_under_the_cursor_stays_clear() {
    let o = overlay(640, 360, 1.0);
    o.session().borrow_mut().pointer_move(p(100.0, 100.0), NO);
    o.refresh();
    let buf = render(&o, 640, 360);
    save("1-hover", &buf);
    assert!(
        !o.window().get_has_magnifier(),
        "no magnifier before drawing"
    );
    assert!(o.window().get_has_hover());
    let px = |x: usize, y: usize| buf.as_slice()[y * 640 + x];
    let original = screen(640, 360);
    let at = |x: usize, y: usize| original.data[(y * 640 + x) * 4 + 1];
    // the veil darkens outside the window under the cursor, not inside it
    assert!(px(500, 300).g < at(500, 300), "{:?}", px(500, 300));
    assert_eq!(px(150, 120).g, at(150, 120), "{:?}", px(150, 120));
}

#[test]
fn the_first_frame_is_the_screen_as_it_was() {
    WINDOW.with(|_| ());
    let session = Session::new(screen(320, 200), Vec::new());
    let o = Overlay::new(session, 1.0, |_, _, _| true).unwrap_or_else(|e| panic!("{e}"));
    let buf = render(&o, 320, 200);
    let original = screen(320, 200);
    let (x, y) = (200, 150);
    let o_px = &original.data[(y * 320 + x) * 4..][..3];
    let px = buf.as_slice()[y * 320 + x];
    assert_eq!([px.r, px.g, px.b], o_px, "no veil before the fade");
}

#[test]
fn a_window_shows_its_own_part_of_the_desktop() {
    WINDOW.with(|_| ());
    // The right half of a 640×360 desktop, as the second of two monitors.
    let session = Session::new(screen(640, 360), Vec::new());
    let right = Screen {
        position: (320, 0),
        size: (320, 360),
        area: vixeeny_editor::Rect::new(320.0, 0.0, 320.0, 360.0),
    };
    let o = Overlay::on_screens(session, 1.0, &[right], |_, _, _| true)
        .unwrap_or_else(|e| panic!("{e}"));
    o.window().set_fade_in(false);
    o.session().borrow_mut().dim = 0.0;
    o.refresh();
    let buf = render(&o, 320, 360);
    save("0-right-monitor", &buf);
    let original = screen(640, 360);
    for (x, y) in [(0, 0), (100, 200), (319, 359)] {
        let o_px = &original.data[(y * 640 + 320 + x) * 4..][..3];
        let px = buf.as_slice()[y * 320 + x];
        assert_eq!([px.r, px.g, px.b], o_px, "at {x},{y}");
    }
}

#[test]
fn a_zone_on_one_monitor_leaves_the_other_window_unchanged() {
    WINDOW.with(|_| ());
    // Two monitors side by side; the zone is drawn on the left one only.
    let session = Session::new(screen(640, 360), Vec::new());
    let right = Screen {
        position: (320, 0),
        size: (320, 360),
        area: vixeeny_editor::Rect::new(320.0, 0.0, 320.0, 360.0),
    };
    let o = Overlay::on_screens(session, 1.0, &[right], |_, _, _| true)
        .unwrap_or_else(|e| panic!("{e}"));
    let w = o.window();
    let mods = vixeeny_editor::Modifiers::default();
    let s = o.session();
    s.borrow_mut()
        .pointer_down(vixeeny_editor::Point::new(20.0, 20.0), mods);
    s.borrow_mut()
        .pointer_move(vixeeny_editor::Point::new(80.0, 70.0), mods);
    o.refresh();
    let parked = (w.get_sel_x(), w.get_sel_y(), w.get_size_x());
    // The zone grows, still far from the right monitor: nothing changes there.
    s.borrow_mut()
        .pointer_move(vixeeny_editor::Point::new(150.0, 120.0), mods);
    o.refresh();
    assert!(w.get_has_selection());
    assert_eq!((w.get_sel_x(), w.get_sel_y(), w.get_size_x()), parked);
    assert!(!w.get_has_magnifier());
    // It reaches the right monitor: the window shows it.
    s.borrow_mut()
        .pointer_move(vixeeny_editor::Point::new(400.0, 120.0), mods);
    o.refresh();
    assert_eq!(w.get_sel_x(), 20.0);
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
    w.set_style_open(true);
    let buf = render(&o, 900, 500);
    save("4-style", &buf);
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
    window.set_size(PhysicalSize::new(216, 60));
    panel.window().set_rise_in(false);
    panel.window().show().unwrap_or_else(|e| panic!("{e}"));
    let draw = || {
        let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(216, 60);
        window.draw_if_needed(|r| {
            r.render(buffer.make_mut_slice(), 216);
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

    // Clicks: the bar is 10 px inside the window; stop at its right end, pause just before.
    use slint::platform::{PointerEventButton, WindowEvent};
    let click = |x: f32| {
        let pos = slint::LogicalPosition::new(x, 30.0);
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
    click(150.0);
    click(186.0);
    assert_eq!(
        *events.borrow(),
        [WidgetEvent::TogglePause, WidgetEvent::Stop]
    );
}

fn side_texts() -> side_panel::SideTexts {
    side_panel::SideTexts {
        image: "Screenshot".into(),
        region: "Region".into(),
        window: "Window".into(),
        screen: "Screen".into(),
        all_monitors: "All screens".into(),
        scrolling: "Scrolling capture".into(),
        ocr: "Copy text (OCR)".into(),
        video: "Video".into(),
        record: "Record".into(),
        stop_recording: "Stop recording".into(),
        replay_start: "Start replay buffer".into(),
        replay_stop: "Stop replay buffer".into(),
        replay_save: "Save replay".into(),
        profile: "Profile: {name}".into(),
        settings: "Settings".into(),
        pin: "Keep open".into(),
        close: "Close".into(),
    }
}

fn side_state(edge: side_panel::Edge, dark: bool) -> side_panel::SideState {
    side_panel::SideState {
        recording: false,
        replay: true,
        profiles: vec!["Jeu 4K HDR".into(), "Tuto 1080p".into()],
        profile: 0,
        dark,
        edge,
        animate: false,
        backdrop: false,
        pinned: false,
    }
}

/// Opens the strip on a 2560×1440 monitor at 100 % and renders it.
fn side_render(
    panel: &side_panel::SidePanel,
    edge: side_panel::Edge,
) -> (SharedPixelBuffer<Rgb8Pixel>, Rc<MinimalSoftwareWindow>) {
    let (_, _, w, h) = side_panel::panel_geometry(edge, (0, 0, 2560, 1440), 96, panel.entries());
    let window = WINDOW.with(Rc::clone);
    window.set_size(PhysicalSize::new(w, h));
    panel.window().show().unwrap_or_else(|e| panic!("{e}"));
    panel.window().set_shown(true);
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    window.draw_if_needed(|r| {
        r.render(buffer.make_mut_slice(), w as usize);
    });
    (buffer, window)
}

#[test]
fn the_side_strip_renders_in_both_themes_and_orientations() {
    use side_panel::{Edge, SidePanel};
    WINDOW.with(|_| ());
    let dark = SidePanel::new(&side_texts(), &side_state(Edge::Right, true))
        .unwrap_or_else(|e| panic!("{e}"));
    let (dark_img, _) = side_render(&dark, Edge::Right);
    save("8-side-dark-right", &dark_img);
    let light = SidePanel::new(&side_texts(), &side_state(Edge::Bottom, false))
        .unwrap_or_else(|e| panic!("{e}"));
    let (light_img, _) = side_render(&light, Edge::Bottom);
    save("8-side-light-bottom", &light_img);
    // Drawn, dark strip dark, light strip light: a point of the strip between two icons (the
    // rest of the window is the transparent room for the labels).
    let at = |b: &SharedPixelBuffer<Rgb8Pixel>, x: u32, y: u32| {
        b.as_slice()[(y * b.width() + x) as usize].r
    };
    let (w, _) = (dark_img.width(), dark_img.height());
    assert!(
        at(&dark_img, w - 12 - 4, 200) < 80,
        "{}",
        at(&dark_img, w - 12 - 4, 200)
    );
    let h = light_img.height();
    assert!(
        at(&light_img, 300, h - 12 - 3) > 200,
        "{}",
        at(&light_img, 300, h - 12 - 3)
    );
}

#[test]
fn the_side_strip_answers_to_the_keyboard_and_the_mouse() {
    use side_panel::{Choice, Edge, SidePanel};
    use slint::platform::{Key, PointerEventButton, WindowEvent};
    WINDOW.with(|_| ());

    // Down, Down, Enter: the third entry (Screen), not the titles.
    let panel = SidePanel::new(&side_texts(), &side_state(Edge::Right, true))
        .unwrap_or_else(|e| panic!("{e}"));
    let (_, window) = side_render(&panel, Edge::Right);
    let press = |key: Key| {
        let text: slint::SharedString = key.into();
        window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window.dispatch_event(WindowEvent::KeyReleased { text });
    };
    assert_eq!(
        panel.entries()[panel.window().get_selected() as usize].id,
        Choice::Region as i32
    );
    press(Key::DownArrow);
    press(Key::DownArrow);
    press(Key::Return);
    assert_eq!(panel.chosen(), Some(Choice::Screen));

    // Escape picks nothing.
    let panel = SidePanel::new(&side_texts(), &side_state(Edge::Right, true))
        .unwrap_or_else(|e| panic!("{e}"));
    let (_, window) = side_render(&panel, Edge::Right);
    let text: slint::SharedString = Key::Escape.into();
    window.dispatch_event(WindowEvent::KeyPressed { text });
    assert_eq!(panel.chosen(), None);
    assert!(!panel.window().get_shown(), "Escape starts the exit");

    // A click on the "Record" button picks it; a click on a separator does nothing. The strip:
    // 12 px from the window's edges, 6 px padding, the pin and close row (28), a separator (9),
    // then the entries (separators of 9, buttons of 40), 2 px apart.
    let panel = SidePanel::new(&side_texts(), &side_state(Edge::Right, true))
        .unwrap_or_else(|e| panic!("{e}"));
    let (_, window) = side_render(&panel, Edge::Right);
    let click = |y: f32| {
        let position = slint::LogicalPosition::new(296.0 - 12.0 - 26.0, y);
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    };
    let first = 12.0 + 6.0 + 28.0 + 2.0 + 9.0 + 2.0;
    click(first + 4.0); // the separator before the screenshots
    assert_eq!(panel.chosen(), None);
    // separator, 6 buttons, separator, each followed by a gap → the record button starts here
    let record_y = first + 9.0 + 2.0 + 6.0 * 42.0 + 9.0 + 2.0 + 20.0;
    click(record_y);
    assert_eq!(panel.chosen(), Some(Choice::RecordToggle));
}

#[test]
fn cycling_the_profile_keeps_the_strip_open_and_reports_it() {
    use side_panel::{Edge, SidePanel};
    use slint::Model;
    use std::cell::RefCell;
    WINDOW.with(|_| ());
    let panel = SidePanel::new(&side_texts(), &side_state(Edge::Right, true))
        .unwrap_or_else(|e| panic!("{e}"));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    panel.on_profile(move |i| sink.borrow_mut().push(i));
    side_render(&panel, Edge::Right);
    panel.window().invoke_activate(100);
    panel.window().invoke_activate(100);
    assert_eq!(*seen.borrow(), [1, 0]);
    assert_eq!(panel.chosen(), None);
    assert!(panel.window().get_shown());
    let labels: Vec<_> = panel
        .window()
        .get_items()
        .iter()
        .map(|i| i.label.to_string())
        .collect();
    assert!(
        labels.contains(&"Profile: Jeu 4K HDR".to_owned()),
        "{labels:?}"
    );
}

fn settings_panel() -> (
    settings_panel::SettingsPanel,
    Rc<std::cell::RefCell<Vec<vixeeny_common::config::Config>>>,
) {
    use std::cell::RefCell;
    WINDOW.with(|_| ());
    let panel = settings_panel::SettingsPanel::new(
        vixeeny_common::config::Config::default(),
        Some("en-US".into()),
        "1.2.3",
        crate::theme::Look::dark(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    panel.on_change(move |c| sink.borrow_mut().push(c.clone()));
    (panel, seen)
}

fn settings_render(
    panel: &settings_panel::SettingsPanel,
    name: &str,
) -> SharedPixelBuffer<Rgb8Pixel> {
    settings_render_at(panel, name, (1040, 720))
}

fn settings_render_at(
    panel: &settings_panel::SettingsPanel,
    name: &str,
    (w, h): (u32, u32),
) -> SharedPixelBuffer<Rgb8Pixel> {
    let window = WINDOW.with(Rc::clone);
    window.set_size(PhysicalSize::new(w, h));
    panel.window().show().unwrap_or_else(|e| panic!("{e}"));
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    // Twice: wrapped texts settle their height on the second layout.
    for _ in 0..2 {
        window.request_redraw();
        window.draw_if_needed(|r| {
            r.render(buffer.make_mut_slice(), w as usize);
        });
    }
    save(name, &buffer);
    buffer
}

#[test]
fn settings_apply_immediately_and_each_section_resets() {
    use vixeeny_settings::Section;
    let (panel, seen) = settings_panel();
    panel.select_section(Section::General);
    settings_render(&panel, "9-settings-general");
    let w = panel.window();
    let groups = w.get_groups();
    w.invoke_row_toggled("sounds".into(), false);
    // The page keeps its rows (and their controls, which then animate): only the row changed.
    assert!(w.get_groups() == groups, "the rows were rebuilt");
    w.invoke_row_chosen("theme".into(), 2); // system, light, dark
    w.invoke_row_number("idle_exit".into(), 62);
    assert_eq!(seen.borrow().len(), 3, "one notification per change");
    let c = panel.config();
    assert!(!c.general.sounds);
    assert_eq!(c.general.theme, "dark");
    assert_eq!(c.general.app_idle_exit_seconds, 60);
    // The same value again is not a change.
    w.invoke_row_toggled("sounds".into(), false);
    assert_eq!(seen.borrow().len(), 3);

    // Another section's change survives this section's reset.
    panel.select_section(Section::Capture);
    w.invoke_row_chosen("image_format".into(), 1);
    panel.select_section(Section::General);
    w.invoke_reset();
    let c = panel.config();
    assert!(c.general.sounds && c.general.theme == "system");
    assert_eq!(c.image.format, "jpeg");
    // Unknown ids and out-of-range choices are ignored.
    let before = seen.borrow().len();
    w.invoke_row_chosen("theme".into(), 99);
    w.invoke_row_toggled("nope".into(), true);
    assert_eq!(seen.borrow().len(), before);
}

#[test]
fn changing_the_language_relabels_the_window_at_once() {
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.select_section(Section::General);
    use slint::Model;
    let titles = |p: &settings_panel::SettingsPanel| {
        p.window()
            .get_section_titles()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(titles(&panel)[1], "General");
    panel.window().invoke_row_chosen("language".into(), 1); // auto, fr, en
    assert_eq!(panel.config().general.language, "fr");
    assert_eq!(titles(&panel)[1], "Général");
    let first = panel.window().get_groups().row_data(0).unwrap();
    assert_eq!(first.title, "Application");
    assert_eq!(first.rows.row_data(0).unwrap().label, "Langue");
}

#[test]
fn the_video_page_edits_the_current_profile_and_reports_problems() {
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.select_section(Section::Video);
    settings_render(&panel, "9-settings-video");
    let w = panel.window();
    w.invoke_row_chosen("fps".into(), 1); // 24, 30, …
    assert_eq!(panel.config().profiles["default"].fps, 30);
    w.invoke_row_chosen("split".into(), 1);
    assert_eq!(panel.config().profiles["default"].split.mode, "size:2048");
    assert_eq!(w.get_notice(), "");
    // MKV is not an MP4: a variable frame rate in MP4 is a problem the page says out loud.
    w.invoke_row_toggled("vfr".into(), true);
    assert!(!w.get_notice().is_empty(), "{}", w.get_notice());
}

#[test]
fn shortcuts_are_edited_in_place_and_conflicts_are_explained() {
    use slint::Model;
    use vixeeny_settings::Section;
    let (panel, seen) = settings_panel();
    panel.select_section(Section::Shortcuts);
    settings_render(&panel, "9-settings-shortcuts");
    let w = panel.window();
    // Row 7 is the replay toggle, row 8 the replay save (Ctrl+Shift+S). Keys are recorded, never typed.
    let record = |row: i32, slot: i32, keys: &str| {
        w.invoke_shortcut_record(row, slot);
        panel.captured(settings_panel::Captured::Combination(keys.into()));
    };
    record(7, 0, "Ctrl+Shift+F9");
    assert_eq!(panel.config().hotkeys.replay_toggle, ["Ctrl+Shift+F9"]);
    assert_eq!(seen.borrow().len(), 1);
    record(7, 1, "Ctrl+Shift+S");
    assert_eq!(panel.config().hotkeys.replay_toggle.len(), 1, "refused");
    let row = w.get_shortcuts().row_data(7).unwrap();
    assert!(row.error.contains("Save the replay"), "{}", row.error);
    assert_eq!(seen.borrow().len(), 1, "a refusal is not a change");
    // Escape stops the recording without touching anything.
    w.invoke_shortcut_record(7, 1);
    assert!(panel.captured(settings_panel::Captured::Cancel));
    assert!(
        !panel.captured(settings_panel::Captured::Cancel),
        "nothing is recording any more"
    );
    assert_eq!(seen.borrow().len(), 1);
}

#[test]
fn every_page_of_the_settings_renders() {
    use vixeeny_settings::{AudioDevices, AudioEntry, Section};
    let (panel, _) = settings_panel();
    let entry = |id: &str, name: &str| AudioEntry {
        id: id.into(),
        name: name.into(),
    };
    panel.set_audio(AudioDevices {
        outputs: vec![
            entry("o1", "Speakers (Realtek Audio)"),
            entry("o2", "Headset (Arctis 7)"),
        ],
        inputs: vec![entry("i1", "Microphone (Blue Yeti)")],
        programs: vec![
            entry("Spotify.exe", "Spotify.exe"),
            entry("chrome.exe", "chrome.exe"),
        ],
    });
    for section in Section::ALL {
        if matches!(section, Section::Gallery) {
            continue;
        }
        panel.select_section(section);
        let name = format!("9-page-{section:?}").to_lowercase();
        settings_render(&panel, &name);
    }
    // Narrow: the navigation keeps its icons only, the controls go under their text.
    panel.select_section(Section::General);
    settings_render_at(&panel, "9-page-narrow", (660, 720));
}

#[test]
fn profiles_are_created_renamed_chosen_and_deleted_from_their_row() {
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.select_section(Section::Video);
    let w = panel.window();
    let action = |a: &str, arg: &str| w.invoke_row_action("profile".into(), a.into(), arg.into());
    action("profile-new", "");
    assert_eq!(panel.config().video.profile, "Profile");
    action("profile-duplicate", "");
    assert_eq!(panel.config().video.profile, "Profile (copy)");
    assert_eq!(panel.config().profiles.len(), 3);
    action("profile-rename", "Jeu 4K");
    assert!(panel.config().profiles.contains_key("Jeu 4K"));
    assert_eq!(panel.config().video.profile, "Jeu 4K");
    // Listed by name: Jeu 4K, Profile, default.
    w.invoke_row_chosen("profile".into(), 1);
    assert_eq!(panel.config().video.profile, "Profile");
    action("profile-rename", "Jeu 4K");
    assert!(!w.get_notice().is_empty(), "the name is taken");
    action("profile-delete", "");
    action("profile-delete", "");
    assert_eq!(panel.config().profiles.len(), 1);
    action("profile-delete", "");
    assert_eq!(panel.config().profiles.len(), 1, "the last one stays");
    settings_render(&panel, "9-settings-profile");
}

#[test]
fn the_gallery_and_the_other_pages_show_what_the_host_gives_them() {
    use settings_panel::{GalleryEntry, GalleryRequest, Line};
    use std::cell::RefCell;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    let requests = Rc::new(RefCell::new(Vec::new()));
    let sink = requests.clone();
    panel.on_gallery(move |r| sink.borrow_mut().push(r));
    panel.select_section(Section::Gallery);
    let tile = |name: &str, video: bool| GalleryEntry {
        name: name.into(),
        detail: "2026-10-02 14:03 · 2.4 MB".into(),
        video,
        thumb: (!video).then(|| (4, 4, [200, 80, 40, 255].repeat(16))),
    };
    panel.set_gallery(
        vec![
            tile("Minecraft_2026-10-02.png", false),
            tile("Replay.mp4", true),
            tile("a.png", false),
        ],
        Some(1),
    );
    settings_render(&panel, "9-settings-gallery");
    let w = panel.window();
    w.invoke_gallery_select(2);
    w.invoke_gallery_open(2);
    w.invoke_gallery_action("open".into());
    w.invoke_gallery_filter(1, "mine".into());
    assert_eq!(
        *requests.borrow(),
        [
            GalleryRequest::Select(2),
            GalleryRequest::Open(2),
            GalleryRequest::Action("open".into()),
            GalleryRequest::Filter(1, "mine".into()),
        ]
    );
    panel.select_section(Section::About);
    panel.set_lines(vec![
        Line {
            text: "Licence GPL-3.0-or-later".into(),
            detail: "https://github.com/Xantoom/Vixeeny".into(),
            strong: false,
        },
        Line {
            text: "Logs".into(),
            detail: "C:\\Users\\me\\AppData\\Local\\Vixeeny\\logs".into(),
            strong: false,
        },
    ]);
    panel.set_update(&settings_panel::UpdateView {
        stage: settings_panel::UpdateStage::Downloading,
        title: "Downloading version 1.3.0…".into(),
        detail: "12.4 MB of 31.0 MB".into(),
        progress: 0.4,
        action: String::new(),
    });
    settings_render(&panel, "9-settings-about");
    panel.set_update(&settings_panel::UpdateView {
        stage: settings_panel::UpdateStage::Ready,
        title: "Version 1.3.0 is ready".into(),
        detail: "Restart Vixeeny to finish. It takes a second.".into(),
        progress: 1.0,
        action: "Restart".into(),
    });
    settings_render(&panel, "9-settings-about-ready");
}

#[test]
fn the_toast_renders_with_a_thumbnail_and_an_error_variant() {
    use crate::toast_panel::{ToastContent, ToastPanel};
    WINDOW.with(|_| ());
    let window = WINDOW.with(Rc::clone);
    window.set_size(PhysicalSize::new(380, 130));
    let draw = |content: &ToastContent| {
        let panel = ToastPanel::new(content).unwrap_or_else(|e| panic!("{e}"));
        panel.window().show().unwrap_or_else(|e| panic!("{e}"));
        let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(380, 130);
        window.draw_if_needed(|r| {
            r.render(buffer.make_mut_slice(), 380);
        });
        (panel, buffer)
    };
    let green = vec![[40u8, 200, 90, 255]; 64 * 48].concat();
    let saved = ToastContent {
        heading: "Image saved".into(),
        body: "capture-001.png".into(),
        thumb: Some((64, 48, green)),
        dark: true,
        action_label: "Open folder".into(),
        ..ToastContent::default()
    };
    let (panel, ok) = draw(&saved);
    save("8-toast-saved", &ok);
    assert!(panel.window().get_has_thumb());
    assert!(ok.as_slice().iter().any(|p| p.g > 180 && p.r < 80));
    let failed = ToastContent {
        heading: "The capture failed".into(),
        body: "The disk is full".into(),
        error: true,
        dark: false,
        action_label: "Open settings".into(),
        ..ToastContent::default()
    };
    let (_, bad) = draw(&failed);
    save("8-toast-error", &bad);
    assert_ne!(ok.as_slice(), bad.as_slice());
}

#[test]
fn the_wizard_walks_its_steps_and_relabels_in_the_chosen_language() {
    use crate::wizard_panel::{STEPS, WizardPanel};
    WINDOW.with(|_| ());
    let window = WINDOW.with(Rc::clone);
    window.set_size(PhysicalSize::new(520, 380));
    let browse = |_: &str| Some(String::from(r"D:\Captures"));
    let panel = WizardPanel::new(
        vixeeny_common::config::Config::default(),
        Some("en-US".into()),
        crate::theme::Look::dark(),
        Box::new(browse),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let w = panel.window();
    w.show().unwrap_or_else(|e| panic!("{e}"));
    let draw = |name: &str| {
        let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(520, 380);
        window.draw_if_needed(|r| {
            r.render(buffer.make_mut_slice(), 520);
        });
        save(name, &buffer);
    };
    assert_eq!(w.get_heading(), "Welcome to Vixeeny");
    assert_eq!(w.get_step_text(), "Step 1 of 4");
    draw("9-wizard-language");
    w.invoke_language_chosen(1);
    assert_eq!(w.get_heading(), "Bienvenue dans Vixeeny");
    assert_eq!(panel.choices().general.language, "fr");
    w.invoke_next();
    assert_eq!(w.get_step(), 1);
    w.invoke_browse(0);
    assert_eq!(panel.choices().paths.images, r"D:\Captures");
    assert_eq!(w.get_images_path(), r"D:\Captures");
    draw("9-wizard-folders");
    w.invoke_next();
    w.invoke_autostart_toggled(false);
    draw("9-wizard-startup");
    w.invoke_next();
    assert!(w.get_last());
    assert_eq!(w.get_finish_label(), "Terminer");
    draw("9-wizard-hardware");
    assert!(!panel.is_finished());
    w.invoke_back();
    assert_eq!(w.get_step() as usize, STEPS - 2);
    w.invoke_next();
    w.invoke_next();
    assert!(panel.is_finished());
    assert!(!panel.choices().general.autostart);
}

#[test]
fn a_long_image_scrolls_under_a_fixed_toolbar() {
    WINDOW.with(|_| ());
    let mut session = Session::new(screen(400, 1200), vec![]);
    session.select_all();
    let o = Overlay::new(session, 1.0, |_, _, _| true).unwrap_or_else(|e| panic!("{e}"));
    o.set_scrolling(true);
    let px = |buf: &SharedPixelBuffer<Rgb8Pixel>, x: usize, y: usize| buf.as_slice()[y * 640 + x];

    let top = render(&o, 640, 360);
    save("5-long-top", &top);
    assert!(o.window().get_has_toolbar());
    // centred horizontally, the toolbar pinned at the top whatever the image height
    assert!(
        o.window().get_toolbar_y() < 40.0,
        "{}",
        o.window().get_toolbar_y()
    );
    assert!(px(&top, 130, 300).g < 70, "{:?}", px(&top, 130, 300));

    o.window().set_scroll_y(800.0);
    let scrolled = render(&o, 640, 360);
    save("5-long-scrolled", &scrolled);
    // the same screen row now shows a lower part of the image
    assert!(
        px(&scrolled, 130, 300).g > 180,
        "{:?}",
        px(&scrolled, 130, 300)
    );
    assert!(o.window().get_toolbar_y() < 40.0);
}
