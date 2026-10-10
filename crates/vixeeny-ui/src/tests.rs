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
    // The widget: a preset, then a place clicked on the little screen.
    w.invoke_row_chosen("widget_corner".into(), 4); // top left, centre, right, bottom…
    assert_eq!(panel.config().recording_widget.corner, "bottom_center");
    w.invoke_row_text("widget_corner".into(), "custom:300:700".into());
    let c = panel.config();
    assert_eq!(c.recording_widget.corner, "custom");
    assert_eq!(c.recording_widget.custom, (0.3, 0.7));
    settings_render(&panel, "9-page-overlay-custom");

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
    w.invoke_row_chosen("p:rc.mode".into(), 4); // Quality: CRF, CQP; Bitrate: VBR, CBR
    assert_eq!(panel.config().video.params["rc.mode"], "vbr");
    // The rows that just appeared, shown at the end of their unfolding.
    panel.window().set_animated(false);
    w.invoke_row_toggled("expert_video".into(), true);
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
    panel.window().invoke_row_chosen("image_format".into(), 2); // (title), PNG, JPEG
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
fn rows_that_stop_applying_fold_away_then_go() {
    use slint::Model;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    let rows = |p: &settings_panel::SettingsPanel| {
        p.window()
            .get_groups()
            .iter()
            .flat_map(|g| g.rows.iter().collect::<Vec<_>>())
            .map(|r| (r.id.to_string(), r.leaving))
            .collect::<Vec<_>>()
    };
    panel.select_section(Section::Overlay);
    panel.window().invoke_row_toggled("widget".into(), false);
    // Still there, folding, in their place under the switch.
    let now = rows(&panel);
    let corner = now.iter().position(|(id, _)| id == "widget_corner");
    let widget = now.iter().position(|(id, _)| id == "widget");
    assert!(
        corner.is_some_and(|c| widget.is_some_and(|w| c == w + 1)),
        "{now:?}"
    );
    assert!(
        now.iter()
            .any(|(id, leaving)| id == "widget_corner" && *leaving)
    );
    // The next change takes them away for good.
    panel.window().invoke_row_chosen("overlay_edge".into(), 0);
    assert!(!rows(&panel).iter().any(|(id, _)| id == "widget_corner"));
    // Without animations, nothing lingers.
    panel.window().set_animated(false);
    panel.window().invoke_row_toggled("widget".into(), true);
    panel.window().invoke_row_toggled("widget".into(), false);
    assert!(!rows(&panel).iter().any(|(_, leaving)| *leaving));
}

#[test]
fn the_search_leads_to_the_setting() {
    use slint::Model;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.window().set_animated(false);
    panel.select_section(Section::General);
    let w = panel.window();
    w.set_search_text("dossier par".into());
    w.invoke_search_edited("dossier par".into());
    // English: nothing; the words are those of the window's language.
    assert_eq!(w.get_search_hits().row_count(), 0);
    w.invoke_search_edited("folder".into());
    let pages: Vec<String> = w
        .get_search_hits()
        .iter()
        .map(|h| h.page.to_string())
        .collect();
    assert!(pages.contains(&"Image".to_owned()), "{pages:?}");
    w.set_search_text("save repl".into());
    w.invoke_search_edited("save repl".into());
    settings_render(&panel, "9-settings-search");
    let hits = w.get_search_hits();
    assert_eq!(hits.row_count(), 1);
    assert_eq!(hits.row_data(0).unwrap().label, "Save the replay");
    // A folded expert setting: its section opens, the row is outlined.
    w.set_search_text(Default::default());
    panel.select_section(Section::Video);
    w.invoke_row_chosen("preset".into(), 2); // best, light, custom
    panel.select_section(Section::General);
    w.set_search_text("chroma".into());
    w.invoke_search_edited("chroma".into());
    assert_eq!(w.get_search_hits().row_count(), 1);
    w.invoke_search_picked(0);
    assert_eq!(w.get_search_text(), "");
    assert_eq!(panel.current_section(), Section::Video);
    assert_eq!(w.get_highlight_id(), "chroma");
}

