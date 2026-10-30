//! Headless rendering with Slint's software renderer: checks layouts without a display. Set `VIXEENY_SCREENSHOTS=<dir>`
//! to also write the rendered frames as PNG files.
#![allow(clippy::unwrap_used, reason = "test helpers fail the test")]

use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel, SharedPixelBuffer};

use vixeeny_ui::*;

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
    let Ok(file) = std::fs::File::create(std::path::Path::new(&dir).join(format!("{name}.png"))) else {
        return;
    };
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), buf.width(), buf.height());
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let Ok(mut writer) = enc.write_header() else {
        return;
    };
    let bytes: Vec<u8> = buf.as_slice().iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    let _ = writer.write_image_data(&bytes);
}

fn settings_panel() -> (settings_panel::SettingsPanel, Rc<std::cell::RefCell<Vec<vixeeny_common::config::Config>>>) {
    use std::cell::RefCell;
    WINDOW.with(|_| ());
    let panel = settings_panel::SettingsPanel::new(
        vixeeny_common::config::Config::default(),
        Some("en-US".into()),
        "1.2.3",
        vixeeny_ui::theme::Look::dark(),
    )
    .unwrap();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    panel.on_change(move |c| sink.borrow_mut().push(c.clone()));
    (panel, seen)
}

fn settings_render(panel: &settings_panel::SettingsPanel, name: &str) -> SharedPixelBuffer<Rgb8Pixel> {
    settings_render_at(panel, name, (1040, 720))
}

fn settings_render_at(
    panel: &settings_panel::SettingsPanel,
    name: &str,
    (w, h): (u32, u32),
) -> SharedPixelBuffer<Rgb8Pixel> {
    let window = WINDOW.with(Rc::clone);
    window.set_size(PhysicalSize::new(w, h));
    panel.window().show().unwrap();
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
fn the_side_bar_shortcut_follows_the_overlay_switch() {
    use slint::Model;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.select_section(Section::Overlay);
    panel.window().invoke_row_toggled("overlay_enabled".into(), false);
    panel.select_section(Section::Shortcuts);
    let rows = panel.window().get_shortcuts();
    assert!(!rows.row_data(10).unwrap().enabled);
    assert!((0..10).all(|i| rows.row_data(i).unwrap().enabled));
    settings_render_at(&panel, "shortcuts-overlay-disabled", (1040, 1100));
    panel.select_section(Section::Overlay);
    panel.window().invoke_row_toggled("overlay_enabled".into(), true);
    panel.select_section(Section::Shortcuts);
    assert!(panel.window().get_shortcuts().row_data(10).unwrap().enabled);
}

#[test]
fn updates_add_a_navigation_badge_for_available_and_ready_states() {
    use settings_panel::{UpdateStage, UpdateView};
    let (panel, _) = settings_panel();
    let base = settings_render(&panel, "update-badge-none");
    for stage in [UpdateStage::Available, UpdateStage::Ready] {
        panel.set_update(&UpdateView { stage, ..Default::default() });
        let shot = settings_render(&panel, "update-badge-present");
        // On the updates tab, at the bottom of the column.
        let changed = (630..672)
            .flat_map(|y| (12..208).map(move |x| y * 1040 + x))
            .filter(|i| base.as_slice()[*i] != shot.as_slice()[*i])
            .count();
        assert!(changed > 10, "no update badge for {stage:?}");
        // Nothing else of the column changes.
        assert!(
            (40..620)
                .flat_map(|y| (12..208).map(move |x| y * 1040 + x))
                .all(|i| base.as_slice()[i] == shot.as_slice()[i])
        );
    }
}

#[test]
fn long_file_names_scroll_to_the_text_cursor() {
    use slint::platform::{Key, PointerEventButton, WindowEvent};
    let (panel, _) = settings_panel();
    panel.select_section(vixeeny_settings::Section::Recording);
    let w = panel.window();
    w.invoke_row_text("template:videos".into(), format!("{}{}", "A".repeat(80), "Z".repeat(80)).into());
    slint::platform::update_timers_and_animations();
    settings_render(&panel, "long-name-initial");
    let at = slint::LogicalPosition::new(800.0, 283.0);
    w.window().dispatch_event(WindowEvent::PointerMoved { position: at });
    w.window().dispatch_event(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left });
    w.window().dispatch_event(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left });
    w.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Home.into() });
    let start = settings_render(&panel, "long-name-start");
    w.window().dispatch_event(WindowEvent::KeyPressed { text: Key::End.into() });
    let end = settings_render(&panel, "long-name-end");
    let changed = (274..294)
        .flat_map(|y| (725..930).map(move |x| y * 1040 + x))
        .filter(|i| start.as_slice()[*i] != end.as_slice()[*i])
        .count();
    assert!(changed > 300, "the field did not scroll to the end: {changed}");
    w.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Home.into() });
    let again = settings_render(&panel, "long-name-start-again");
    let changed = (274..294)
        .flat_map(|y| (725..930).map(move |x| y * 1040 + x))
        .filter(|i| start.as_slice()[*i] != again.as_slice()[*i])
        .count();
    assert!(changed < 100, "the field did not return to the beginning: {changed}");
}

