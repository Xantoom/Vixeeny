// SPDX-License-Identifier: GPL-3.0-or-later
//! Binds an [`EditorWindow`] to an editor [`Session`]: forwards input, pushes the view back.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::Key as SlintKey;
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use vixeeny_editor::selection::CursorHint;
use vixeeny_editor::{Color, Command, Key, KeyInput, Modifiers, Point, RgbaImage, Session, Tool};

use crate::EditorWindow;

/// Toolbar button index → tool. Index 12 is "crop", i.e. adjusting the zone itself.
pub const TOOLS: [Option<Tool>; 13] = [
    Some(Tool::Select),
    Some(Tool::Pen),
    Some(Tool::Line),
    Some(Tool::Arrow),
    Some(Tool::Rect),
    Some(Tool::Ellipse),
    Some(Tool::Text),
    Some(Tool::Highlighter),
    Some(Tool::Blur),
    Some(Tool::Pixelate),
    Some(Tool::Marker),
    Some(Tool::Eyedropper),
    None,
];

pub fn tool_index(tool: Option<Tool>) -> i32 {
    TOOLS
        .iter()
        .position(|t| *t == tool)
        .map_or(-1, |i| i as i32)
}

fn slint_color(c: Color) -> slint::Color {
    slint::Color::from_argb_u8(c.a, c.r, c.g, c.b)
}

fn editor_color(c: slint::Color) -> Color {
    Color {
        r: c.red(),
        g: c.green(),
        b: c.blue(),
        a: c.alpha(),
    }
}

fn slint_image(img: &RgbaImage) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &img.data, img.width, img.height,
    ))
}

fn cursor_kind(c: CursorHint) -> i32 {
    match c {
        CursorHint::Crosshair => 0,
        CursorHint::Move => 1,
        CursorHint::ResizeNs => 2,
        CursorHint::ResizeEw => 3,
        CursorHint::ResizeNeSw => 4,
        CursorHint::ResizeNwSe => 5,
        CursorHint::Default => 6,
    }
}

/// Maps the text of a Slint key event to an editor key.
pub fn map_key(text: &str, ctrl: bool, shift: bool) -> Option<KeyInput> {
    let c = text.chars().next()?;
    let key = if c == char::from(SlintKey::Escape) {
        Key::Escape
    } else if c == char::from(SlintKey::Delete) || c == char::from(SlintKey::Backspace) {
        Key::Delete
    } else if c == char::from(SlintKey::Return) || c == '\n' {
        Key::Enter
    } else if c == char::from(SlintKey::LeftArrow) {
        Key::Left
    } else if c == char::from(SlintKey::RightArrow) {
        Key::Right
    } else if c == char::from(SlintKey::UpArrow) {
        Key::Up
    } else if c == char::from(SlintKey::DownArrow) {
        Key::Down
    } else {
        // With Ctrl held, some platforms deliver control characters (Ctrl+C = U+0003).
        let c = if ctrl && (c as u32) < 32 && c != '\u{1b}' {
            char::from_u32(c as u32 + 96).unwrap_or(c)
        } else {
            c
        };
        Key::Char(c)
    };
    Some(KeyInput { key, ctrl, shift })
}

type CommandHandler = dyn Fn(Command, &Session);

pub struct Overlay {
    window: EditorWindow,
    session: Rc<RefCell<Session>>,
}

impl Overlay {
    /// `on_command` is called when the user asks to copy, save, run OCR or close; the host does
    /// the work (it has the clipboard and the file system) and closes the window.
    pub fn new(
        mut session: Session,
        ui_scale: f32,
        on_command: impl Fn(Command, &Session) + 'static,
    ) -> Result<Self, slint::PlatformError> {
        session.set_ui_scale(ui_scale);
        let window = EditorWindow::new()?;
        let base = session.base().clone();
        window.set_frozen(slint_image(&base));
        window.set_image_width(i32::try_from(base.width).unwrap_or(i32::MAX));
        window.set_image_height(i32::try_from(base.height).unwrap_or(i32::MAX));
        window.set_ui_scale(ui_scale);
        let palette: Vec<slint::Color> = Color::PALETTE.iter().map(|c| slint_color(*c)).collect();
        window.set_palette(ModelRc::from(Rc::new(VecModel::from(palette))));
        let overlay = Self {
            window,
            session: Rc::new(RefCell::new(session)),
        };
        overlay.wire(Rc::new(on_command));
        overlay.refresh();
        Ok(overlay)
    }

    pub fn window(&self) -> &EditorWindow {
        &self.window
    }

    pub fn session(&self) -> Rc<RefCell<Session>> {
        self.session.clone()
    }

