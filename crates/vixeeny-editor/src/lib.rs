// SPDX-License-Identifier: GPL-3.0-or-later
//! vixeeny-editor — the annotation editor's model (plan 5.3): annotations, undo/redo, tool
//! interaction and rendering to pixels. No UI, no OS: everything is testable.

pub mod geometry;
pub mod history;
pub mod model;
pub mod text;

pub use geometry::{Point, Rect};
pub use history::{Edit, History};
pub use model::{Annotation, AnnotationId, Color, Document, Item, Style};
pub mod effects;
pub mod render;
pub use render::{RgbaImage, bgra_to_rgba, render};
pub mod editor;
pub use editor::{Editor, Modifiers, Outcome, Tool, ToolSettings};
pub mod selection;
pub use selection::{CursorHint, Handle, Selection};
pub mod session;
pub use session::{Command, Key, KeyInput, MagnifierView, Session, View};
