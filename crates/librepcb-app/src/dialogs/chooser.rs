//! The library element chooser dialogs of the element editors (upstream
//! `SymbolChooserDialog`, `ComponentChooserDialog` and
//! `PackageChooserDialog`): a search field ("What are you looking for?")
//! and the list of matching elements of the workspace libraries with the
//! description of the selected one.
//!
//! Port of libs/librepcb/editor/library/{sym/symbolchooserdialog,
//! cmp/componentchooserdialog,pkg/packagechooserdialog}.{ui,cpp}.
//! Differences to upstream: no category tree and no graphical preview;
//! without a search term, all elements are listed.

use std::sync::Arc;

use librepcb_core::types::{LengthUnit, Uuid};
use librepcb_core::workspace::{ElementKind, LibraryDb};
use librepcb_i18n::tr;

use super::{
    Applied, DialogContext, DialogOptions, FieldEvent, Form, FormDialog, ListButtons, ListItem,
    TabDialogResult,
};

/// What the chosen element is for (answered to the tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChooserPurpose {
    /// A new symbol variant of a component with a gate of the symbol.
    AddVariant,
    /// A new gate of a symbol variant.
    AddGate {
        /// The variant.
        variant: Uuid,
    },
    /// Replace the symbol of a gate.
    GateSymbol {
        /// The variant.
        variant: Uuid,
        /// The gate.
        gate: Uuid,
    },
    /// The component of a device.
    DeviceComponent,
    /// The package of a device.
    DevicePackage,
}

impl ChooserPurpose {
    /// The kind of the elements to choose from.
    pub fn kind(self) -> ElementKind {
        match self {
            Self::AddVariant | Self::AddGate { .. } | Self::GateSymbol { .. } => {
                ElementKind::Symbol
            }
            Self::DeviceComponent => ElementKind::Component,
            Self::DevicePackage => ElementKind::Package,
        }
    }
}

const COMPONENT_CHOOSER: &str = "librepcb::editor::ComponentChooserDialog";
const PACKAGE_CHOOSER: &str = "librepcb::editor::PackageChooserDialog";
const SYMBOL_CHOOSER: &str = "librepcb::editor::SymbolChooserDialog";

/// The chooser dialog.
pub struct ElementChooserDialog {
    form: Form,
    purpose: ChooserPurpose,
    db: Arc<LibraryDb>,
    locales: Vec<String>,
    /// The listed elements (UUID, name, description).
    elements: Vec<(Uuid, String, String)>,
}

impl ElementChooserDialog {
    /// A dialog listing the elements of the purpose's kind.
    pub fn new(purpose: ChooserPurpose, db: Arc<LibraryDb>, locales: Vec<String>) -> Self {
        let ctx = match purpose.kind() {
            ElementKind::Component => COMPONENT_CHOOSER,
            ElementKind::Package => PACKAGE_CHOOSER,
            _ => SYMBOL_CHOOSER,
        };
        let mut form = Form::new(LengthUnit::Millimeters);
        form.text("filter", "", "");
        form.update("filter", |f| {
            f.placeholder = tr!(ctx, "What are you looking for?").into();
        });
        form.list("list", "", &[], &[], 14, ListButtons::default());
        form.note("description", "");
        let mut dialog = Self {
            form,
            purpose,
            db,
            locales,
            elements: Vec::new(),
        };
        dialog.search("");
        dialog
    }

    /// The listed elements (tests).
    pub fn elements(&self) -> &[(Uuid, String, String)] {
        &self.elements
    }

    /// Selects a listed element (tests).
    pub fn select(&mut self, uuid: Uuid) {
        let index = self.elements.iter().position(|e| e.0 == uuid);
        self.form.set_index("list", index);
        self.update_description();
    }

    fn search(&mut self, term: &str) {
        let kind = self.purpose.kind();
        let uuids = self.db.find(kind, term.trim()).unwrap_or_default();
        self.elements = uuids
            .into_iter()
            .filter_map(|uuid| {
                let dir = self.db.latest(kind, uuid).ok().flatten()?;
                let tr = self
                    .db
                    .translations(kind, &dir, &self.locales)
                    .ok()
                    .flatten()?;
                Some((uuid, tr.name, tr.description))
            })
            .collect();
        self.elements
            .sort_by(|a, b| super::natural_cmp(&a.1.to_lowercase(), &b.1.to_lowercase()));
        let items: Vec<ListItem> = self
            .elements
            .iter()
            .map(|(_, name, _)| ListItem::text(name.clone()))
            .collect();
        let index = (!items.is_empty()).then_some(0);
        self.form.set_items("list", &items, index);
        self.update_description();
    }

    fn update_description(&mut self) {
        let text = self
            .form
            .get_index("list")
            .and_then(|i| self.elements.get(i))
            .map(|e| e.2.clone())
            .unwrap_or_default();
        self.form.set_text("description", text);
    }
}

impl FormDialog for ElementChooserDialog {
    fn title(&self) -> String {
        match self.purpose.kind() {
            ElementKind::Component => tr!(COMPONENT_CHOOSER, "Choose Component"),
            ElementKind::Package => tr!(PACKAGE_CHOOSER, "Choose Package"),
            _ => tr!(SYMBOL_CHOOSER, "Choose Symbol"),
        }
    }

    form_accessors!();

    fn options(&self) -> DialogOptions {
        DialogOptions {
            apply: false,
            width: 500.0,
            label_width: 0.0,
            ..DialogOptions::default()
        }
    }

    fn field_event(&mut self, _ctx: &DialogContext<'_>, id: &str, _event: FieldEvent) {
        match id {
            "filter" => {
                let term = self.form.get_text("filter");
                self.search(&term);
            }
            "list" => self.update_description(),
            _ => {}
        }
    }

    fn apply(&mut self, _ctx: &DialogContext<'_>) -> Result<Applied, String> {
        let Some((uuid, _, _)) = self
            .form
            .get_index("list")
            .and_then(|i| self.elements.get(i))
        else {
            return Err(match self.purpose.kind() {
                ElementKind::Component => tr!(COMPONENT_CHOOSER, "Please select a component."),
                ElementKind::Package => tr!(PACKAGE_CHOOSER, "Please select a package."),
                _ => tr!(SYMBOL_CHOOSER, "Please select a symbol."),
            });
        };
        Ok(Applied::Tab(TabDialogResult::ElementChosen(
            self.purpose,
            *uuid,
        )))
    }
}
