//! The backend of upstream's `AttributeListView` widget in dialogs.
//!
//! Port of libs/librepcb/editor/modelview/attributelistmodel.{h,cpp} and
//! `validateAttributeKey()` of libs/librepcb/editor/utils/slinthelpers.cpp.
//! Upstream edits the attribute list of the edited object through undo
//! commands; in the dialogs the rows are edited in place and converted to
//! an [`AttributeList`] when the dialog is applied ([`AttributeEditor::list()`]).
//! Like upstream, the last row is the "new attribute" row; here a new row
//! is appended as soon as a key is typed into it (upstream adds the
//! attribute on `apply()`).

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use librepcb_app_ui as ui;
use librepcb_core::attribute::{Attribute, AttributeKey, AttributeList, AttributeType};
use librepcb_i18n::tr;
use slint::{ModelRc, SharedString};

use crate::models::{UiModel, model_rc};

/// The rows of an attribute list editor.
pub struct AttributeEditor {
    model: Rc<UiModel<ui::AttributeData>>,
}

fn type_index(t: AttributeType) -> i32 {
    AttributeType::ALL
        .iter()
        .position(|x| *x == t)
        .map_or(0, |i| i as i32)
}

fn type_at(index: i32) -> Option<AttributeType> {
    usize::try_from(index)
        .ok()
        .and_then(|i| AttributeType::ALL.get(i).copied())
}

fn item(a: &Attribute) -> ui::AttributeData {
    let t = a.attribute_type();
    ui::AttributeData {
        key: a.key().as_str().into(),
        key_error: SharedString::new(),
        r#type: type_index(t),
        value: a.value().into(),
        value_valid: t.is_value_valid(a.value()),
        unit: a
            .unit()
            .and_then(|u| t.available_units().iter().position(|x| x == u))
            .map_or(-1, |i| i as i32),
        action: ui::AttributeAction::None,
    }
}

fn last_item() -> ui::AttributeData {
    ui::AttributeData {
        key: SharedString::new(),
        key_error: SharedString::new(),
        r#type: type_index(AttributeType::String),
        value: SharedString::new(),
        value_valid: true,
        unit: -1,
        action: ui::AttributeAction::None,
    }
}

/// Upstream `validateAttributeKey()`: the error message of a key input.
fn key_error(input: &str, duplicate: bool) -> SharedString {
    if duplicate {
        tr!("SlintHelpers", "Duplicate").into()
    } else if AttributeKey::new(AttributeKey::clean(input)).is_ok() {
        SharedString::new()
    } else if input.trim().is_empty() {
        tr!("SlintHelpers", "Required").into()
    } else {
        tr!("SlintHelpers", "Invalid").into()
    }
}

impl AttributeEditor {
    /// An editor for a list; the UI writes are handled by the editor
    /// itself.
    pub fn new(list: &AttributeList) -> Rc<RefCell<Self>> {
        let mut rows: Vec<ui::AttributeData> = list.iter().map(item).collect();
        rows.push(last_item());
        let this = Rc::new(RefCell::new(Self {
            model: UiModel::shared(rows),
        }));
        let weak: Weak<RefCell<Self>> = Rc::downgrade(&this);
        this.borrow().model.set_handler(move |row, data| {
            if let Some(e) = weak.upgrade() {
                e.borrow_mut().row_written(row, &data);
            }
        });
        this
    }

    /// The model for the UI.
    pub fn model_rc(&self) -> ModelRc<ui::AttributeData> {
        model_rc(&self.model)
    }

    /// The rows (including the "new attribute" row).
    pub fn rows(&self) -> Vec<ui::AttributeData> {
        self.model.to_vec()
    }

