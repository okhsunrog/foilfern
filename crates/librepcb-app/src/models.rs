//! Slint models whose rows are also written by the UI.
//!
//! Port of the pattern behind upstream's `UiObjectList` and the custom
//! `slint::Model` implementations (`FileSystemModel`, `QuickAccessModel`,
//! `NotificationsModel`, ...): the `.slint` code writes some fields of a row
//! (`sections[i].current-tab-index = ...`, `item.action = ...`,
//! `layer.visible = ...`), and the backend reacts to it.
//!
//! [`UiModel`] stores rows written by the UI immediately (so that the UI
//! reads back what it wrote) and reports them to a handler *asynchronously*
//! (like upstream's `Qt::QueuedConnection`), so handlers never run while
//! Slint evaluates bindings or while the backend state is borrowed.
//! Backend updates go through [`UiModel::set`], [`UiModel::insert`],
//! [`UiModel::remove`] and [`UiModel::replace_all`], which never call the
//! handler.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{Model, ModelNotify, ModelRc, ModelTracker};

/// Runs `f` asynchronously on the UI thread, after the current event (the
/// equivalent of upstream's `QMetaObject::invokeMethod(..., Qt::QueuedConnection)`).
pub fn defer(f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(Duration::ZERO, f);
}

type Handler<T> = Rc<dyn Fn(usize, T)>;

/// A vector model with a handler for rows written by the UI.
pub struct UiModel<T> {
    rows: RefCell<Vec<T>>,
    notify: ModelNotify,
    handler: RefCell<Option<Handler<T>>>,
}

impl<T: Clone + 'static> Default for UiModel<T> {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl<T: Clone + 'static> UiModel<T> {
    /// Creates a model with initial rows.
    pub fn new(rows: Vec<T>) -> Self {
        Self {
            rows: RefCell::new(rows),
            notify: ModelNotify::default(),
            handler: RefCell::new(None),
        }
    }

    /// Creates a shared model.
    pub fn shared(rows: Vec<T>) -> Rc<Self> {
        Rc::new(Self::new(rows))
    }

    /// Sets the handler called (asynchronously) with the row index and the
    /// new data whenever the UI writes a row.
    pub fn set_handler(&self, handler: impl Fn(usize, T) + 'static) {
        *self.handler.borrow_mut() = Some(Rc::new(handler));
    }

    /// The row count.
    pub fn len(&self) -> usize {
        self.rows.borrow().len()
    }

    /// Whether the model is empty.
    pub fn is_empty(&self) -> bool {
        self.rows.borrow().is_empty()
    }

    /// A copy of a row.
    pub fn get(&self, row: usize) -> Option<T> {
        self.rows.borrow().get(row).cloned()
    }

    /// Replaces a row (backend side, the handler is not called).
    pub fn set(&self, row: usize, data: T) {
        let changed = match self.rows.borrow_mut().get_mut(row) {
            Some(r) => {
                *r = data;
                true
            }
            None => false,
        };
        if changed {
            self.notify.row_changed(row);
        }
    }

    /// Modifies a row in place (backend side).
    pub fn update(&self, row: usize, f: impl FnOnce(&mut T)) {
        let changed = match self.rows.borrow_mut().get_mut(row) {
            Some(r) => {
                f(r);
                true
            }
            None => false,
        };
        if changed {
            self.notify.row_changed(row);
        }
    }

    /// Inserts a row (clamped to the end).
    pub fn insert(&self, row: usize, data: T) {
        let row = row.min(self.len());
        self.rows.borrow_mut().insert(row, data);
        self.notify.row_added(row, 1);
    }

    /// Appends a row.
    pub fn push(&self, data: T) {
        self.insert(usize::MAX, data);
    }

    /// Removes a row.
    pub fn remove(&self, row: usize) -> Option<T> {
        if row >= self.len() {
            return None;
        }
        let data = self.rows.borrow_mut().remove(row);
        self.notify.row_removed(row, 1);
        Some(data)
    }

    /// Removes `count` rows starting at `row`.
    pub fn remove_range(&self, row: usize, count: usize) {
        let len = self.len();
        let end = row.saturating_add(count).min(len);
        if row < end {
            self.rows.borrow_mut().drain(row..end);
            self.notify.row_removed(row, end - row);
        }
    }

    /// Inserts several rows at `row`.
    pub fn insert_many(&self, row: usize, data: Vec<T>) {
        let row = row.min(self.len());
        let n = data.len();
        if n > 0 {
            self.rows.borrow_mut().splice(row..row, data);
            self.notify.row_added(row, n);
        }
    }

    /// Replaces all rows.
    pub fn replace_all(&self, rows: Vec<T>) {
        *self.rows.borrow_mut() = rows;
        self.notify.reset();
    }

    /// A copy of all rows.
    pub fn to_vec(&self) -> Vec<T> {
        self.rows.borrow().clone()
    }
}

impl<T: Clone + 'static> Model for UiModel<T> {
    type Data = T;

    fn row_count(&self) -> usize {
        self.len()
    }

    fn row_data(&self, row: usize) -> Option<T> {
        self.get(row)
    }

    fn set_row_data(&self, row: usize, data: T) {
        if row >= self.len() {
            return;
        }
        self.rows.borrow_mut()[row] = data.clone();
        self.notify.row_changed(row);
        if let Some(handler) = self.handler.borrow().clone() {
            defer(move || handler(row, data));
        }
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Wraps a shared model into a [`ModelRc`].
pub fn model_rc<T: Clone + 'static>(model: &Rc<UiModel<T>>) -> ModelRc<T> {
    ModelRc::from(model.clone() as Rc<dyn Model<Data = T>>)
}

/// A read-only [`ModelRc`] of a vector.
pub fn vec_model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    ModelRc::new(slint::VecModel::from(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_updates_do_not_call_handler() {
        let m = UiModel::shared(vec![1, 2, 3]);
        let calls = Rc::new(RefCell::new(0));
        let c = calls.clone();
        m.set_handler(move |_, _| *c.borrow_mut() += 1);
        m.set(0, 10);
        m.insert(1, 5);
        m.remove(3);
        m.insert_many(0, vec![7, 8]);
        m.remove_range(0, 1);
        assert_eq!(m.to_vec(), vec![8, 10, 5, 2]);
        assert_eq!(*calls.borrow(), 0);
        // UI writes are stored immediately.
        m.set_row_data(1, 42);
        assert_eq!(m.row_data(1), Some(42));
        assert_eq!(m.row_data(9), None);
    }
}
