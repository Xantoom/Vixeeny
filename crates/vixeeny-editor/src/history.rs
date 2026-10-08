// SPDX-License-Identifier: GPL-3.0-or-later
//! Unlimited undo/redo as invertible edits.

use crate::model::{Annotation, AnnotationId, Document, Item};

#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Add {
        index: usize,
        item: Item,
    },
    Remove {
        index: usize,
        item: Item,
    },
    Replace {
        id: AnnotationId,
        old: Annotation,
        new: Annotation,
    },
}

impl Edit {
    fn inverse(&self) -> Self {
        match self {
            Self::Add { index, item } => Self::Remove {
                index: *index,
                item: item.clone(),
            },
            Self::Remove { index, item } => Self::Add {
                index: *index,
                item: item.clone(),
            },
            Self::Replace { id, old, new } => Self::Replace {
                id: *id,
                old: new.clone(),
                new: old.clone(),
            },
        }
    }

    fn apply(&self, doc: &mut Document) {
        match self {
            Self::Add { index, item } => doc
                .items
                .insert((*index).min(doc.items.len()), item.clone()),
            Self::Remove { item, .. } => doc.items.retain(|i| i.id != item.id),
            Self::Replace { id, new, .. } => {
                if let Some(i) = doc.items.iter_mut().find(|i| i.id == *id) {
                    i.annotation = new.clone();
                }
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl History {
    /// Applies a new edit; the redo branch is dropped.
    pub fn commit(&mut self, doc: &mut Document, edit: Edit) {
        edit.apply(doc);
        self.undo.push(edit);
        self.redo.clear();
    }

    /// Returns the edit undone, if any.
    pub fn undo(&mut self, doc: &mut Document) -> Option<Edit> {
        let edit = self.undo.pop()?;
        edit.inverse().apply(doc);
        self.redo.push(edit.clone());
        Some(edit)
    }

    pub fn redo(&mut self, doc: &mut Document) -> Option<Edit> {
        let edit = self.redo.pop()?;
        edit.apply(doc);
        self.undo.push(edit.clone());
        Some(edit)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Point;
    use crate::model::{Color, Style};

    fn line(x: f32) -> Annotation {
        Annotation::Line {
            from: Point::new(0.0, 0.0),
            to: Point::new(x, 0.0),
            style: Style {
                color: Color::PALETTE[0],
                width: 1.0,
            },
        }
    }

    fn add(doc: &mut Document, h: &mut History, a: Annotation) -> AnnotationId {
        let id = doc.fresh_id();
        let index = doc.items.len();
        h.commit(
            doc,
            Edit::Add {
                index,
                item: Item { id, annotation: a },
            },
        );
        id
    }

    #[test]
    fn undo_redo_round_trip_for_every_edit() {
        let mut doc = Document::new(100, 100);
        let mut h = History::default();
        let before = doc.clone();
        let a = add(&mut doc, &mut h, line(1.0));
        let b = add(&mut doc, &mut h, line(2.0));
        h.commit(
            &mut doc,
            Edit::Replace {
                id: a,
                old: line(1.0),
                new: line(9.0),
            },
        );
        let item_b = doc.items.iter().find(|i| i.id == b).cloned().unwrap();
        h.commit(
            &mut doc,
            Edit::Remove {
                index: 1,
                item: item_b,
            },
        );
        let after = doc.clone();
        assert_eq!(doc.items.len(), 1);

        while h.undo(&mut doc).is_some() {}
        assert_eq!(doc.items, before.items);
        while h.redo(&mut doc).is_some() {}
        assert_eq!(doc.items, after.items);
    }

    #[test]
    fn removal_restores_the_original_position() {
        let mut doc = Document::new(10, 10);
        let mut h = History::default();
        let a = add(&mut doc, &mut h, line(1.0));
        let _b = add(&mut doc, &mut h, line(2.0));
        let item = doc.items[0].clone();
        h.commit(&mut doc, Edit::Remove { index: 0, item });
        assert_eq!(doc.items.len(), 1);
        h.undo(&mut doc);
        assert_eq!(doc.items[0].id, a);
    }

    #[test]
    fn a_new_edit_clears_the_redo_branch() {
        let mut doc = Document::new(10, 10);
        let mut h = History::default();
        add(&mut doc, &mut h, line(1.0));
        h.undo(&mut doc);
        assert!(h.can_redo());
        add(&mut doc, &mut h, line(2.0));
        assert!(!h.can_redo());
        assert!(h.can_undo());
    }

    #[test]
    fn history_is_unbounded() {
        let mut doc = Document::new(10, 10);
        let mut h = History::default();
        for i in 0..5000 {
            add(&mut doc, &mut h, line(i as f32));
        }
        let mut n = 0;
        while h.undo(&mut doc).is_some() {
            n += 1;
        }
        assert_eq!((n, doc.items.len()), (5000, 0));
    }

    #[test]
    fn undo_on_an_empty_history_does_nothing() {
        let mut doc = Document::new(10, 10);
        let mut h = History::default();
        assert!(h.undo(&mut doc).is_none());
        assert!(h.redo(&mut doc).is_none());
    }
}
