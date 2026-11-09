//! The backend of the generic form dialogs (`ui/dialogs/formdialog.slint`).
//!
//! Not a port: upstream builds each dialog with Qt Designer (`*.ui`). Here
//! a dialog describes its content as a list of [`ui::FormField`]s (labels,
//! editors, lists) in a [`Form`]; the UI renders them with upstream's
//! widgets and writes edited rows back into the form's model (like
//! upstream's models written by the UI, see [`crate::models`]). The form
//! then applies step up/down requests of length, angle and ratio edits and
//! hands edits and list actions to the dialog ([`FieldEvent`]).

use std::collections::HashMap;
use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_core::types::{Angle, Length, LengthUnit, Ratio};
use slint::{ModelRc, SharedString};

use crate::helpers::{length_from_ui, unit_to_ui};
use crate::length_edit::{LengthEdit, steps};
use crate::models::{UiModel, model_rc, vec_model};

/// Step of the angle edits in dialogs (upstream `setSingleStep(90.0)`).
const ANGLE_STEP: i32 = 90_000_000;

/// Step of the ratio edits in dialogs (1 %).
const RATIO_STEP: i32 = 10_000;

/// A list action of the UI (`FormFieldAction`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListAction {
    /// A row was selected.
    Select(usize),
    /// A row was double-clicked.
    Activate(usize),
    /// The check state of a row was toggled.
    Toggle(usize),
    /// A new item was entered.
    Add(String),
    /// Remove a row.
    Remove(usize),
    /// Move a row up.
    MoveUp(usize),
    /// Move a row down.
    MoveDown(usize),
    /// Duplicate a row.
    Duplicate(usize),
}

/// What the UI did with a field (reported to the dialog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldEvent {
    /// The value changed (text, check state, index, length, ...).
    Edited,
    /// A button was clicked.
    Clicked,
    /// A list action.
    List(ListAction),
}

/// A list item.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListItem {
    /// Text (lists without columns).
    pub text: String,
    /// Cells (tables).
    pub cells: Vec<String>,
    /// Check state, if checkable.
    pub checked: Option<bool>,
    /// Color swatch.
    pub color: Option<slint::Color>,
    /// Whether the row is shown enabled (not grayed out).
    pub enabled: bool,
}

impl ListItem {
    /// A plain text item.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            enabled: true,
            ..Self::default()
        }
    }

    /// A checkable item.
    pub fn check(text: impl Into<String>, checked: bool) -> Self {
        Self {
            checked: Some(checked),
            ..Self::text(text)
        }
    }

    /// A table row.
    pub fn row(cells: Vec<String>) -> Self {
        Self {
            cells,
            enabled: true,
            ..Self::default()
        }
    }

    fn to_ui(&self) -> ui::FormListItem {
        ui::FormListItem {
            text: self.text.as_str().into(),
            cells: vec_model(self.cells.iter().map(|c| c.as_str().into()).collect()),
            checkable: self.checked.is_some(),
            checked: self.checked.unwrap_or(false),
            color: self.color.unwrap_or(slint::Color::from_argb_u8(0, 0, 0, 0)),
            enabled: self.enabled,
        }
    }
}

/// Which list buttons are shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListButtons {
    /// An input to add items.
    pub add: bool,
    /// A remove button.
    pub remove: bool,
    /// Move up/down buttons.
    pub move_: bool,
    /// A duplicate button.
    pub duplicate: bool,
}

/// The fields of a form dialog, shared with the UI.
pub struct Form {
    model: Rc<UiModel<ui::FormField>>,
    ids: HashMap<String, usize>,
    lengths: HashMap<String, LengthEdit>,
    pages: Vec<String>,
    page: i32,
    column: i32,
    unit: LengthUnit,
}

fn empty_field(id: &str, kind: ui::FormFieldKind, label: &str) -> ui::FormField {
    ui::FormField {
        id: id.into(),
        kind,
        label: label.into(),
        enabled: true,
        index: -1,
        action_row: -1,
        ..ui::FormField::default()
    }
}

fn strings(v: &[String]) -> ModelRc<SharedString> {
    vec_model(v.iter().map(|s| SharedString::from(s.as_str())).collect())
}