#[test]
fn shortcuts_that_cannot_work_are_marked() {
    use slint::Model;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.window().set_animated(false);
    panel.select_section(Section::Shortcuts);
    let w = panel.window();
    // The region capture (row 0) is taken by another program.
    panel.set_taken_shortcuts(vec!["PrintScreen".into()]);
    let row = w.get_shortcuts().row_data(0).unwrap();
    // Red, the reason on hover only: no text under the action.
    assert!(
        row.keys
            .row_data(0)
            .unwrap()
            .tip
            .contains("another program")
    );
    assert!(row.error.is_empty());
    // One that Windows keeps.
    w.invoke_shortcut_record(5, 1);
    panel.captured(settings_panel::Captured::Combination("Win+Alt+R".into()));
    let row = w.get_shortcuts().row_data(5).unwrap();
    assert!(row.keys.row_data(1).unwrap().tip.contains("Windows"));
    assert!(row.keys.row_data(0).unwrap().tip.is_empty());
    settings_render(&panel, "9-settings-shortcuts-taken");
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
    assert_eq!(w.get_recording_keys(), "SHIFT + CTRL + F10");
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
    panel.window().set_animated(false);
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
    {
        use vixeeny_settings::machine::{Cpu, Disk, Gpu, Machine, Screen};
        panel.set_machine(Machine {
            cpu: Some(Cpu {
                name: "AMD Ryzen 7 9800X3D 8-Core Processor".into(),
                cores: 8,
                threads: 16,
            }),
            ram_bytes: 64 << 30,
            gpus: vec![
                Gpu {
                    name: "NVIDIA GeForce RTX 5080".into(),
                    integrated: false,
                    memory_bytes: 16 << 30,
                },
                Gpu {
                    name: "AMD Radeon(TM) Graphics".into(),
                    integrated: true,
                    memory_bytes: 0,
                },
            ],
            disks: vec![
                Disk {
                    letter: "C:".into(),
                    label: String::new(),
                    model: "CT2000T500SSD8".into(),
                    bytes: 2_000_398_934_016,
                    free_bytes: 612_000_000_000,
                },
                Disk {
                    letter: "D:".into(),
                    label: "Jeux".into(),
                    model: "Samsung SSD 990 PRO 4TB".into(),
                    bytes: 4_000_787_030_016,
                    free_bytes: 3_100_000_000_000,
                },
            ],
            screens: vec![
                Screen {
                    number: 1,
                    primary: true,
                    name: "Odyssey G80SH".into(),
                    width: 3840,
                    height: 2160,
                    hz: 240,
                },
                Screen {
                    number: 2,
                    primary: false,
                    name: "LG ULTRAWIDE".into(),
                    width: 3440,
                    height: 1440,
                    hz: 144,
                },
            ],
        });
    }
    for section in Section::ALL {
        panel.select_section(section);
        let name = format!("9-page-{section:?}").to_lowercase();
        settings_render(&panel, &name);
    }
    // The programs to record, ticked one by one.
    panel.select_section(Section::Audio);
    panel.window().invoke_row_chosen("audio_capture".into(), 3);
    panel
        .window()
        .invoke_row_toggled("src:app:Spotify.exe".into(), true);
    settings_render(&panel, "9-page-audio-programs");
    // The microphone: its level, its volume.
    panel.window().invoke_row_toggled("mic_on".into(), true);
    panel.set_mic_level(0.62);
    settings_render_at(&panel, "9-page-audio-mic", (1040, 1100));
    // The smallest window: nothing overlaps.
    panel.select_section(Section::General);
    settings_render_at(&panel, "9-page-smallest", (960, 640));
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

// The tooltip of an "i" shows anywhere on its dot, and stays while the pointer moves on it: a
// popup took the pointer from the dot and closed again at the next move.
#[test]
fn info_tooltip_stays_while_the_pointer_moves_on_its_dot() {
    use slint::LogicalPosition;
    use slint::platform::WindowEvent;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.select_section(Section::Overlay);
    let window = WINDOW.with(Rc::clone);
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    let size = (2080, 1440);
    let base = settings_render_at(&panel, "tip-base", size);
    let changed = |name: &str| {
        let shot = settings_render_at(&panel, name, size);
        shot.as_slice()
            .iter()
            .zip(base.as_slice())
            .filter(|(a, b)| a != b)
            .count()
    };
    let move_to = |x: f32, y: f32| {
        window.dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(x, y),
        });
    };
    // The glyph, the ring, inside the dot off the glyph, and the margin around it.
    for (x, y) in [
        (354.0, 173.0),
        (348.5, 173.0),
        (354.0, 168.0),
        (350.0, 169.0),
        (359.5, 177.5),
    ] {
        for dx in [0.0, 0.5, 1.0, 1.5] {
            move_to(x + dx, y);
            assert!(
                changed("tip-on") > 20_000,
                "no tooltip at ({}, {y})",
                x + dx
            );
        }
        move_to(600.0, 600.0);
        assert!(
            changed("tip-off") < 2_000,
            "the tooltip stays away from the dot"
        );
    }
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
}

// Behind Mica the window draws no background of its own (it shows through, black here) and the
// cards only veil it.
#[test]
fn with_mica_the_window_leaves_its_background_to_it() {
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.select_section(Section::Video);
    panel.window().global::<Theme>().set_mica(true);
    let shot = settings_render(&panel, "9-settings-mica");
    let at = |x: usize, y: usize| shot.as_slice()[y * 1040 + x];
    let nav = at(100, 600);
    assert_eq!((nav.r, nav.g, nav.b), (0, 0, 0));
    let card = at(600, 140);
    assert!(
        card.r > 0 && card.r < 0x1c,
        "a veil, not the opaque card: {card:?}"
    );
    panel.window().global::<Theme>().set_mica(false);
}
