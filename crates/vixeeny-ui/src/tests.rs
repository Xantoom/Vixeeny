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
    // The custom options of x264 in VBR: a bitrate and a maximum instead of the quality.
    w.invoke_row_chosen("container".into(), 0);
    w.invoke_row_chosen("encoder".into(), 0);
    w.invoke_row_chosen("preset".into(), 2); // best, light, custom
    w.invoke_row_chosen("p:rc.mode".into(), 1); // quality, VBR, CBR
    assert_eq!(panel.config().video.params["rc.mode"], "vbr");
    settings_render_at(&panel, "9-settings-video-custom", (1040, 2200));
}

#[test]
fn rows_that_appear_on_the_same_page_unfold() {
    use slint::Model;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    let fresh = |p: &settings_panel::SettingsPanel| {
        p.window()
            .get_groups()
            .iter()
            .flat_map(|g| g.rows.iter().collect::<Vec<_>>())
            .filter(|r| r.fresh)
            .map(|r| r.id.to_string())
            .collect::<Vec<_>>()
    };
    panel.select_section(Section::Image);
    assert!(fresh(&panel).is_empty(), "a new page does not unfold");
    // JPEG brings its own options in: they unfold, the rows already there do not.
    panel.window().invoke_row_chosen("image_format".into(), 1);
    let unfolded = fresh(&panel);
    assert!(
        unfolded.contains(&"jpeg_quality".to_owned()),
        "{unfolded:?}"
    );
    assert!(!unfolded.contains(&"image_format".to_owned()));
    panel.select_section(Section::Video);
    assert!(fresh(&panel).is_empty());
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
    // Leaving the window stops the recording without touching anything.
    w.invoke_shortcut_record(6, 1);
    assert!(panel.captured(settings_panel::Captured::Cancel));
    assert!(
        !panel.captured(settings_panel::Captured::Cancel),
        "nothing is recording any more"
    );
    assert_eq!(seen.borrow().len(), 1);
    // The keys show while held, in the order pressed; letting go saves them.
    w.invoke_shortcut_record(6, 1);
    for code in ["ShiftLeft", "ControlRight", "ShiftRight", "F10"] {
        assert!(panel.key(code, true));
    }
    assert_eq!(w.get_recording_keys(), "Shift + Ctrl + F10");
    assert!(panel.key("F10", false));
    assert_eq!(
        panel.config().hotkeys.replay_toggle,
        ["Ctrl+Shift+F9", "Shift+Ctrl+F10"]
    );
    assert!(
        !panel.key("ShiftLeft", false),
        "recorded: the keys are free again"
    );
    // Escape empties the slot.
    w.invoke_shortcut_record(6, 0);
    assert!(panel.key("Escape", true));
    assert_eq!(panel.config().hotkeys.replay_toggle, ["Shift+Ctrl+F10"]);
}

#[test]
fn a_chord_ends_at_the_first_key_let_go() {
    use settings_panel::{Captured, Chord, ChordStep};
    let mut chord = Chord::default();
    chord.press("ControlLeft");
    // Modifiers alone, let go: forgotten, still waiting.
    assert_eq!(chord.release("ControlLeft"), ChordStep::Held(String::new()));
    chord.press("AltLeft");
    // Print Screen only reports going up.
    assert_eq!(
        chord.release("PrintScreen"),
        ChordStep::Done(Captured::Combination("Alt+PrintScreen".into()))
    );
    chord.press("ShiftLeft");
    chord.press("KeyR");
    assert_eq!(
        chord.release("ShiftLeft"),
        ChordStep::Done(Captured::Combination("Shift+KeyR".into()))
    );
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