impl Form {
    /// An empty form whose lengths are shown in `unit`.
    pub fn new(unit: LengthUnit) -> Self {
        Self {
            model: UiModel::shared(Vec::new()),
            ids: HashMap::new(),
            lengths: HashMap::new(),
            pages: Vec::new(),
            page: 0,
            column: 0,
            unit,
        }
    }

    /// The model shown by the UI.
    pub fn model(&self) -> &Rc<UiModel<ui::FormField>> {
        &self.model
    }

    /// The model as [`ModelRc`].
    pub fn model_rc(&self) -> ModelRc<ui::FormField> {
        model_rc(&self.model)
    }

    /// The page titles (tabs; empty or one: no tab bar).
    pub fn pages(&self) -> &[String] {
        &self.pages
    }

    /// Whether fields are in the side column.
    pub fn has_side_column(&self) -> bool {
        self.model.to_vec().iter().any(|f| f.column == 1)
    }

    /// Starts a new page (tab); following fields are added to it.
    pub fn page(&mut self, title: impl Into<String>) {
        self.pages.push(title.into());
        self.page = (self.pages.len() - 1) as i32;
    }

    /// Adds following fields to the side column (`true`) or the main
    /// column.
    pub fn side_column(&mut self, side: bool) {
        self.column = i32::from(side);
    }

    /// Removes all fields (and pages).
    pub fn clear(&mut self) {
        self.model.replace_all(Vec::new());
        self.ids.clear();
        self.lengths.clear();
        self.pages.clear();
        self.page = 0;
        self.column = 0;
    }

    /// Removes all fields after `id` (to rebuild a dependent part).
    pub fn truncate_after(&mut self, id: &str) {
        let Some(row) = self.ids.get(id).copied() else {
            return;
        };
        let len = self.model.len();
        if row + 1 < len {
            let removed: Vec<String> = self.model.to_vec()[row + 1..]
                .iter()
                .map(|f| f.id.to_string())
                .collect();
            self.model.remove_range(row + 1, len - row - 1);
            for id in removed {
                self.ids.remove(&id);
                self.lengths.remove(&id);
            }
        }
    }

    fn push(&mut self, mut field: ui::FormField) -> &mut Self {
        field.page = self.page;
        field.column = self.column;
        if !field.id.is_empty() {
            self.ids.insert(field.id.to_string(), self.model.len());
        }
        self.model.push(field);
        self
    }

    /// A section title.
    pub fn header(&mut self, label: impl AsRef<str>) -> &mut Self {
        self.push(empty_field("", ui::FormFieldKind::Header, label.as_ref()))
    }

