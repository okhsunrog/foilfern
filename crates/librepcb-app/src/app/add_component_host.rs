//! Shows the "add component" dialog ([`crate::dialogs::add_component`],
//! `ui/dialogs/addcomponentdialog.slint`) for a schematic tab and passes
//! the choice to the tab's FSM.
//!
//! Upstream `SchematicEditorState_AddComponent` opens the modal
//! `AddComponentDialog` itself and opens it again after the placement was
//! finished with Escape if "Add more" is checked
//! (`getAutoOpenAgain()`); here the application does both.

use librepcb_app_ui as ui;
use slint::{ComponentHandle, Image, SharedString};

use super::{State, deferred};
use crate::dialogs::add_component::AddComponentDialog;
use crate::models::{model_rc, vec_model};
use crate::tabs::TabId;

/// The open "add component" dialog.
#[derive(Debug)]
pub struct OpenAddComponent {
    /// The tab which requested the dialog.
    pub tab: TabId,
    /// The dialog.
    pub dialog: AddComponentDialog,
}

/// Binds the callbacks of the `AddComponentDialogData` global.
pub fn bind(window: &ui::AppWindow, weak: &std::rc::Weak<std::cell::RefCell<State>>) {
    let d = window.global::<ui::AddComponentDialogData>();
    let w = weak.clone();
    d.on_filter_edited(move |text| {
        deferred(&w, move |s| {
            s.with_add_component(|dlg| dlg.search(text.as_str(), false));
        });
    });
    let w = weak.clone();
    d.on_category_selected(move |row| {
        deferred(&w, move |s| {
            if let Ok(row) = usize::try_from(row) {
                s.with_add_component(|dlg| dlg.select_category(row));
            }
        });
    });
    let w = weak.clone();
    d.on_component_selected(move |row| {
        deferred(&w, move |s| {
            if let Ok(row) = usize::try_from(row) {
                s.with_add_component(|dlg| dlg.select_row(row));
            }
        });
    });
    let w = weak.clone();
    d.on_symbol_variant_selected(move |index| {
        deferred(&w, move |s| {
            if let Ok(index) = usize::try_from(index) {
                s.with_add_component(|dlg| dlg.select_symbol_variant(index));
            }
        });
    });
    let w = weak.clone();
    d.on_accepted(move || deferred(&w, |s| s.close_add_component_dialog(true)));
    let w = weak.clone();
    d.on_rejected(move || deferred(&w, |s| s.close_add_component_dialog(false)));
}

impl State {
    /// Opens the "add component" dialog for a tab (replacing an open one).
    pub(crate) fn open_add_component_dialog(&mut self, tab: TabId, search_term: &str) {
        let (db, locale_order, norm_order) = {
            let ws = self.workspace.lock();
            let settings = ws.settings();
            (
                ws.shared_library_db(),
                settings.library_locale_order.get().clone(),
                settings.library_norm_order.get().clone(),
            )
        };
        let dialog = AddComponentDialog::new(db, locale_order, norm_order, search_term);
        let w = self.this.clone();
        dialog.categories().set_handler(move |row, data| {
            deferred(&w, move |s| {
                s.with_add_component(|dlg| dlg.category_row_written(row, &data));
            });
        });
        let w = self.this.clone();
        dialog.components().set_handler(move |row, data| {
            deferred(&w, move |s| {
                s.with_add_component(|dlg| dlg.component_row_written(row, &data));
            });
        });
        if let Some(w) = self.window() {
            let d = w.global::<ui::AddComponentDialogData>();
            d.set_filter(search_term.into());
            d.set_categories(model_rc(dialog.categories()));
            d.set_components(model_rc(dialog.components()));
        }
        self.add_component = Some(OpenAddComponent { tab, dialog });
        self.refresh_add_component_dialog();
        if let Some(w) = self.window() {
            w.global::<ui::AddComponentDialogData>().set_shown(true);
        }
    }

    /// The open "add component" dialog (tests).
    pub fn add_component_dialog(&mut self) -> Option<&mut AddComponentDialog> {
        self.add_component.as_mut().map(|o| &mut o.dialog)
    }

    /// Runs `f` on the open dialog and updates the UI.
    pub(crate) fn with_add_component(&mut self, f: impl FnOnce(&mut AddComponentDialog)) {
        if let Some(open) = &mut self.add_component {
            f(&mut open.dialog);
            self.refresh_add_component_dialog();
        }
    }

    fn refresh_add_component_dialog(&mut self) {
        let Some(open) = &self.add_component else {
            return;
        };
        let Some(w) = self.window() else { return };
        let d = w.global::<ui::AddComponentDialogData>();
        let v = &open.dialog.view;
        d.set_component_name(v.component_name.as_str().into());
        d.set_component_description(v.component_description.as_str().into());
        d.set_symbol_variants(vec_model(
            v.symbol_variants
                .iter()
                .map(|s| SharedString::from(s.as_str()))
                .collect(),
        ));
        d.set_symbol_variant_index(v.symbol_variant_index);
        d.set_symbol_preview(v.symbol_preview.clone().unwrap_or_default());
        d.set_device_name(v.device_name.as_str().into());
        d.set_footprint_preview(v.footprint_preview.clone().unwrap_or_else(Image::default));
        d.set_error(v.error.as_str().into());
        d.set_can_accept(open.dialog.choice().is_some());
        d.set_current_component_row(open.dialog.current_row());
    }

    /// Closes the dialog; `accept` passes the choice to the tab (and
    /// remembers to open the dialog again if "Add more" is checked).
    pub fn close_add_component_dialog(&mut self, accept: bool) {
        let add_more = self.window().is_some_and(|w| {
            let d = w.global::<ui::AddComponentDialogData>();
            d.set_shown(false);
            d.set_categories(vec_model(Vec::new()));
            d.set_components(vec_model(Vec::new()));
            d.get_add_more()
        });
        let Some(open) = self.add_component.take() else {
            return;
        };
        let choice = if accept { open.dialog.choice() } else { None };
        if accept && choice.is_none() {
            return;
        }
        self.editing.reopen_add_component = (choice.is_some() && add_more).then_some(open.tab);
        if let Some((si, ti)) = self.find_tab(open.tab) {
            let update = self.sections[si].tabs_mut()[ti].add_component(choice);
            self.apply_update(si, ti, update);
            self.after_tab_event(si, ti);
        }
    }

    /// Opens the dialog again after the placement of the chosen component
    /// was finished (upstream `getAutoOpenAgain()`), called after events of
    /// a tab.
    pub(super) fn maybe_reopen_add_component(&mut self, section: usize, tab: usize) {
        let Some(id) = self.editing.reopen_add_component else {
            return;
        };
        let Some(t) = self.sections.get(section).and_then(|s| s.tabs().get(tab)) else {
            return;
        };
        if t.id() != id || t.is_adding_component() {
            return;
        }
        self.editing.reopen_add_component = None;
        if t.is_select_tool() {
            self.open_add_component_dialog(id, "");
        }
    }
}