#[test]
fn image_profiles_and_shortcut_groups_render_in_french() {
    use slint::Model;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.window().invoke_row_chosen("language".into(), 1);
    panel.window().set_animated(false);
    panel.select_section(Section::Image);
    let window = panel.window();
    assert_eq!(window.get_groups().row_count(), 6);
    for kind in vixeeny_common::config::ImageKind::ALL {
        window.invoke_row_toggled(format!("image:{}:custom_encoding", kind.name()).into(), true);
    }
    window.invoke_row_chosen("image:window:image_format".into(), 0);
    window.invoke_row_number("image:region:jpeg_quality".into(), 61);
    assert_eq!(panel.config().image.region.jpeg.quality, 61);
    assert_eq!(panel.config().image.window.format, "png");
    assert_eq!(panel.config().image.fullscreen.format, "jpeg");
    settings_render_at(&panel, "settings-images-fr", (1040, 1900));
    panel.select_section(Section::Shortcuts);
    assert_eq!(window.get_shortcut_spans().row_count(), 4);
    settings_render_at(&panel, "settings-shortcuts-fr", (1040, 1100));
    panel.select_section(Section::Overlay);
    settings_render(&panel, "settings-overlays-fr");
}

#[test]
fn a_page_resets_once_changed_and_asked() {
    use vixeeny_settings::Section;
    let (panel, seen) = settings_panel();
    panel.select_section(Section::General);
    let w = panel.window();
    assert!(!w.get_page_changed(), "as it comes: nothing to reset");
    w.invoke_row_chosen("theme".into(), 1); // system, light, dark
    assert!(w.get_page_changed());
    w.set_confirming(1);
    settings_render(&panel, "9-settings-reset-page");
    w.invoke_reset_page();
    assert_eq!(panel.config().general.theme, "system");
    assert!(!w.get_page_changed());
    assert_eq!(seen.borrow().len(), 2);
    // Everything: from another page too.
    w.invoke_row_chosen("theme".into(), 2);
    panel.select_section(Section::Overlay);
    w.invoke_row_toggled("overlay_enabled".into(), false);
    w.set_confirming(2);
    settings_render(&panel, "9-settings-reset-all");
    w.invoke_reset_all();
    assert_eq!(panel.config(), vixeeny_common::config::Config::default());
}

