use super::*;
use crate::model::Annotation;

const NO: Modifiers = Modifiers { shift: false };

fn p(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

fn session() -> Session {
    // 200×100 image: left half red, right half blue; one window on the left
    let mut base = RgbaImage::filled(200, 100, Color::rgb(255, 0, 0));
    for y in 0..100usize {
        for x in 100..200usize {
            let o = (y * 200 + x) * 4;
            base.data[o..o + 3].copy_from_slice(&[0, 0, 255]);
        }
    }
    Session::new(base, vec![Rect::new(10.0, 10.0, 80.0, 60.0)])
}

fn drag(s: &mut Session, a: Point, b: Point) {
    s.pointer_down(a, NO);
    s.pointer_move(p((a.x + b.x) / 2.0, (a.y + b.y) / 2.0), NO);
    s.pointer_up(b, NO);
}

fn ctrl(c: char) -> KeyInput {
    KeyInput {
        key: Key::Char(c),
        ctrl: true,
        shift: false,
    }
}

#[test]
fn before_any_selection_the_screen_is_calm() {
    let mut s = session();
    s.pointer_move(p(50.0, 50.0), NO);
    let v = s.view();
    assert!(v.magnifier.is_none() && v.toolbar.is_none() && v.selection.is_none());
}

#[test]
fn the_magnifier_shows_the_pixel_under_the_drag() {
    let mut s = session();
    s.pointer_down(p(20.0, 20.0), NO);
    s.pointer_move(p(50.0, 50.0), NO);
    let m = s.view().magnifier.expect("magnifier");
    assert_eq!(m.hex, "#ff0000");
    assert_eq!((m.pixels.width, m.pixels.height), (11, 11));
    s.pointer_move(p(150.0, 50.0), NO);
    assert_eq!(s.view().magnifier.unwrap().hex, "#0000ff");
}

#[test]
fn the_toolbar_and_magnifier_stay_on_one_monitor() {
    // Two 1000×800 monitors side by side; a zone ending at the right edge of the left one.
    let base = RgbaImage::filled(2000, 800, Color::rgb(0, 0, 0));
    let mut s = Session::new(base, Vec::new()).with_screens(vec![
        Rect::new(0.0, 0.0, 1000.0, 800.0),
        Rect::new(1000.0, 0.0, 1000.0, 800.0),
    ]);
    s.pointer_down(p(400.0, 100.0), NO);
    s.pointer_move(p(990.0, 300.0), NO);
    let m = s.view().magnifier.expect("magnifier");
    assert!(m.position.x + m.size <= 1000.0, "{:?}", m.position);
    s.pointer_up(p(990.0, 300.0), NO);
    let bar = s.view().toolbar.expect("toolbar");
    assert!(bar.x + TOOLBAR_SIZE.0 <= 1000.0, "{bar:?}");
}

#[test]
fn hovering_highlights_the_window_and_a_click_selects_it() {
    let mut s = session();
    s.pointer_move(p(30.0, 30.0), NO);
    assert_eq!(
        s.view().hover_window,
        Some(Rect::new(10.0, 10.0, 80.0, 60.0))
    );
    s.pointer_down(p(30.0, 30.0), NO);
    s.pointer_up(p(30.0, 30.0), NO);
    let v = s.view();
    assert_eq!(v.selection, Some(Rect::new(10.0, 10.0, 80.0, 60.0)));
    assert!(v.settled && v.toolbar.is_some() && v.magnifier.is_none());
    assert_eq!(v.size_label.unwrap().0, "80 × 60");
}

#[test]
fn drawing_a_zone_shows_its_size_while_dragging() {
    let mut s = session();
    s.pointer_down(p(20.0, 20.0), NO);
    s.pointer_move(p(70.0, 60.0), NO);
    let v = s.view();
    assert_eq!(v.size_label.unwrap().0, "50 × 40");
    assert!(!v.settled && v.toolbar.is_none());
    assert!(v.magnifier.is_some(), "the magnifier follows the drag");
}

#[test]
fn tools_draw_inside_the_zone_and_the_export_contains_them() {
    let mut s = session();
    drag(&mut s, p(20.0, 20.0), p(120.0, 80.0));
    s.choose_tool(Some(Tool::Rect));
    s.set_filled(true);
    s.set_color(Color::rgb(0, 255, 0));
    drag(&mut s, p(30.0, 30.0), p(60.0, 60.0));
    assert!(matches!(
        s.editor().doc.items[0].annotation,
        Annotation::Rect { .. }
    ));
    let out = s.export().unwrap();
    assert_eq!((out.width, out.height), (100, 60));
    assert_eq!(out.pixel(25, 25), Color::rgb(0, 255, 0)); // inside the rectangle
    assert_eq!(out.pixel(2, 2), Color::rgb(255, 0, 0)); // untouched red
    assert_eq!(out.pixel(90, 2), Color::rgb(0, 0, 255)); // untouched blue
    // the live view agrees with the export, over what the annotation covers only
    let view = s.view();
    let (annotated, at) = view.annotated.clone().unwrap();
    assert!(at.x >= 20.0 && at.y >= 20.0, "{at:?}");
    assert!(annotated.width < out.width && annotated.height < out.height);
    for y in 0..annotated.height {
        for x in 0..annotated.width {
            let (ox, oy) = (at.x as u32 - 20 + x, at.y as u32 - 20 + y);
            assert_eq!(annotated.pixel(x, y), out.pixel(ox, oy), "at {x},{y}");
        }
    }
    // nothing changed: the same drawing comes back, not a new one
    let again = s.view().annotated.unwrap().0;
    assert!(std::rc::Rc::ptr_eq(&annotated, &again));
}

#[test]
fn pressing_on_a_handle_resizes_even_with_a_tool() {
    let mut s = session();
    drag(&mut s, p(20.0, 20.0), p(120.0, 80.0));
    s.choose_tool(Some(Tool::Rect));
    s.pointer_move(p(60.0, 50.0), NO); // away from where the zone was drawn
    drag(&mut s, p(120.0, 80.0), p(150.0, 90.0)); // SE handle
    assert_eq!(s.view().selection, Some(Rect::new(20.0, 20.0, 130.0, 70.0)));
    assert!(s.editor().doc.items.is_empty());
    assert_eq!(s.tool(), Some(Tool::Rect), "the tool stays selected");
}

#[test]
fn pressing_outside_starts_a_new_zone_and_drops_the_tool() {
    let mut s = session();
    drag(&mut s, p(20.0, 20.0), p(60.0, 60.0));
    s.choose_tool(Some(Tool::Pen));
    drag(&mut s, p(100.0, 10.0), p(180.0, 90.0));
    assert_eq!(s.view().selection, Some(Rect::new(100.0, 10.0, 80.0, 80.0)));
    assert_eq!(s.tool(), None);
}

#[test]
fn without_a_tool_dragging_inside_moves_the_zone() {
    let mut s = session();
    drag(&mut s, p(20.0, 20.0), p(60.0, 60.0));
    drag(&mut s, p(40.0, 40.0), p(50.0, 45.0));
    assert_eq!(s.view().selection, Some(Rect::new(30.0, 25.0, 40.0, 40.0)));
}

#[test]
fn a_gesture_in_progress_is_part_of_the_view_but_not_the_document() {
    let mut s = session();
    drag(&mut s, p(10.0, 10.0), p(190.0, 90.0));
    s.choose_tool(Some(Tool::Line));
    s.pointer_down(p(20.0, 20.0), NO);
    s.pointer_move(p(100.0, 50.0), NO);
    assert!(s.editor().doc.items.is_empty());
    assert!(s.view().annotated.is_some());
    s.pointer_up(p(100.0, 50.0), NO);
    assert_eq!(s.editor().doc.items.len(), 1);
}

#[test]
fn text_tool_opens_a_field_then_commits() {
    let mut s = session();
    drag(&mut s, p(10.0, 10.0), p(190.0, 90.0));
    s.choose_tool(Some(Tool::Text));
    s.pointer_down(p(30.0, 30.0), NO);
    s.pointer_up(p(30.0, 30.0), NO);
    assert_eq!(s.view().text_input, Some(p(30.0, 30.0)));
    assert!(s.is_typing());
    s.commit_text("Hello");
    assert_eq!(s.view().text_input, None);
    assert!(
        matches!(&s.editor().doc.items[0].annotation, Annotation::Text { text, .. } if text == "Hello")
    );
    // Escape cancels the field instead of closing
    s.pointer_down(p(60.0, 60.0), NO);
    s.pointer_up(p(60.0, 60.0), NO);
    assert_eq!(
        s.key(KeyInput {
            key: Key::Escape,
            ctrl: false,
            shift: false
        }),
        None
    );
    assert_eq!(s.view().text_input, None);
    assert_eq!(
        s.key(KeyInput {
            key: Key::Escape,
            ctrl: false,
            shift: false
        }),
        Some(Command::Close)
    );
}

#[test]
fn keyboard_commands() {
    let mut s = session();
    assert_eq!(s.key(ctrl('c')), Some(Command::Copy));
    assert_eq!(s.key(ctrl('C')), Some(Command::Copy));
    assert_eq!(s.key(ctrl('s')), Some(Command::Save));
    assert_eq!(
        s.key(KeyInput {
            key: Key::Char('s'),
            ctrl: true,
            shift: true
        }),
        Some(Command::SaveAs)
    );
    assert_eq!(
        s.key(KeyInput {
            key: Key::Char('c'),
            ctrl: false,
            shift: false
        }),
        None
    );
}

#[test]
fn undo_redo_and_delete_from_the_keyboard() {
    let mut s = session();
    drag(&mut s, p(10.0, 10.0), p(190.0, 90.0));
    s.choose_tool(Some(Tool::Ellipse));
    drag(&mut s, p(30.0, 30.0), p(80.0, 70.0));
    assert_eq!(s.editor().doc.items.len(), 1);
    s.key(ctrl('z'));
    assert!(s.editor().doc.items.is_empty());
    s.key(ctrl('y'));
    assert_eq!(s.editor().doc.items.len(), 1);
    s.key(ctrl('z'));
    s.key(KeyInput {
        key: Key::Char('z'),
        ctrl: true,
        shift: true,
    });
    assert_eq!(s.editor().doc.items.len(), 1);
    // after a redo nothing is selected: pick the shape with the Select tool, then delete it
    s.choose_tool(Some(Tool::Select));
    s.pointer_down(p(80.0, 50.0), NO);
    s.pointer_up(p(80.0, 50.0), NO);
    s.key(KeyInput {
        key: Key::Delete,
        ctrl: false,
        shift: false,
    });
    assert!(s.editor().doc.items.is_empty());
    assert!(s.view().can_undo && !s.view().can_redo);
}

#[test]
fn select_all_takes_the_whole_image() {
    let mut s = session();
    s.key(ctrl('a'));
    assert_eq!(s.view().selection, Some(Rect::new(0.0, 0.0, 200.0, 100.0)));
    assert!(s.export().is_some());
}

#[test]
fn nothing_to_export_without_a_zone() {
    assert!(session().export().is_none());
}

#[test]
fn eyedropper_changes_the_colour() {
    let mut s = session();
    drag(&mut s, p(10.0, 10.0), p(190.0, 90.0));
    s.choose_tool(Some(Tool::Eyedropper));
    s.pointer_down(p(150.0, 50.0), NO);
    s.pointer_up(p(150.0, 50.0), NO);
    assert_eq!(s.view().color, Color::rgb(0, 0, 255));
    assert_eq!(s.view().recent_colors, [Color::rgb(0, 0, 255)]);
}

#[test]
fn cursor_shapes() {
    let mut s = session();
    s.pointer_move(p(5.0, 5.0), NO);
    assert_eq!(s.view().cursor, CursorHint::Crosshair);
    drag(&mut s, p(20.0, 20.0), p(120.0, 80.0));
    s.pointer_move(p(60.0, 50.0), NO);
    assert_eq!(s.view().cursor, CursorHint::Move);
    s.choose_tool(Some(Tool::Pen));
    assert_eq!(s.view().cursor, CursorHint::Crosshair);
    s.choose_tool(Some(Tool::Select));
    assert_eq!(s.view().cursor, CursorHint::Default);
    s.pointer_move(p(120.0, 80.0), NO);
    assert_eq!(s.view().cursor, CursorHint::ResizeNwSe);
}

#[test]
fn the_toolbar_stays_on_screen() {
    let mut s = session();
    drag(&mut s, p(0.0, 0.0), p(200.0, 100.0));
    let t = s.view().toolbar.unwrap();
    // the image is smaller than the toolbar: it is clamped to the corner
    assert_eq!(t.x, 0.0);
    assert!(t.y >= 0.0);
}

#[test]
fn thickness_scales_every_tool() {
    let mut s = session();
    s.set_width(4.0);
    let d = s.editor().settings.clone();
    assert_eq!(
        (
            d.text_size,
            d.marker_radius,
            d.blur_radius,
            d.pixelate_block
        ),
        (24.0, 14.0, 6.0, 10)
    );
    s.set_width(10.0);
    let t = &s.editor().settings;
    assert_eq!(
        (
            t.text_size,
            t.marker_radius,
            t.blur_radius,
            t.pixelate_block
        ),
        (48.0, 20.0, 15.0, 25)
    );
    s.set_width(0.0);
    assert_eq!(s.editor().settings.style.width, 1.0);
    assert_eq!(s.editor().settings.pixelate_block, 3);
    s.set_width(1000.0);
    assert_eq!(s.editor().settings.style.width, 40.0);
}

#[test]
fn the_auto_command_fires_once_when_the_first_zone_settles() {
    let mut s = session().with_auto_command(Command::Scroll);
    s.pointer_down(p(20.0, 20.0), NO);
    s.pointer_move(p(60.0, 60.0), NO);
    assert_eq!(s.pointer_up(p(60.0, 60.0), NO), Some(Command::Scroll));
    // a second gesture does not repeat it
    s.pointer_down(p(30.0, 30.0), NO);
    assert_eq!(s.pointer_up(p(35.0, 35.0), NO), None);
}

#[test]
fn a_click_on_a_window_also_settles_the_zone() {
    let mut s = session().with_auto_command(Command::Scroll);
    s.pointer_down(p(30.0, 30.0), NO);
    assert_eq!(s.pointer_up(p(30.0, 30.0), NO), Some(Command::Scroll));
    // a click on nothing selects nothing and keeps waiting
    let mut s = session().with_auto_command(Command::Scroll);
    s.pointer_down(p(150.0, 90.0), NO);
    assert_eq!(s.pointer_up(p(150.0, 90.0), NO), None);
    s.pointer_down(p(110.0, 10.0), NO);
    assert_eq!(s.pointer_up(p(180.0, 80.0), NO), Some(Command::Scroll));
}