    /// Handles a row written by the UI (upstream `set_row_data()` and
    /// `trigger()`).
    pub fn row_written(&mut self, row: usize, data: &ui::AttributeData) {
        let len = self.model.len();
        if row >= len {
            return;
        }
        let is_last = row + 1 == len;
        match data.action {
            ui::AttributeAction::MoveUp if row > 0 && !is_last => {
                let mut rows = self.model.to_vec();
                rows[row].action = ui::AttributeAction::None;
                rows.swap(row, row - 1);
                self.model.replace_all(rows);
                return;
            }
            ui::AttributeAction::Delete if !is_last => {
                self.model.remove(row);
                return;
            }
            ui::AttributeAction::None => {}
            _ => {
                self.model
                    .update(row, |r| r.action = ui::AttributeAction::None);
                return;
            }
        }
        let rows = self.model.to_vec();
        let old = &rows[row];
        let key = data.key.to_string();
        let t = type_at(data.r#type);
        let type_modified = data.r#type != old.r#type;
        let mut value_modified = data.value != old.value;
        let mut unit_modified = data.unit != old.unit;
        let mut value = data.value.to_string();
        let mut unit = t.and_then(|t| {
            usize::try_from(data.unit)
                .ok()
                .and_then(|i| t.available_units().get(i))
        });
        let cleaned = AttributeKey::clean(&key);
        let duplicate = !cleaned.is_empty()
            && rows
                .iter()
                .enumerate()
                .any(|(i, r)| i != row && AttributeKey::clean(&r.key) == cleaned);
        if type_modified && let Some(t) = t {
            if !t.is_value_valid(&value) {
                value.clear();
                value_modified = true;
            }
            unit = t.default_unit();
            unit_modified = true;
        }
        let mut value_without_unit = value.trim().to_owned();
        if value_modified && let Some(t) = t {
            value = value.trim().to_owned();
            if let Some((rest, u)) = t.extract_unit_from_value(&value_without_unit) {
                let rest = rest.to_owned();
                value_without_unit = rest;
                unit = Some(u);
                unit_modified = true;
            }
        }
        let mut new = old.clone();
        new.key = data.key.clone();
        new.key_error = if is_last && key.trim().is_empty() {
            SharedString::new()
        } else {
            key_error(&key, duplicate)
        };
        if type_modified {
            new.r#type = data.r#type;
        }
        if value_modified {
            new.value = value.as_str().into();
            new.value_valid = t.is_some_and(|t| t.is_value_valid(&value_without_unit));
        }
        if unit_modified && let Some(t) = t {
            new.unit = unit
                .and_then(|u| t.available_units().iter().position(|x| x == u))
                .map_or(-1, |i| i as i32);
        }
        self.model.set(row, new);
        if is_last && !key.trim().is_empty() {
            self.model.push(last_item());
        }
    }

    /// The attribute list; `Err` with a message if a row is invalid.
    pub fn list(&self) -> Result<AttributeList, String> {
        let mut list = AttributeList::new();
        for r in self.model.to_vec() {
            if r.key.trim().is_empty() {
                continue;
            }
            let key = AttributeKey::new(AttributeKey::clean(&r.key)).map_err(|_| {
                tr!(
                    "AttributeKey",
                    "Invalid attribute key: '{0}'",
                    r.key.as_str()
                )
            })?;
            if list.contains_name(key.as_str()) {
                return Err(tr!(
                    "librepcb::editor::AttributeListModel",
                    "There is already an attribute with the name \"{0}\".",
                    key.as_str()
                ));
            }
            let t = type_at(r.r#type).unwrap_or(AttributeType::String);
            let mut value = r.value.trim().to_owned();
            if let Some((rest, _)) = t.extract_unit_from_value(&value) {
                value = rest.to_owned();
            }
            let unit = usize::try_from(r.unit)
                .ok()
                .and_then(|i| t.available_units().get(i))
                .or_else(|| t.default_unit());
            let attribute = Attribute::new(key, t, value, unit).map_err(|e| e.to_string())?;
            list.push(attribute);
        }
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_edit_delete() {
        let e = AttributeEditor::new(&AttributeList::new());
        assert_eq!(e.borrow().rows().len(), 1);
        let mut row = e.borrow().rows()[0].clone();
        row.key = "mpn".into();
        e.borrow_mut().row_written(0, &row);
        assert_eq!(e.borrow().rows().len(), 2);
        let mut row = e.borrow().rows()[0].clone();
        row.value = "ABC-1".into();
        e.borrow_mut().row_written(0, &row);
        let list = e.borrow().list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list.get(0).unwrap().key().as_str(), "MPN");
        assert_eq!(list.get(0).unwrap().value(), "ABC-1");
        // Duplicate key.
        let mut row = e.borrow().rows()[1].clone();
        row.key = "MPN".into();
        e.borrow_mut().row_written(1, &row);
        assert!(!e.borrow().rows()[1].key_error.is_empty());
        // Delete.
        let mut row = e.borrow().rows()[0].clone();
        row.action = ui::AttributeAction::Delete;
        e.borrow_mut().row_written(0, &row);
        assert_eq!(e.borrow().rows().len(), 2);
    }
}