#[test]
fn settings_apply_immediately() {
    use vixeeny_settings::Section;
    let (panel, seen) = settings_panel();
    panel.select_section(Section::General);
    settings_render(&panel, "9-settings-general");
    let w = panel.window();
    let groups = w.get_groups();
    w.invoke_row_toggled("autostart".into(), false);
    // The page keeps its rows (and their controls, which then animate): only the row changed.
    assert!(w.get_groups() == groups, "the rows were rebuilt");
    w.invoke_row_chosen("theme".into(), 2); // system, light, dark
    assert_eq!(seen.borrow().len(), 2, "one notification per change");
    let c = panel.config();
    assert!(!c.general.autostart);
    assert_eq!(c.general.theme, "dark");
    // The same value again is not a change.
    w.invoke_row_toggled("autostart".into(), false);
    assert_eq!(seen.borrow().len(), 2);
    panel.select_section(Section::Overlay);
    w.invoke_row_chosen("overlay_edge".into(), 0); // left, right, top, bottom
    assert_eq!(panel.config().overlay.edge, "left");
    // The widget: a preset, then a place clicked on the little screen.
    w.invoke_row_chosen("widget_corner".into(), 4); // top left, centre, right, bottom…
    assert_eq!(panel.config().recording_widget.corner, "bottom_center");
    w.invoke_row_text("widget_corner".into(), "custom:300:700".into());
    // A text is applied once the event in progress ends.
    slint::platform::update_timers_and_animations();
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
        p.window().get_section_titles().iter().map(|s| s.to_string()).collect::<Vec<_>>()
    };
    assert_eq!(titles(&panel)[0], "General");
    assert_eq!(titles(&panel)[2], "Recording");
    panel.window().invoke_row_chosen("language".into(), 1); // auto, fr, en
    assert_eq!(panel.config().general.language, "fr");
    assert_eq!(titles(&panel)[0], "Général");
    assert_eq!(titles(&panel)[2], "Enregistrement");
    let first = panel.window().get_groups().row_data(0).unwrap();
    assert_eq!(first.rows.row_data(0).unwrap().label, "Langue");
}

#[test]
fn the_video_page_edits_the_current_profile_and_reports_problems() {
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.select_section(Section::Recording);
    settings_render(&panel, "9-settings-video");
    let w = panel.window();
    w.invoke_row_chosen("fps".into(), 0); // 30, 60 (and 120 on a fast monitor)
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
    w.invoke_row_chosen("preset".into(), 4); // quality, balanced, compact, a line, custom
    w.invoke_row_chosen("p:rc.mode".into(), 1); // CRF, VBR, CBR
    assert_eq!(panel.config().video.params["rc.mode"], "vbr");
    // The rows that just appeared, shown at the end of their unfolding.
    panel.window().set_animated(false);
    w.invoke_row_toggled("expert_video".into(), true);
    settings_render_at(&panel, "9-settings-video-custom", (1040, 2200));
}