    /// A wrapped informational text.
    pub fn note(&mut self, id: &str, text: impl AsRef<str>) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Note, "");
        f.text = text.as_ref().into();
        self.push(f)
    }

    /// A read-only value.
    pub fn label(&mut self, id: &str, label: impl AsRef<str>, text: impl AsRef<str>) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Label, label.as_ref());
        f.text = text.as_ref().into();
        self.push(f)
    }

    /// A single line text.
    pub fn text(&mut self, id: &str, label: impl AsRef<str>, text: impl AsRef<str>) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Text, label.as_ref());
        f.text = text.as_ref().into();
        self.push(f)
    }

    /// A single line text with suggestions.
    pub fn text_with_suggestions(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        text: impl AsRef<str>,
        suggestions: &[String],
    ) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Text, label.as_ref());
        f.text = text.as_ref().into();
        f.suggestions = strings(suggestions);
        self.push(f)
    }

    /// A multi-line text.
    pub fn multiline(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        text: impl AsRef<str>,
        rows: i32,
    ) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Multiline, label.as_ref());
        f.text = text.as_ref().into();
        f.rows = rows;
        self.push(f)
    }

    /// A length edit with the generic steps.
    pub fn length(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        value: Length,
        minimum: Length,
    ) -> &mut Self {
        self.length_with_steps(id, label, value, minimum, steps::GENERIC)
    }

    /// A length edit with predefined steps.
    pub fn length_with_steps(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        value: Length,
        minimum: Length,
        steps: &'static [Length],
    ) -> &mut Self {
        let mut edit = LengthEdit::new(steps);
        edit.configure(value, minimum, steps);
        let mut f = empty_field(id, ui::FormFieldKind::Length, label.as_ref());
        f.length = edit.ui_data();
        f.length.unit = unit_to_ui(self.unit);
        self.lengths.insert(id.to_owned(), edit);
        self.push(f)
    }

    /// A length edit with an "automatic" check box (e.g. "From Design
    /// Rules").
    pub fn length_auto(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        value: Length,
        minimum: Length,
        auto: bool,
    ) -> &mut Self {
        self.length(id, label, value, minimum);
        self.update(id, |f| {
            f.auto_supported = true;
            f.auto_checked = auto;
        });
        self
    }

    /// An angle edit.
    pub fn angle(&mut self, id: &str, label: impl AsRef<str>, value: Angle) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Angle, label.as_ref());
        f.angle.value = value.to_micro_deg();
        self.push(f)
    }

    /// A ratio edit (bounds in ppm).
    pub fn ratio(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        value: Ratio,
        minimum: i32,
        maximum: i32,
    ) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Ratio, label.as_ref());
        f.ratio = ui::RatioEditData {
            value: value.to_ppm(),
            minimum,
            maximum,
            can_increase: true,
            can_decrease: true,
            increase: false,
            decrease: false,
        };
        self.push(f)
    }

    /// A check box.
    pub fn checkbox(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        text: impl AsRef<str>,
        checked: bool,
    ) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Checkbox, label.as_ref());
        f.text = text.as_ref().into();
        f.checked = checked;
        self.push(f)
    }

    /// A combo box.
    pub fn choice(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        options: &[String],
        index: Option<usize>,
    ) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Choice, label.as_ref());
        f.options = strings(options);
        f.index = index.map_or(-1, |i| i as i32);
        self.push(f)
    }

    /// Radio buttons (upstream `QRadioButton` groups), one per option.
    pub fn radio(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        options: &[String],
        index: usize,
    ) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Radio, label.as_ref());
        f.options = strings(options);
        f.index = index as i32;
        self.push(f)
    }

    /// Horizontal (`vertical == false`) or vertical alignment selector
    /// (index 0..2: left/center/right, bottom/center/top).
    pub fn alignment(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        vertical: bool,
        index: usize,
    ) -> &mut Self {
        let kind = if vertical {
            ui::FormFieldKind::Valign
        } else {
            ui::FormFieldKind::Halign
        };
        let mut f = empty_field(id, kind, label.as_ref());
        f.index = index as i32;
        self.push(f)
    }

    /// A list or table.
    pub fn list(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        columns: &[String],
        items: &[ListItem],
        rows: i32,
        buttons: ListButtons,
    ) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::List, label.as_ref());
        f.columns = strings(columns);
        f.items = vec_model(items.iter().map(ListItem::to_ui).collect());
        f.rows = rows;
        f.list_add = buttons.add;
        f.list_remove = buttons.remove;
        f.list_move = buttons.move_;
        f.list_duplicate = buttons.duplicate;
        self.push(f)
    }

    /// An attribute list editor (the model is owned by an
    /// [`AttributeEditor`](super::attributes::AttributeEditor)).
    pub fn attributes(
        &mut self,
        id: &str,
        label: impl AsRef<str>,
        model: ModelRc<ui::AttributeData>,
    ) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Attributes, label.as_ref());
        f.attributes = model;
        self.push(f)
    }

    /// A push button.
    pub fn button(&mut self, id: &str, label: impl AsRef<str>, text: impl AsRef<str>) -> &mut Self {
        let mut f = empty_field(id, ui::FormFieldKind::Button, label.as_ref());
        f.text = text.as_ref().into();
        self.push(f)
    }

    // --- Field access ---

    /// Whether the form has a field.
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains_key(id)
    }

    /// A copy of a field.
    pub fn field(&self, id: &str) -> Option<ui::FormField> {
        self.ids.get(id).and_then(|row| self.model.get(*row))
    }

    /// Modifies a field (backend side).
    pub fn update(&self, id: &str, f: impl FnOnce(&mut ui::FormField)) {
        if let Some(row) = self.ids.get(id) {
            self.model.update(*row, f);
        }
    }

    /// The text of a field.
    pub fn get_text(&self, id: &str) -> String {
        self.field(id)
            .map(|f| f.text.to_string())
            .unwrap_or_default()
    }

    /// The check state of a check box.
    pub fn get_checked(&self, id: &str) -> bool {
        self.field(id).is_some_and(|f| f.checked)
    }

    /// The current index of a combo box, alignment selector or list.
    pub fn get_index(&self, id: &str) -> Option<usize> {
        self.field(id).and_then(|f| usize::try_from(f.index).ok())
    }

    /// The value of a length edit.
    pub fn get_length(&self, id: &str) -> Length {
        self.field(id)
            .map(|f| length_from_ui(f.length.value))
            .unwrap_or_default()
    }

    /// The "automatic" check state of a length edit.
    pub fn get_auto(&self, id: &str) -> bool {
        self.field(id).is_some_and(|f| f.auto_checked)
    }

    /// The value of an angle edit.
    pub fn get_angle(&self, id: &str) -> Angle {
        self.field(id)
            .map(|f| Angle::new(f.angle.value))
            .unwrap_or_default()
    }

    /// The value of a ratio edit.
    pub fn get_ratio(&self, id: &str) -> Ratio {
        self.field(id)
            .map(|f| Ratio::new(f.ratio.value))
            .unwrap_or_default()
    }

    /// The check states of the items of a list.
    pub fn get_list_checked(&self, id: &str) -> Vec<bool> {
        use slint::Model;
        self.field(id)
            .map(|f| f.items.iter().map(|i| i.checked).collect())
            .unwrap_or_default()
    }

    /// Sets the text of a field.
    pub fn set_text(&self, id: &str, text: impl AsRef<str>) {
        self.update(id, |f| f.text = text.as_ref().into());
    }

    /// Sets the check state of a check box.
    pub fn set_checked(&self, id: &str, checked: bool) {
        self.update(id, |f| f.checked = checked);
    }

    /// Sets the index of a combo box, alignment selector or list.
    pub fn set_index(&self, id: &str, index: Option<usize>) {
        self.update(id, |f| f.index = index.map_or(-1, |i| i as i32));
    }

    /// Sets the value of a length edit.
    pub fn set_length(&mut self, id: &str, value: Length) {
        let data = self.lengths.get_mut(id).map(|e| {
            e.set_value(value);
            e.ui_data()
        });
        self.update(id, |f| {
            let unit = f.length.unit;
            match data {
                Some(d) => f.length = d,
                None => f.length.value = crate::helpers::length_to_ui(value),
            }
            f.length.unit = unit;
        });
    }

    /// Sets the "automatic" check state of a length edit.
    pub fn set_auto(&self, id: &str, auto: bool) {
        self.update(id, |f| f.auto_checked = auto);
    }

    /// Sets the value of an angle edit.
    pub fn set_angle(&self, id: &str, value: Angle) {
        self.update(id, |f| f.angle.value = value.to_micro_deg());
    }

    /// Sets the value of a ratio edit.
    pub fn set_ratio(&self, id: &str, value: Ratio) {
        self.update(id, |f| f.ratio.value = value.to_ppm());
    }

    /// Enables or disables a field.
    pub fn set_enabled(&self, id: &str, enabled: bool) {
        self.update(id, |f| f.enabled = enabled);
    }

    /// Sets the error message of a field (empty: valid).
    pub fn set_error(&self, id: &str, error: impl AsRef<str>) {
        let error = SharedString::from(error.as_ref());
        self.update(id, |f| {
            if f.error != error {
                f.error = error;
            }
        });
    }

    /// Sets the hint text of a field.
    pub fn set_hint(&self, id: &str, hint: impl AsRef<str>) {
        self.update(id, |f| f.hint = hint.as_ref().into());
    }

    /// Replaces the options of a combo box.
    pub fn set_options(&self, id: &str, options: &[String], index: Option<usize>) {
        self.update(id, |f| {
            f.options = strings(options);
            f.index = index.map_or(-1, |i| i as i32);
        });
    }

    /// Replaces the items of a list (keeps the selection if valid).
    pub fn set_items(&self, id: &str, items: &[ListItem], index: Option<usize>) {
        self.update(id, |f| {
            f.items = vec_model(items.iter().map(ListItem::to_ui).collect());
            f.index = index.filter(|i| *i < items.len()).map_or(-1, |i| i as i32);
        });
    }

    // --- UI edits ---

    /// Handles a row written by the UI: applies step requests and returns
    /// the field id and what happened (`None` for stale or unchanged
    /// rows).
    pub fn ui_written(&mut self, row: usize, data: &ui::FormField) -> Option<(String, FieldEvent)> {
        let id = data.id.to_string();
        if self.ids.get(&id) != Some(&row) {
            return None;
        }
        let event = match data.action {
            ui::FormFieldAction::None => FieldEvent::Edited,
            ui::FormFieldAction::Increase | ui::FormFieldAction::Decrease => {
                let up = data.action == ui::FormFieldAction::Increase;
                self.step(&id, data, up);
                FieldEvent::Edited
            }
            ui::FormFieldAction::Clicked => FieldEvent::Clicked,
            action => {
                let r = usize::try_from(data.action_row).unwrap_or(usize::MAX);
                FieldEvent::List(match action {
                    ui::FormFieldAction::Select => ListAction::Select(r),
                    ui::FormFieldAction::Activate => ListAction::Activate(r),
                    ui::FormFieldAction::Toggle => ListAction::Toggle(r),
                    ui::FormFieldAction::Add => ListAction::Add(data.action_text.to_string()),
                    ui::FormFieldAction::Remove => ListAction::Remove(r),
                    ui::FormFieldAction::MoveUp => ListAction::MoveUp(r),
                    ui::FormFieldAction::MoveDown => ListAction::MoveDown(r),
                    _ => ListAction::Duplicate(r),
                })
            }
        };
        if data.action != ui::FormFieldAction::None {
            self.update(&id, |f| {
                f.action = ui::FormFieldAction::None;
                f.action_row = -1;
            });
        } else if data.kind == ui::FormFieldKind::Length {
            // Keep the step state in sync with the edited value.
            let value = length_from_ui(data.length.value.clone());
            if let Some(e) = self.lengths.get_mut(&id) {
                e.set_value(value);
                let d = e.ui_data();
                self.update(&id, |f| {
                    f.length.can_increase = d.can_increase;
                    f.length.can_decrease = d.can_decrease;
                });
            }
        }
        if let FieldEvent::List(ListAction::Select(r)) = &event {
            self.update(&id, |f| f.index = *r as i32);
        }
        Some((id, event))
    }

    fn step(&mut self, id: &str, data: &ui::FormField, up: bool) {
        match data.kind {
            ui::FormFieldKind::Length => {
                if let Some(e) = self.lengths.get_mut(id) {
                    let mut d = data.length.clone();
                    d.increase = up;
                    d.decrease = !up;
                    e.set_ui_data(&d);
                    let mut new = e.ui_data();
                    new.unit = data.length.unit;
                    self.update(id, |f| f.length = new);
                }
            }
            ui::FormFieldKind::Angle => {
                let step = if up { ANGLE_STEP } else { -ANGLE_STEP };
                let value = (Angle::new(data.angle.value) + Angle::new(step)).mapped_to_0_360deg();
                self.update(id, |f| f.angle.value = value.to_micro_deg());
            }
            ui::FormFieldKind::Ratio => {
                let step = if up { RATIO_STEP } else { -RATIO_STEP };
                let value = data
                    .ratio
                    .value
                    .saturating_add(step)
                    .clamp(data.ratio.minimum, data.ratio.maximum);
                self.update(id, |f| f.ratio.value = value);
            }
            _ => {}
        }
    }

    /// Simulates an edit by the UI (tests): modifies the field and handles
    /// it like a row written by the UI.
    pub fn edit(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut ui::FormField),
    ) -> Option<(String, FieldEvent)> {
        let row = *self.ids.get(id)?;
        let mut data = self.model.get(row)?;
        f(&mut data);
        self.model.set(row, data.clone());
        self.ui_written(row, &data)
    }
}

impl std::fmt::Debug for Form {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Form")
            .field("fields", &self.ids.len())
            .field("pages", &self.pages)
            .finish()
    }
}
