//! The undo stack of a library element editor: port of the behavior of
//! libs/librepcb/editor/undostack.{h,cpp} as used by the library editor
//! tabs.
//!
//! Differences to upstream: instead of undo command objects, a group
//! records a snapshot of the element content before and after it (library
//! elements are small, so snapshots are cheap and exact). Undo restores the
//! snapshot before the group, redo the one after it. A group whose content
//! did not change is dropped on commit (upstream: empty groups and commands
//! without modifications are dropped). The redo history is discarded when
//! a group is committed.

use crate::error::{Error, Result};
use crate::undo_stack::HistoryEntry;

/// A committed group.
#[derive(Debug, Clone)]
struct Entry<C> {
    id: u64,
    text: String,
    before: C,
    after: C,
}

/// The active group.
#[derive(Debug, Clone)]
struct Active<C> {
    id: u64,
    text: String,
    before: C,
    /// Increased whenever the content is modified in the group (for
    /// [`ElementUndoStack::state_id()`]).
    modifications: u64,
}

/// Snapshot based undo stack over the content `C` of an element.
#[derive(Debug, Clone)]
pub struct ElementUndoStack<C> {
    entries: Vec<Entry<C>>,
    index: usize,
    clean_index: Option<usize>,
    active: Option<Active<C>>,
    next_id: u64,
}

impl<C> Default for ElementUndoStack<C> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            index: 0,
            clean_index: Some(0),
            active: None,
            next_id: 0,
        }
    }
}

impl<C: Clone + PartialEq> ElementUndoStack<C> {
    /// Creates an empty (clean) stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a group can be undone (not while a group is active).
    pub fn can_undo(&self) -> bool {
        self.active.is_none() && self.index > 0
    }

    /// Whether a group can be redone (not while a group is active).
    pub fn can_redo(&self) -> bool {
        self.active.is_none() && self.index < self.entries.len()
    }

    /// Text of the group which would be undone.
    pub fn undo_text(&self) -> Option<&str> {
        self.can_undo()
            .then(|| self.entries[self.index - 1].text.as_str())
    }

    /// Text of the group which would be redone.
    pub fn redo_text(&self) -> Option<&str> {
        self.can_redo()
            .then(|| self.entries[self.index].text.as_str())
    }

    /// Whether the current state is the clean (saved) state.
    pub fn is_clean(&self) -> bool {
        self.clean_index == Some(self.index)
    }

    /// Marks the current state as clean.
    pub fn set_clean(&mut self) {
        self.clean_index = Some(self.index);
    }

    /// Whether a group is active.
    pub fn is_group_active(&self) -> bool {
        self.active.is_some()
    }

    /// Text of the active group.
    pub fn active_group_text(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.text.as_str())
    }

    /// All groups, oldest first.
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| HistoryEntry {
                text: e.text.clone(),
                done: i < self.index,
            })
            .collect()
    }

    /// Number of done groups.
    pub fn index(&self) -> usize {
        self.index
    }

    /// An identifier of the current state (upstream `getUniqueStateId()`),
    /// see [`UndoStack::state_id()`](crate::UndoStack::state_id).
    pub fn state_id(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for entry in &self.entries[..self.index] {
            entry.id.hash(&mut hasher);
        }
        if let Some(active) = &self.active {
            active.id.hash(&mut hasher);
            active.modifications.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Starts a group; `before` is the current content.
    pub fn begin_group(&mut self, text: impl Into<String>, before: C) -> Result<()> {
        if self.active.is_some() {
            return Err(Error::GroupActive);
        }
        self.next_id += 1;
        self.active = Some(Active {
            id: self.next_id,
            text: text.into(),
            before,
            modifications: 0,
        });
        Ok(())
    }

    /// Records that the content was modified in the active group.
    pub fn touch(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.modifications += 1;
        }
    }

    /// The content at the start of the active group.
    pub fn active_before(&self) -> Option<&C> {
        self.active.as_ref().map(|a| &a.before)
    }

    /// Commits the active group with the current content `after`. Returns
    /// `false` if nothing changed and the group was dropped.
    pub fn commit_group(&mut self, after: C) -> Result<bool> {
        let active = self.active.take().ok_or(Error::NoGroupActive)?;
        if active.before == after {
            return Ok(false);
        }
        if self.clean_index.is_some_and(|i| i > self.index) {
            self.clean_index = None;
        }
        self.entries.truncate(self.index);
        self.entries.push(Entry {
            id: active.id,
            text: active.text,
            before: active.before,
            after,
        });
        self.index += 1;
        Ok(true)
    }

    /// Aborts the active group and returns the content to restore.
    pub fn abort_group(&mut self) -> Result<C> {
        Ok(self.active.take().ok_or(Error::NoGroupActive)?.before)
    }

    /// Undoes the last group and returns the content to restore.
    pub fn undo(&mut self) -> Option<C> {
        if !self.can_undo() {
            return None;
        }
        self.index -= 1;
        Some(self.entries[self.index].before.clone())
    }

    /// Redoes the next group and returns the content to restore.
    pub fn redo(&mut self) -> Option<C> {
        if !self.can_redo() {
            return None;
        }
        self.index += 1;
        Some(self.entries[self.index - 1].after.clone())
    }

    /// Removes all groups (the active one is dropped without restoring);
    /// the current state becomes the clean state.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.active = None;
        self.index = 0;
        self.clean_index = Some(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_clean() {
        let mut s = ElementUndoStack::<i32>::new();
        assert!(s.is_clean());
        s.begin_group("a", 1).unwrap();
        assert!(s.commit_group(2).unwrap());
        s.begin_group("b", 2).unwrap();
        assert!(!s.commit_group(2).unwrap()); // unchanged -> dropped
        assert_eq!(s.index(), 1);
        assert!(!s.is_clean());
        assert_eq!(s.undo(), Some(1));
        assert!(s.is_clean());
        assert_eq!(s.redo(), Some(2));
        s.set_clean();
        assert_eq!(s.undo(), Some(1));
        s.begin_group("c", 1).unwrap();
        assert!(s.commit_group(3).unwrap());
        assert!(!s.is_clean()); // clean state no longer reachable
        assert_eq!(s.history().len(), 1);
    }

    #[test]
    fn abort_returns_snapshot() {
        let mut s = ElementUndoStack::<i32>::new();
        s.begin_group("a", 5).unwrap();
        assert!(s.begin_group("b", 5).is_err());
        assert!(!s.can_undo());
        assert_eq!(s.abort_group().unwrap(), 5);
        assert!(s.abort_group().is_err());
    }
}