#[test]
fn a_backdrop_colour_is_picked_or_typed() {
    use vixeeny_settings::Section;
    let (panel, seen) = settings_panel();
    panel.select_section(Section::Image);
    panel.window().invoke_row_chosen("image:window:window_backdrop".into(), 3); // gradient
    let w = panel.window();
    w.invoke_row_colour("image:window:backdrop_gradient".into(), 1, slint::Color::from_rgb_u8(1, 2, 3));
    settings_render_at(&panel, "image:window:backdrop_gradient", (1040, 1500));
    // Its start opens the picker.
    let at = slint::LogicalPosition::new(726.0, 744.0);
    let button = slint::platform::PointerEventButton::Left;
    for event in [
        slint::platform::WindowEvent::PointerMoved { position: at },
        slint::platform::WindowEvent::PointerPressed { position: at, button },
        slint::platform::WindowEvent::PointerReleased { position: at, button },
    ] {
        w.window().dispatch_event(event);
    }
    slint::platform::update_timers_and_animations();
    settings_render_at(&panel, "backdrop_picker", (1040, 1500));
    let last = |seen: &std::cell::RefCell<Vec<vixeeny_common::config::Config>>| {
        seen.borrow().last().map(|c| c.image.window.clone())
    };
    assert_eq!(last(&seen).map(|i| i.backdrop_gradient), Some("#654ea3,#010203".into()));
    w.invoke_row_chosen("image:window:window_backdrop".into(), 2); // colour
    w.invoke_row_code("image:window:backdrop_colour".into(), 0, "#ABCDEF".into());
    slint::platform::update_timers_and_animations();
    assert_eq!(last(&seen).map(|i| i.backdrop_colour), Some("#abcdef".into()));
    // Not a colour: nothing changes.
    w.invoke_row_code("image:window:backdrop_colour".into(), 0, "#zz".into());
    slint::platform::update_timers_and_animations();
    assert_eq!(last(&seen).map(|i| i.backdrop_colour), Some("#abcdef".into()));
    settings_render_at(&panel, "image:window:backdrop_colour", (1040, 1500));
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

    // PNG brings its own options in: they unfold, the rows already there do not.
    panel.window().invoke_row_chosen("image:defaults:image_format".into(), 0); // PNG, JPEG
    let unfolded = fresh(&panel);
    assert!(unfolded.contains(&"image:defaults:png_compression".to_owned()), "{unfolded:?}");
    assert!(!unfolded.contains(&"image:defaults:image_format".to_owned()));
    panel.select_section(Section::Recording);
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
    assert!(corner.is_some_and(|c| widget.is_some_and(|w| c == w + 2)), "{now:?}");
    assert!(now.iter().any(|(id, leaving)| id == "widget_corner" && *leaving));
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
    let pages: Vec<String> = w.get_search_hits().iter().map(|h| h.page.to_string()).collect();
    assert!(pages.contains(&"Image".to_owned()), "{pages:?}");
    w.set_search_text("save repl".into());
    w.invoke_search_edited("save repl".into());
    settings_render(&panel, "9-settings-search");
    // Both shortcuts that save the replay, the plain one first.
    let labels: Vec<String> = w.get_search_hits().iter().map(|h| h.label.to_string()).collect();
    let shortcuts: Vec<&String> = labels.iter().filter(|l| l.ends_with("the replay")).collect();
    assert_eq!(shortcuts, ["Save the replay", "Save and edit the replay"]);
    // And its notification.
    assert!(labels.contains(&"Replay saved".to_owned()), "{labels:?}");
    // A folded expert setting: its section opens, the row is outlined.
    w.set_search_text(Default::default());
    panel.select_section(Section::Recording);
    w.invoke_row_chosen("preset".into(), 4); // quality, balanced, compact, a line, custom
    panel.select_section(Section::General);
    w.set_search_text("chroma".into());
    w.invoke_search_edited("chroma".into());
    assert_eq!(w.get_search_hits().row_count(), 1);
    w.invoke_search_picked(0);
    assert_eq!(w.get_search_text(), "");
    assert_eq!(panel.current_section(), Section::Recording);
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
    assert!(row.keys.row_data(0).unwrap().tip.contains("another program"));
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
fn a_refused_shortcut_says_why_in_the_users_language() {
    use slint::Model;
    use vixeeny_settings::Section;
    WINDOW.with(|_| ());
    let panel = settings_panel::SettingsPanel::new(
        vixeeny_common::config::Config::default(),
        Some("fr-FR".into()),
        "1.2.3",
        vixeeny_ui::theme::Look::dark(),
    )
    .unwrap();
    panel.select_section(Section::Shortcuts);
    let w = panel.window();
    // A letter alone types text: refused, the key by its name.
    w.invoke_shortcut_record(0, 1);
    panel.captured(settings_panel::Captured::Combination("KeyR".into()));
    let row = w.get_shortcuts().row_data(0).unwrap();
    assert_eq!(row.error, "R demande CTRL, ALT, SHIFT ou WIN");
}

#[test]
fn shortcuts_are_edited_in_place_and_conflicts_are_explained() {
    use slint::Model;
    use vixeeny_settings::Section;
    let (panel, seen) = settings_panel();
    panel.select_section(Section::Shortcuts);
    settings_render(&panel, "9-settings-shortcuts");
    let w = panel.window();
    // Row 7 is the replay toggle, row 8 the replay save (Ctrl+F11). Keys are recorded, never typed.
    let record = |row: i32, slot: i32, keys: &str| {
        w.invoke_shortcut_record(row, slot);
        panel.captured(settings_panel::Captured::Combination(keys.into()));
    };
    record(7, 0, "Ctrl+Shift+F7");
    assert_eq!(panel.config().hotkeys.replay_toggle, ["Ctrl+Shift+F7"]);
    assert_eq!(seen.borrow().len(), 1);
    record(7, 1, "Ctrl+F11");
    assert_eq!(panel.config().hotkeys.replay_toggle.len(), 1, "refused");
    let row = w.get_shortcuts().row_data(7).unwrap();
    assert!(row.error.contains("Save the replay"), "{}", row.error);
    assert_eq!(seen.borrow().len(), 1, "a refusal is not a change");
    // Leaving the window stops the recording without touching anything.
    w.invoke_shortcut_record(7, 1);
    assert!(panel.captured(settings_panel::Captured::Cancel));
    assert!(!panel.captured(settings_panel::Captured::Cancel), "nothing is recording any more");
    assert_eq!(seen.borrow().len(), 1);
    // The keys show while held, in the order pressed; letting go saves them.
    w.invoke_shortcut_record(7, 1);
    for code in ["ShiftLeft", "ControlRight", "ShiftRight", "F6"] {
        assert!(panel.key(code, true));
    }
    assert_eq!(w.get_recording_keys(), "SHIFT + CTRL + F6");
    assert!(panel.key("F6", false));
    assert_eq!(panel.config().hotkeys.replay_toggle, ["Ctrl+Shift+F7", "Shift+Ctrl+F6"]);
    assert!(!panel.key("ShiftLeft", false), "recorded: the keys are free again");
    // Escape cancels: the slot keeps its shortcut.
    w.invoke_shortcut_record(7, 0);
    assert!(panel.key("Escape", true));
    assert_eq!(panel.config().hotkeys.replay_toggle, ["Ctrl+Shift+F7", "Shift+Ctrl+F6"]);
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
    assert_eq!(chord.release("PrintScreen"), ChordStep::Done(Captured::Combination("Alt+PrintScreen".into())));
    chord.press("ShiftLeft");
    chord.press("KeyR");
    assert_eq!(chord.release("ShiftLeft"), ChordStep::Done(Captured::Combination("Shift+KeyR".into())));
}

#[test]
fn every_page_of_the_settings_renders() {
    use vixeeny_settings::{AudioDevices, AudioEntry, Section};
    let (panel, _) = settings_panel();
    panel.window().set_animated(false);
    let entry = |id: &str, name: &str| AudioEntry { id: id.into(), name: name.into(), icon: None };
    // A program's icon: a blue disc on a transparent ground.
    let disc: Vec<u8> = (0..32 * 32)
        .flat_map(|i| {
            let (x, y) = ((i % 32) as f32 - 15.5, (i / 32) as f32 - 15.5);
            if x * x + y * y < 15.0 * 15.0 { [30, 120, 230, 255] } else { [0, 0, 0, 0] }
        })
        .collect();
    panel.set_audio(AudioDevices {
        outputs: vec![entry("o1", "Speakers (Realtek Audio)"), entry("o2", "Headset (Arctis 7)")],
        inputs: vec![entry("i1", "Microphone (Blue Yeti)")],
        programs: vec![
            AudioEntry {
                icon: Some(vixeeny_settings::Icon { width: 32, height: 32, rgba: disc.into() }),
                ..entry("Spotify.exe", "Spotify")
            },
            entry("chrome.exe", "Google Chrome"),
        ],
        games: Vec::new(),
    });
    panel.set_machine(vixeeny_settings::Machine::sample());
    for section in Section::ALL {
        panel.select_section(section);
        let name = format!("9-page-{section:?}").to_lowercase();
        settings_render(&panel, &name);
    }
    // The programs to record, ticked one by one.
    panel.select_section(Section::Recording);
    panel.window().invoke_row_chosen("audio_capture".into(), 3);
    panel.window().invoke_row_toggled("src:app:Spotify.exe".into(), true);
    settings_render(&panel, "9-page-audio-programs");
    // The microphone: its level, its volume.
    panel.window().invoke_row_toggled("mic_on".into(), true);
    panel.set_mic_level(0.62);
    settings_render_at(&panel, "9-page-audio-mic", (1040, 1100));
    // The replay on: where it goes, what it keeps, what it records with, the games (none open).
    panel.select_section(Section::Replay);
    panel.window().invoke_row_toggled("replay_enabled".into(), true);
    panel.window().invoke_row_toggled("replay_after".into(), true);
    panel.window().invoke_row_toggled("games".into(), true);
    settings_render_at(&panel, "9-page-replay-on", (1040, 2400));
    // The smallest window: nothing overlaps.
    panel.select_section(Section::General);
    settings_render_at(&panel, "9-page-smallest", (960, 640));
    // In French (longer words): every page at the narrowest width, on a wide screen, and zoomed to 150 %.
    panel.window().invoke_row_chosen("language".into(), 1);
    for section in Section::ALL {
        panel.select_section(section);
        let name = format!("{section:?}").to_lowercase();
        settings_render_at(&panel, &format!("9-fr-narrow-{name}"), (960, 1600));
        settings_render_at(&panel, &format!("9-fr-wide-{name}"), (1920, 1080));
    }
    let window = WINDOW.with(Rc::clone);
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.5 });
    for section in Section::ALL {
        panel.select_section(section);
        let name = format!("{section:?}").to_lowercase();
        settings_render_at(&panel, &format!("9-fr-150-{name}"), (1440, 960));
    }
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
}