    fn wire(&self, on_command: Rc<CommandHandler>) {
        let shift = Rc::new(Cell::new(false));
        let w = &self.window;

        let (session, weak) = (self.session.clone(), w.as_weak());
        let sh = shift.clone();
        w.on_pointer_pressed(move |x, y, s| {
            sh.set(s);
            session
                .borrow_mut()
                .pointer_down(Point::new(x, y), Modifiers { shift: s });
            refresh_from(&weak, &session);
        });
        let (session, weak) = (self.session.clone(), w.as_weak());
        let sh = shift.clone();
        w.on_pointer_moved(move |x, y, _| {
            session
                .borrow_mut()
                .pointer_move(Point::new(x, y), Modifiers { shift: sh.get() });
            refresh_from(&weak, &session);
        });
        let (session, weak) = (self.session.clone(), w.as_weak());
        let sh = shift.clone();
        w.on_pointer_released(move |x, y, s| {
            sh.set(s);
            session
                .borrow_mut()
                .pointer_up(Point::new(x, y), Modifiers { shift: s });
            refresh_from(&weak, &session);
        });

        let sh = shift.clone();
        w.on_key_released(move |text| {
            if text.starts_with(char::from(SlintKey::Shift)) {
                sh.set(false);
            }
        });
        let (session, weak, handler) = (self.session.clone(), w.as_weak(), on_command.clone());
        let sh = shift;
        w.on_key_pressed(move |text, ctrl, shift_held| {
            if text.starts_with(char::from(SlintKey::Shift)) {
                sh.set(true);
            }
            let Some(input) = map_key(&text, ctrl, shift_held) else {
                return;
            };
            // While the text field has the focus the field handles typing itself.
            if session.borrow().is_typing() && !matches!(input.key, Key::Escape) {
                return;
            }
            let command = session.borrow_mut().key(input);
            if let Some(command) = command {
                handler(command, &session.borrow());
            }
            refresh_from(&weak, &session);
        });

        let (session, weak) = (self.session.clone(), w.as_weak());
        w.on_tool_chosen(move |i| {
            let tool = usize::try_from(i)
                .ok()
                .and_then(|i| TOOLS.get(i).copied())
                .flatten();
            session.borrow_mut().choose_tool(tool);
            refresh_from(&weak, &session);
        });
        let (session, weak) = (self.session.clone(), w.as_weak());
        w.on_color_chosen(move |c| {
            session.borrow_mut().set_color(editor_color(c));
            refresh_from(&weak, &session);
        });
        let (session, weak) = (self.session.clone(), w.as_weak());
        w.on_width_changed(move |v| {
            session.borrow_mut().set_width(v);
            refresh_from(&weak, &session);
        });
        let (session, weak) = (self.session.clone(), w.as_weak());
        w.on_filled_toggled(move || {
            let filled = session.borrow().view().filled;
            session.borrow_mut().set_filled(!filled);
            refresh_from(&weak, &session);
        });
        let (session, weak) = (self.session.clone(), w.as_weak());
        w.on_text_committed(move |text| {
            session.borrow_mut().commit_text(&text);
            refresh_from(&weak, &session);
        });
        let (session, weak, handler) = (self.session.clone(), w.as_weak(), on_command);
        w.on_action(move |name| {
            match name.as_str() {
                "undo" => session.borrow_mut().undo(),
                "redo" => session.borrow_mut().redo(),
                other => {
                    let command = match other {
                        "copy" => Command::Copy,
                        "save" => Command::Save,
                        "save-as" => Command::SaveAs,
                        "ocr" => Command::Ocr,
                        _ => Command::Close,
                    };
                    handler(command, &session.borrow());
                }
            }
            refresh_from(&weak, &session);
        });
    }

    /// Pushes the session's view into the window.
    pub fn refresh(&self) {
        refresh_from(&self.window.as_weak(), &self.session);
    }
}

fn refresh_from(weak: &slint::Weak<EditorWindow>, session: &Rc<RefCell<Session>>) {
    let Some(w) = weak.upgrade() else { return };
    let v = session.borrow().view();
    w.set_dim(v.dim);
    w.set_has_selection(v.selection.is_some());
    w.set_settled(v.settled);
    if let Some(r) = v.selection {
        w.set_sel_x(r.x);
        w.set_sel_y(r.y);
        w.set_sel_w(r.w);
        w.set_sel_h(r.h);
    }
    w.set_has_hover(v.hover_window.is_some());
    if let Some(r) = v.hover_window {
        w.set_hover_x(r.x);
        w.set_hover_y(r.y);
        w.set_hover_w(r.w);
        w.set_hover_h(r.h);
    }
    if let Some((text, at)) = &v.size_label {
        w.set_size_text(text.as_str().into());
        w.set_size_x(at.x);
        w.set_size_y(at.y);
    }
    w.set_has_annotated(v.annotated.is_some());
    if let Some((img, at)) = &v.annotated {
        w.set_annotated(slint_image(img));
        w.set_annotated_x(at.x);
        w.set_annotated_y(at.y);
    }
    w.set_has_magnifier(v.magnifier.is_some());
    if let Some(m) = &v.magnifier {
        w.set_magnifier(slint_image(&m.pixels));
        w.set_mag_x(m.position.x);
        w.set_mag_y(m.position.y);
        w.set_mag_size(m.size);
        w.set_mag_hex(m.hex.as_str().into());
        w.set_mag_color(slint_color(m.color));
    }
    w.set_has_toolbar(v.toolbar.is_some());
    if let Some(at) = v.toolbar {
        w.set_toolbar_x(at.x);
        w.set_toolbar_y(at.y);
    }
    w.set_tool(tool_index(v.tool));
    w.set_current_color(slint_color(v.color));
    let recent: Vec<slint::Color> = v.recent_colors.iter().map(|c| slint_color(*c)).collect();
    // Only touch the model when it changed: replacing it every pointer move would rebuild the row.
    let same = {
        let old = w.get_recent();
        old.row_count() == recent.len()
            && recent
                .iter()
                .enumerate()
                .all(|(i, c)| old.row_data(i) == Some(*c))
    };
    if !same {
        w.set_recent(ModelRc::from(Rc::new(VecModel::from(recent))));
    }
    w.set_stroke_width(v.width);
    w.set_filled(v.filled);
    w.set_can_undo(v.can_undo);
    w.set_can_redo(v.can_redo);
    w.set_has_text_input(v.text_input.is_some());
    if let Some(at) = v.text_input {
        w.set_text_x(at.x);
        w.set_text_y(at.y);
    }
    w.set_cursor_kind(cursor_kind(v.cursor));
}
