//! Headless rendering with Slint's software renderer: checks layouts without a display.
//! Set `VIXEENY_SCREENSHOTS=<dir>` to also write the rendered frames as PNG files.

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
        video: "Video".into(),
        record: "Record".into(),
        stop_recording: "Stop recording".into(),
        replay_start: "Start replay buffer".into(),
        replay_stop: "Stop replay buffer".into(),
        replay_save: "Save replay".into(),
        settings: "Settings".into(),
        pin: "Keep open".into(),
        close: "Close".into(),
    }
}

fn side_state(edge: side_panel::Edge, dark: bool) -> side_panel::SideState {
    side_panel::SideState {
        recording: false,
        replay: true,
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
    // separator, 5 buttons, separator, each followed by a gap → the record button starts here
    let record_y = first + 9.0 + 2.0 + 5.0 * 42.0 + 9.0 + 2.0 + 20.0;
    click(record_y);
    assert_eq!(panel.chosen(), Some(Choice::RecordToggle));
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
fn settings_apply_immediately() {
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
    assert_eq!(seen.borrow().len(), 2, "one notification per change");
    let c = panel.config();
    assert!(!c.general.sounds);
    assert_eq!(c.general.theme, "dark");
    // The same value again is not a change.
    w.invoke_row_toggled("sounds".into(), false);
    assert_eq!(seen.borrow().len(), 2);
    panel.select_section(Section::Overlay);
    w.invoke_row_chosen("overlay_edge".into(), 0); // left, right, top, bottom
    assert_eq!(panel.config().overlay.edge, "left");

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
    assert_eq!(titles(&panel)[0], "General");
    assert_eq!(titles(&panel)[4], "Sound");
    panel.window().invoke_row_chosen("language".into(), 1); // auto, fr, en
    assert_eq!(panel.config().general.language, "fr");
    assert_eq!(titles(&panel)[0], "Général");
    assert_eq!(titles(&panel)[4], "Son");
    let first = panel.window().get_groups().row_data(0).unwrap();
    assert_eq!(first.rows.row_data(0).unwrap().label, "Langue");
}

#[test]
fn the_video_page_edits_the_current_profile_and_reports_problems() {
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.select_section(Section::Video);
    settings_render(&panel, "9-settings-video");
    let w = panel.window();
    w.invoke_row_chosen("fps".into(), 1); // 24, 30, 60 (and more on a fast monitor)
    assert_eq!(panel.config().video.fps, 30);
    w.invoke_row_chosen("split".into(), 1);
    assert_eq!(panel.config().video.split.mode, "size:2048");
    assert_eq!(w.get_notice(), "");
    // HEVC does not go in WebM: a problem the page says out loud.
    w.invoke_row_chosen("encoder_kind".into(), 1);
    w.invoke_row_chosen("encoder".into(), 1); // x264, x265, …
    w.invoke_row_chosen("container".into(), 2);
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
    // Row 6 is the replay toggle, row 7 the replay save (Ctrl+Shift+S). Keys are recorded, never typed.
    let record = |row: i32, slot: i32, keys: &str| {
        w.invoke_shortcut_record(row, slot);
        panel.captured(settings_panel::Captured::Combination(keys.into()));
    };
    record(6, 0, "Ctrl+Shift+F9");
    assert_eq!(panel.config().hotkeys.replay_toggle, ["Ctrl+Shift+F9"]);
    assert_eq!(seen.borrow().len(), 1);
    record(6, 1, "Ctrl+Shift+S");
    assert_eq!(panel.config().hotkeys.replay_toggle.len(), 1, "refused");
    let row = w.get_shortcuts().row_data(6).unwrap();
    assert!(row.error.contains("Save the replay"), "{}", row.error);
    assert_eq!(seen.borrow().len(), 1, "a refusal is not a change");
    // Escape stops the recording without touching anything.
    w.invoke_shortcut_record(6, 1);
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
        icon: None,
    };
    // A program's icon: a blue disc on a transparent ground.
    let disc: Vec<u8> = (0..32 * 32)
        .flat_map(|i| {
            let (x, y) = ((i % 32) as f32 - 15.5, (i / 32) as f32 - 15.5);
            if x * x + y * y < 15.0 * 15.0 {
                [30, 120, 230, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect();
    panel.set_audio(AudioDevices {
        outputs: vec![
            entry("o1", "Speakers (Realtek Audio)"),
            entry("o2", "Headset (Arctis 7)"),
        ],
        inputs: vec![entry("i1", "Microphone (Blue Yeti)")],
        programs: vec![
            AudioEntry {
                icon: Some(vixeeny_settings::Icon {
                    width: 32,
                    height: 32,
                    rgba: disc.into(),
                }),
                ..entry("Spotify.exe", "Spotify")
            },
            entry("chrome.exe", "Google Chrome"),
        ],
    });
    for section in Section::ALL {
        panel.select_section(section);
        let name = format!("9-page-{section:?}").to_lowercase();
        settings_render(&panel, &name);
    }
    // Narrow: the navigation keeps its icons only, the controls go under their text.
    panel.select_section(Section::General);
    settings_render_at(&panel, "9-page-narrow", (660, 720));
}

#[test]
fn the_updates_and_about_pages_show_what_the_host_gives_them() {
    use settings_panel::Line;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.window().set_animated(false);
    panel.select_section(Section::About);
    panel.set_lines(vec![
        Line {
            text: "Licence GPL-3.0-or-later".into(),
            detail: "https://github.com/Xantoom/Vixeeny".into(),
        },
        Line {
            text: "Logs".into(),
            detail: "C:\\Users\\me\\AppData\\Local\\Vixeeny\\logs".into(),
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
    panel.select_section(Section::Updates);
    settings_render(&panel, "9-settings-updates");
    panel.set_update(&settings_panel::UpdateView {
        stage: settings_panel::UpdateStage::Ready,
        title: "Version 1.3.0 is ready".into(),
        detail: "Restart Vixeeny to finish. It takes a second.".into(),
        progress: 1.0,
        action: "Restart".into(),
    });
    settings_render(&panel, "9-settings-updates-ready");
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
    w.set_animated(false);
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