#[test]
fn identical_host_data_preserves_the_settings_page_model() {
    let (panel, _) = settings_panel();
    panel.select_section(vixeeny_settings::Section::Recording);
    let before = panel.window().get_groups();
    panel.set_display(vixeeny_settings::Display::default());
    panel.set_admin(vixeeny_settings::Admin::default());
    panel.set_audio(vixeeny_settings::AudioDevices::default());
    assert_eq!(panel.window().get_groups(), before);
    panel.set_display(vixeeny_settings::Display { hdr: true, max_refresh: 144 });
    assert_ne!(panel.window().get_groups(), before);
}

#[test]
fn download_progress_keeps_the_notes_model_until_the_notes_change() {
    use settings_panel::{UpdateStage, UpdateView};
    use vixeeny_ui::slint::Model;
    let (panel, _) = settings_panel();
    let mut view = UpdateView {
        stage: UpdateStage::Downloading,
        notes: vec![NoteLine { kind: 2, level: 0, text: "A release note".into() }],
        ..Default::default()
    };
    panel.set_update(&view);
    let before = panel.window().get_update_notes();
    view.progress = 0.5;
    panel.set_update(&view);
    assert_eq!(panel.window().get_update_notes(), before);
    assert_eq!(panel.window().get_update_progress(), 0.5);
    view.notes[0].text = "Changed release note".into();
    panel.set_update(&view);
    assert_ne!(panel.window().get_update_notes(), before);
    assert_eq!(panel.window().get_update_notes().row_data(0).unwrap().text, "Changed release note");
    view.notes.clear();
    panel.set_update(&view);
    assert_eq!(panel.window().get_update_notes().row_count(), 0);
}

#[test]
fn the_updates_and_about_pages_show_what_the_host_gives_them() {
    use settings_panel::Line;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.window().set_animated(false);
    panel.select_section(Section::About);
    panel.set_lines(vec![
        Line { text: "Licence GPL-3.0-or-later".into(), detail: "https://github.com/Xantoom/Vixeeny".into() },
        Line { text: "Logs".into(), detail: "C:\\Users\\me\\AppData\\Local\\Vixeeny\\logs".into() },
    ]);
    panel.set_update(&settings_panel::UpdateView {
        stage: settings_panel::UpdateStage::Downloading,
        title: "Downloading version 1.3.0…".into(),
        detail: "12.4 MB of 31.0 MB".into(),
        progress: 0.4,
        action: String::new(),
        ..Default::default()
    });
    settings_render(&panel, "9-settings-about");
    panel.select_section(Section::Updates);
    settings_render(&panel, "9-settings-updates");
    panel.set_update(&settings_panel::UpdateView {
        stage: settings_panel::UpdateStage::Ready,
        title: "Version 1.3.0 is ready".into(),
        detail: "It installs at the next start of Vixeeny.".into(),
        progress: 1.0,
        action: "Restart".into(),
        notes_title: "Release notes".into(),
        notes: [
            (1, 0, "What's new"),
            (2, 0, "Video and Sound are now one Recording page, with three profiles on every page."),
            (2, 1, "Quality, Balanced and Performance."),
            (1, 0, "Fixes"),
            // Long enough to take several lines: the notes are as high as their lines.
            (
                2,
                0,
                "The replay now shows a game's own web pages (shop, news, events, account) when they open over the \
                 game. Other programs over the game still stay out of the replay, and the buffer goes on through the \
                 change.",
            ),
            (2, 0, "A window capture keeps its bottom rows."),
            (2, 0, "Settings are saved whole."),
        ]
        .into_iter()
        .map(|(kind, level, text)| vixeeny_ui::NoteLine { kind, level, text: text.into() })
        .collect(),
    });
    // The separate notes card starts unfolded.
    assert!(panel.window().get_notes_open());
    settings_render(&panel, "9-settings-updates-ready");
    // Up to date: the card is green.
    panel.set_update(&settings_panel::UpdateView {
        stage: settings_panel::UpdateStage::Idle,
        title: "Vixeeny 1.2.0 is up to date".into(),
        detail: "Last checked: 2026-10-11, 14:32".into(),
        progress: 0.0,
        action: "Check now".into(),
        ..Default::default()
    });
    settings_render(&panel, "9-settings-updates-current");
}

// The label keeps its tooltip while the pointer moves over it.
#[test]
fn info_tooltip_stays_while_the_pointer_moves_over_its_label() {
    use slint::LogicalPosition;
    use slint::platform::WindowEvent;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.select_section(Section::Recording);
    let window = WINDOW.with(Rc::clone);
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 2.0 });
    let size = (2080, 1440);
    let base = settings_render_at(&panel, "tip-base", size);
    let changed = |name: &str| {
        let shot = settings_render_at(&panel, name, size);
        shot.as_slice().iter().zip(base.as_slice()).filter(|(a, b)| a != b).count()
    };
    let move_to = |x: f32, y: f32| {
        window.dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
    };
    // Different points of the file-name label, including its text and the space beside it.
    for (x, y) in [(260.0, 285.0), (280.0, 285.0), (300.0, 280.0), (310.0, 281.0), (300.0, 284.5)] {
        for dx in [0.0, 0.5, 1.0, 1.5] {
            move_to(x + dx, y);
            assert!(changed("tip-on") > 20_000, "no tooltip at ({}, {y})", x + dx);
        }
        move_to(600.0, 600.0);
        assert!(changed("tip-off") < 2_000, "the tooltip stays away from the label");
    }
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: 1.0 });
}

// Behind Mica the window's background lets it through a little (black here, so darker) and the cards only veil it.
#[test]
fn with_mica_the_window_lets_it_through() {
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.select_section(Section::Recording);
    panel.window().global::<Theme>().set_mica(true);
    let shot = settings_render(&panel, "9-settings-mica");
    let at = |x: usize, y: usize| shot.as_slice()[y * 1040 + x];
    let nav = at(100, 600);
    assert!(nav.r > 0 && nav.r < 0x14, "the background, see-through: {nav:?}");
    let card = at(600, 140);
    assert!(card.r > nav.r, "a veil over it: {card:?}");
    panel.window().global::<Theme>().set_mica(false);
}

// A label hovered, then the page scrolled under the pointer: its tooltip clears when the pointer leaves.
#[test]
fn a_tooltip_goes_after_a_scroll() {
    use slint::LogicalPosition;
    use slint::platform::{PointerEventButton, WindowEvent};
    use vixeeny_encode::probe::{EncoderProbe, FormatProbe, ProbeResult};
    use vixeeny_encode::registry::Chroma;
    use vixeeny_settings::Section;
    let (panel, _) = settings_panel();
    panel.window().set_animated(false);
    panel.set_probe(
        Some(ProbeResult {
            encoders: vec![EncoderProbe {
                id: "nvenc_h264".into(),
                adapter: Some(0),
                formats: vec![FormatProbe { depth: 8, chroma: Chroma::C420, uhd: true, hdr: false }],
                tools: None,
                max: None,
            }],
            ..ProbeResult::default()
        }),
        false,
    );
    panel.select_section(Section::Recording);
    panel.window().invoke_row_chosen("preset".into(), 4); // custom: the expert settings
    let window = WINDOW.with(Rc::clone);
    let size = (1040, 720);
    let at = |x: f32, y: f32| LogicalPosition::new(x, y);
    let scroll = |y: f32, dy: f32| {
        window.dispatch_event(WindowEvent::PointerScrolled { position: at(600.0, y), delta_x: 0.0, delta_y: dy });
    };
    settings_render_at(&panel, "tip-scroll-0", size);
    window.dispatch_event(WindowEvent::PointerMoved { position: at(600.0, 500.0) });
    scroll(500.0, -2000.0);
    settings_render_at(&panel, "tip-scroll-1", size);
    for event in [
        WindowEvent::PointerPressed { position: at(600.0, 493.0), button: PointerEventButton::Left },
        WindowEvent::PointerReleased { position: at(600.0, 493.0), button: PointerEventButton::Left },
    ] {
        window.dispatch_event(event);
    }
    scroll(500.0, -4000.0);
    settings_render_at(&panel, "tip-scroll-2", size);
    // Where the pointer ends, away from any label.
    let end = at(335.0 + 49.0 * 5.5, 604.0 - 49.0 * 10.0);
    window.dispatch_event(WindowEvent::PointerMoved { position: end });
    scroll(end.y, 100.0);
    let clean = settings_render_at(&panel, "tip-scroll-3", size);
    scroll(end.y, -100.0);
    // A label near the bottom; a little scroll; the pointer moves up and away.
    window.dispatch_event(WindowEvent::PointerMoved { position: at(335.0, 604.0) });
    scroll(604.0, 100.0);
    for k in 0..50 {
        let k = k as f32;
        window.dispatch_event(WindowEvent::PointerMoved { position: at(335.0 + k * 5.5, 604.0 - k * 10.0) });
    }
    let left = settings_render_at(&panel, "tip-scroll-4", size);
    let differ = left.as_slice().iter().zip(clean.as_slice()).filter(|(a, b)| a != b).count();
    assert!(differ < 500, "the tooltip stayed ({differ} pixels)");
}

#[test]
fn the_whats_new_window_lists_the_changes_by_section() {
    let window = WINDOW.with(Rc::clone);
    let whats_new = WhatsNewWindow::new().unwrap();
    whats_new.set_heading("Vixeeny has been updated to 1.3.0".into());
    whats_new.set_show_label("Show what's new after an update".into());
    whats_new.set_ok_label("OK".into());
    let lines: Vec<NoteLine> = [
        (1, 0, "What's new"),
        (2, 0, "Video and Sound are now one Recording page, with three profiles on every page."),
        (2, 1, "Quality, Balanced and Performance."),
        (2, 0, "A Notifications page: one switch for all notifications, and one per kind."),
        (1, 0, "Fixes"),
        (2, 0, "The replay follows the game a launcher starts."),
    ]
    .into_iter()
    .map(|(kind, level, text)| NoteLine { kind, level, text: text.into() })
    .collect();
    whats_new.set_notes(slint::ModelRc::from(Rc::new(slint::VecModel::from(lines))));
    let (w, h) = (560, 520);
    window.set_size(PhysicalSize::new(w, h));
    whats_new.show().unwrap();
    let mut buffer = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    for _ in 0..2 {
        window.request_redraw();
        window.draw_if_needed(|r| {
            r.render(buffer.make_mut_slice(), w as usize);
        });
    }
    save("9-whats-new", &buffer);
    whats_new.hide().unwrap();
}
