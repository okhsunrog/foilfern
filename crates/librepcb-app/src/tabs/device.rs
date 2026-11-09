//! The device editor tab.
//!
//! Port of libs/librepcb/editor/library/dev/devicetab.{h,cpp} with
//! libs/librepcb/editor/library/dev/{devicepinoutlistmodel,
//! devicepinoutbuilder,partlistmodel,parteditor}.{h,cpp}: the component and
//! package of the device (chosen in the chooser dialogs, shown as previews),
//! the metadata with datasheet and attributes, the pinout (list, reset,
//! automatic, from a CSV file and the interactive assignment), the parts,
//! the checks with approvals and automatic fixes, undo/redo and saving (see
//! [`ElementCore`]), and the wizard of new devices (component & package,
//! metadata, pinout, parts).
//!
//! Differences to upstream: the previews are static images (no zoom, no
//! measure tool, the symbol pins are not labeled with the pad names);
//! "auto-connect" and "load from file" keep the existing connections
//! instead of asking whether to reset them first.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::{Device, Part};
use librepcb_core::library::pkg::{AssemblyType, Package};
use librepcb_core::library::sym::Symbol;
use librepcb_core::library::{LibraryBaseElement, LibraryElement, Resource};
use librepcb_core::types::{ElementName, SimpleString, Uuid};
use librepcb_core::workspace::ElementKind;
use librepcb_editor::library_editor::commands::{
    AddPart, DuplicatePart, EditDeviceAttributes, EditElementMetadata, EditPart, InteractivePinout,
    MovePartUp, RemovePart, SetDeviceComponent, SetDevicePackage, SetPadSignal,
    auto_connect_pinout, has_auto_connectable_pads, has_unconnected_pads_and_signals,
    pinout_from_csv, reset_pinout,
};
use librepcb_i18n::tr;
use librepcb_scene::{
    ColorScheme, FootprintScene, FootprintSceneObject, RenderOptions, RenderSize, SymbolScene,
    SymbolSceneObject, render_scene,
};

use super::component::{assemble_variant_symbol, load_element};
use super::editing::error_notification;
use super::element_core::{ElementCore, OpenMode};
use super::{TabId, TabRequest, TabUpdate};
use crate::dialogs::attributes::AttributeEditor;
use crate::dialogs::chooser::ChooserPurpose;
use crate::helpers::color_to_ui;
use crate::models::{UiModel, model_rc, vec_model};
use crate::validation;

/// A row of one of the tab's list models written by the UI.
#[derive(Debug, Clone)]
pub enum DeviceRowEvent {
    /// A pinout row (signal).
    Pinout(usize, ui::DevicePinoutData),
    /// A part row (MPN, manufacturer, actions); the last row is the new
    /// part.
    Part(usize, ui::PartData),
}

/// Receives the rows written by the UI (deferred to the application).
pub type DeviceRowSink = Rc<dyn Fn(DeviceRowEvent)>;

/// Upstream `cleanDescription()`: the default description without empty
/// lines.
fn clean_description(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The device editor tab (upstream `DeviceTab`).
pub struct DeviceTab {
    id: TabId,
    core: ElementCore<Device>,
    library_index: i32,
    sink: DeviceRowSink,
    datasheet_url: String,
    datasheet_url_error: String,
    attributes: Rc<RefCell<AttributeEditor>>,
    // Dependencies.
    component_selected: bool,
    component: Option<Component>,
    component_description: String,
    package_selected: bool,
    package: Option<Package>,
    package_description: String,
    symbols: HashMap<Uuid, Option<Symbol>>,
    /// Signals of the pinout combo boxes (first: unconnected).
    signal_names: Vec<(Option<Uuid>, String)>,
    // Pinout.
    pinout: Rc<UiModel<ui::DevicePinoutData>>,
    pinout_rows: Vec<Uuid>,
    interactive: InteractivePinout,
    // Parts (the last row is the new part).
    parts: Rc<UiModel<ui::PartData>>,
    part_attributes: Vec<Rc<RefCell<AttributeEditor>>>,
    new_part: Part,
    // Previews.
    component_scene: Option<SymbolScene>,
    package_scene: Option<FootprintScene>,
    previews_state: Option<u64>,
    lists_state: Option<u64>,
    font: Option<librepcb_core::font::StrokeFont>,
    frame: i32,
    pending_errors: Vec<String>,
    pending_requests: Vec<TabRequest>,
}

impl DeviceTab {
    /// Creates the tab for an element core; `sink` receives the rows of
    /// the list models written by the UI.
    pub fn new(core: ElementCore<Device>, sink: DeviceRowSink) -> Self {
        let pinout = UiModel::shared(Vec::new());
        let s = Rc::clone(&sink);
        pinout.set_handler(move |row, data| s(DeviceRowEvent::Pinout(row, data)));
        let parts = UiModel::shared(Vec::new());
        let s = Rc::clone(&sink);
        parts.set_handler(move |row, data| s(DeviceRowEvent::Part(row, data)));
        let attributes = AttributeEditor::new(core.editor.element().attributes());
        // New devices have placeholder UUIDs until chosen (upstream
        // `mComponentSelected`, `mPackageSelected`).
        let selected = core.mode != OpenMode::New;
        let mut tab = Self {
            id: TabId::new(),
            core,
            library_index: -1,
            sink,
            datasheet_url: String::new(),
            datasheet_url_error: String::new(),
            attributes,
            component_selected: selected,
            component: None,
            component_description: String::new(),
            package_selected: selected,
            package: None,
            package_description: String::new(),
            symbols: HashMap::new(),
            signal_names: Vec::new(),
            pinout,
            pinout_rows: Vec::new(),
            interactive: InteractivePinout::default(),
            parts,
            part_attributes: Vec::new(),
            new_part: Part::new(
                SimpleString::default(),
                SimpleString::default(),
                Default::default(),
            ),
            component_scene: None,
            package_scene: None,
            previews_state: None,
            lists_state: None,
            font: librepcb_scene::default_stroke_font(),
            frame: 0,
            pending_errors: Vec::new(),
            pending_requests: Vec::new(),
        };
        tab.refresh();
        tab
    }

    /// Sets the tab identifier (to connect models before the tab exists).
    pub fn with_id(mut self, id: TabId) -> Self {
        self.id = id;
        self
    }

    /// The unique tab identifier.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The shared element state.
    pub fn core(&self) -> &ElementCore<Device> {
        &self.core
    }

    /// The shared element state, mutably.
    pub fn core_mut(&mut self) -> &mut ElementCore<Device> {
        &mut self.core
    }

    /// Sets the index of the library in `Data.libraries`.
    pub fn set_library_index(&mut self, index: i32) {
        self.library_index = index;
    }

    /// The loaded component.
    pub fn component(&self) -> Option<&Component> {
        self.component.as_ref()
    }

    /// The loaded package.
    pub fn package(&self) -> Option<&Package> {
        self.package.as_ref()
    }

    fn device(&self) -> &Device {
        self.core.editor.element()
    }

    /// Updates the UI data from the device (upstream `refreshUiData()`).
    fn refresh(&mut self) {
        self.core.refresh();
        let dev = self.core.editor.element();
        self.datasheet_url = dev
            .element_metadata()
            .resources()
            .iter()
            .next()
            .map(|r| r.url().clone())
            .unwrap_or_default();
        self.datasheet_url_error.clear();
        if self.attributes.borrow().list().ok().as_ref() != Some(dev.attributes()) {
            self.attributes = AttributeEditor::new(dev.attributes());
        }
        self.refresh_dependencies();
        self.refresh_lists();
    }

    /// Loads the component and package if they changed (upstream
    /// `refreshDependentElements()`).
    fn refresh_dependencies(&mut self) {
        let db = std::sync::Arc::clone(&self.core.db);
        let (cmp_uuid, pkg_uuid) = (self.device().component_uuid(), self.device().package_uuid());
        let locale_desc = |d: &str| clean_description(d);
        if self.component_selected
            && self
                .component
                .as_ref()
                .is_none_or(|c| c.metadata().uuid() != cmp_uuid)
        {
            self.component = load_element::<Component>(&db, ElementKind::Component, cmp_uuid);
            self.component_description = match &self.component {
                Some(c) => locale_desc(c.metadata().descriptions().default_value()),
                None => tr!(
                    "librepcb::editor::DeviceTab",
                    "Component not found: {0}",
                    cmp_uuid.to_string()
                ),
            };
            self.previews_state = None;
            self.lists_state = None;
        }
        if self.package_selected
            && self
                .package
                .as_ref()
                .is_none_or(|p| p.metadata().uuid() != pkg_uuid)
        {
            self.package = load_element::<Package>(&db, ElementKind::Package, pkg_uuid);
            self.package_description = match &self.package {
                Some(p) => locale_desc(p.metadata().descriptions().default_value()),
                None => tr!(
                    "librepcb::editor::DeviceTab",
                    "Package not found: {0}",
                    pkg_uuid.to_string()
                ),
            };
            self.previews_state = None;
            self.lists_state = None;
        }
        // Symbols of the default variant.
        let uuids: Vec<Uuid> = self
            .component
            .iter()
            .flat_map(|c| c.symbol_variants().iter().take(1))
            .flat_map(|v| v.symbol_items().iter().map(|g| g.symbol_uuid()))
            .collect();
        for uuid in uuids {
            self.symbols
                .entry(uuid)
                .or_insert_with(|| load_element::<Symbol>(&db, ElementKind::Symbol, uuid));
        }
    }

    /// Updates the pinout and parts lists (after modifications).
    fn refresh_lists(&mut self) {
        let state = self.core.editor.state_id();
        if self.lists_state == Some(state) {
            return;
        }
        self.lists_state = Some(state);
        self.frame = self.frame.wrapping_add(1);
        // Signal names, sorted (upstream `ComponentSignalNameListModel`).
        let mut signals: Vec<(Option<Uuid>, String)> = self
            .component
            .iter()
            .flat_map(|c| c.signals().iter())
            .map(|s| (Some(s.uuid()), s.name().to_string()))
            .collect();
        signals.sort_by(|a, b| crate::dialogs::natural_cmp(&a.1, &b.1));
        signals.insert(
            0,
            (
                None,
                format!(
                    "({})",
                    tr!(
                        "librepcb::editor::ComponentSignalNameListModel",
                        "unconnected"
                    )
                    .to_lowercase()
                ),
            ),
        );
        self.signal_names = signals;
        // Pinout, sorted by pad name.
        let dev = self.core.editor.element();
        let pad_name = |uuid: Uuid| {
            self.package
                .as_ref()
                .and_then(|p| p.pads().by_uuid(&uuid))
                .map(|p| p.name().to_string())
        };
        let mut rows: Vec<(Uuid, String, Option<Uuid>)> = dev
            .pad_signal_map()
            .iter()
            .map(|i| {
                (
                    i.pad_uuid(),
                    pad_name(i.pad_uuid()).unwrap_or_else(|| i.pad_uuid().to_string()[..8].into()),
                    i.signal_uuid(),
                )
            })
            .collect();
        rows.sort_by(|a, b| crate::dialogs::natural_cmp(&a.1, &b.1));
        self.pinout_rows = rows.iter().map(|r| r.0).collect();
        self.pinout.replace_all(
            rows.iter()
                .map(|(_, name, signal)| ui::DevicePinoutData {
                    pad_name: name.as_str().into(),
                    signal_uuid: signal.map(|s| s.to_string()).unwrap_or_default().into(),
                    signal_index: self
                        .signal_names
                        .iter()
                        .position(|(u, _)| u == signal)
                        .map_or(-1, |i| i as i32),
                })
                .collect(),
        );
        // Parts.
        self.part_attributes = dev
            .parts()
            .iter()
            .map(|p| AttributeEditor::new(p.attributes()))
            .chain(std::iter::once(AttributeEditor::new(
                self.new_part.attributes(),
            )))
            .collect();
        let parts: Vec<ui::PartData> = dev
            .parts()
            .iter()
            .chain(std::iter::once(&self.new_part))
            .zip(&self.part_attributes)
            .map(|(p, attrs)| ui::PartData {
                mpn: p.mpn().as_str().into(),
                manufacturer: p.manufacturer().as_str().into(),
                attributes: attrs.borrow().model_rc(),
                action: ui::PartAction::None,
            })
            .collect();
        self.parts.replace_all(parts);
    }

    fn signals(&self) -> librepcb_core::library::cmp::ComponentSignalList {
        self.component
            .as_ref()
            .map(|c| c.signals().clone())
            .unwrap_or_default()
    }

    fn pads(&self) -> librepcb_core::library::pkg::PackagePadList {
        self.package
            .as_ref()
            .map(|p| p.pads().clone())
            .unwrap_or_default()
    }

    /// Executes a device command, collecting errors.
    fn execute<C>(&mut self, cmd: C) -> Option<C::Output>
    where
        C: librepcb_editor::library_editor::ElementCommand<Device>,
    {
        match self.core.editor.execute(cmd) {
            Ok(output) => Some(output),
            Err(e) => {
                self.pending_errors.push(e.to_string());
                None
            }
        }
    }

    fn finish(&mut self) -> TabUpdate {
        if self.lists_state != Some(self.core.editor.state_id()) {
            self.core.refresh();
            self.refresh_dependencies();
            self.refresh_lists();
        }
        self.core.run_checks_if_modified();
        let mut update = TabUpdate {
            data_changed: true,
            repaint: true,
            ..TabUpdate::default()
        };
        for e in std::mem::take(&mut self.pending_errors) {
            update.requests.push(error_notification(e));
        }
        update
            .requests
            .extend(std::mem::take(&mut self.pending_requests));
        update
    }

    // --- Lists ---

    /// A row of a list model was written by the UI.
    pub fn row_written(&mut self, event: DeviceRowEvent) -> TabUpdate {
        if !self.core.is_writable() {
            self.lists_state = None;
            return self.finish();
        }
        match event {
            DeviceRowEvent::Pinout(row, data) => {
                let Some(pad) = self.pinout_rows.get(row).copied() else {
                    return TabUpdate::default();
                };
                let current = self
                    .device()
                    .pad_signal_map()
                    .by_uuid(&pad)
                    .and_then(|i| i.signal_uuid());
                let signal = usize::try_from(data.signal_index)
                    .ok()
                    .and_then(|i| self.signal_names.get(i))
                    .map(|(u, _)| *u);
                match signal {
                    Some(signal) if signal != current => {
                        self.execute(SetPadSignal {
                            pad,
                            signal: Some(signal),
                            optional: None,
                        });
                    }
                    _ => self.lists_state = None,
                }
            }
            DeviceRowEvent::Part(row, data) => self.part_row_written(row, data),
        }
        self.finish()
    }

    fn part_row_written(&mut self, row: usize, data: ui::PartData) {
        let count = self.device().parts().len();
        if row < count {
            match data.action {
                ui::PartAction::MoveUp => {
                    self.execute(MovePartUp { index: row });
                }
                ui::PartAction::Duplicate => {
                    self.execute(DuplicatePart { index: row });
                }
                ui::PartAction::Delete => {
                    self.execute(RemovePart { index: row });
                }
                ui::PartAction::None => {
                    let Some(part) = self.device().parts().get(row) else {
                        return;
                    };
                    let mpn = SimpleString::clean(&data.mpn);
                    let mfr = SimpleString::clean(&data.manufacturer);
                    let cmd = EditPart {
                        index: row,
                        mpn: (data.mpn.as_str() != part.mpn().as_str() && !mpn.is_empty())
                            .then_some(mpn),
                        manufacturer: (data.manufacturer.as_str() != part.manufacturer().as_str()
                            && !mfr.is_empty())
                        .then_some(mfr),
                        attributes: None,
                    };
                    if cmd.mpn.is_some() || cmd.manufacturer.is_some() {
                        self.execute(cmd);
                    } else {
                        self.lists_state = None;
                    }
                }
            }
        } else if row == count && data.action == ui::PartAction::None {
            // The "new part" row: kept until applied.
            self.new_part.set_mpn(SimpleString::clean(&data.mpn));
            self.new_part
                .set_manufacturer(SimpleString::clean(&data.manufacturer));
            self.parts.set(row, data);
        }
    }

    /// A component or package was chosen (chooser dialog).
    pub fn element_chosen(&mut self, purpose: ChooserPurpose, uuid: Uuid) -> TabUpdate {
        let db = std::sync::Arc::clone(&self.core.db);
        match purpose {
            ChooserPurpose::DeviceComponent => {
                match load_element::<Component>(&db, ElementKind::Component, uuid) {
                    Some(cmp) => {
                        self.component_selected = true;
                        self.execute(SetDeviceComponent { component: &cmp });
                        self.component = None;
                    }
                    None => self.pending_errors.push(tr!(
                        "librepcb::editor::DeviceTab",
                        "Component not found: {0}",
                        uuid.to_string()
                    )),
                }
            }
            ChooserPurpose::DevicePackage => {
                match load_element::<Package>(&db, ElementKind::Package, uuid) {
                    Some(pkg) => {
                        self.package_selected = true;
                        self.execute(SetDevicePackage { package: &pkg });
                        self.package = None;
                    }
                    None => self.pending_errors.push(tr!(
                        "librepcb::editor::DeviceTab",
                        "Package not found: {0}",
                        uuid.to_string()
                    )),
                }
            }
            _ => {}
        }
        self.lists_state = None;
        self.refresh_dependencies();
        self.finish()
    }

    /// The content of a pinout CSV file (upstream
    /// `DevicePinoutBuilder::loadFromFile()`).
    pub fn pinout_file_chosen(&mut self, content: &str) -> TabUpdate {
        self.interactive.exit();
        let cmd = pinout_from_csv(self.device(), &self.pads(), &self.signals(), content, false);
        self.execute(cmd);
        self.finish()
    }

    /// Applies the edited properties (upstream `commitUiData()`).
    fn commit(&mut self) -> Vec<TabRequest> {
        if self.core.editor.is_group_active() {
            return Vec::new();
        }
        let mut requests: Vec<TabRequest> = self.core.commit_metadata().into_iter().collect();
        // Datasheet (the first resource).
        let dev = self.core.editor.element();
        let mut resources = dev.element_metadata().resources().clone();
        let url = self.datasheet_url.trim().to_owned();
        let name = ElementName::new(ElementName::clean(&format!(
            "Datasheet {}",
            self.core.meta.name.trim()
        )))
        .ok();
        let valid = validation::parse_user_url(&url);
        let first = resources.iter().next().cloned();
        let mut changed = false;
        match (valid, first) {
            (Some(u), None) => {
                if let Some(name) = name {
                    resources.push(Resource::new(name, "application/pdf", u.to_string()));
                    changed = true;
                }
            }
            (None, Some(_)) => {
                resources.take(0);
                changed = true;
            }
            (Some(u), Some(res)) if url != *res.url() => {
                if let Some(r) = resources.iter_mut().next() {
                    if let Some(name) = name {
                        r.set_name(name);
                    }
                    r.set_url(u.to_string());
                    changed = true;
                }
            }
            _ => {}
        }
        if changed {
            self.execute(EditElementMetadata {
                resources: Some(resources),
                ..EditElementMetadata::default()
            });
        }
        // Attributes.
        let attributes = self.attributes.borrow().list();
        match attributes {
            Ok(list) if list != *self.device().attributes() => {
                self.execute(EditDeviceAttributes { attributes: list });
            }
            Ok(_) => {}
            Err(e) => self.pending_errors.push(e),
        }
        // Parts (upstream `PartListModel::apply()`).
        let count = self.device().parts().len();
        for (index, editor) in self.part_attributes.clone().iter().enumerate() {
            let list = editor.borrow().list();
            match list {
                Ok(list) if index < count => {
                    if self
                        .device()
                        .parts()
                        .get(index)
                        .is_some_and(|p| *p.attributes() != list)
                    {
                        self.execute(EditPart {
                            index,
                            mpn: None,
                            manufacturer: None,
                            attributes: Some(list),
                        });
                    }
                }
                Ok(list) => *self.new_part.attributes_mut() = list,
                Err(e) => self.pending_errors.push(e),
            }
        }
        if !self.new_part.mpn().is_empty() && !self.new_part.manufacturer().is_empty() {
            let part = self.new_part.clone();
            if self.execute(AddPart { part }).is_some() {
                // Reset the MPN but keep the rest.
                self.new_part.set_mpn(SimpleString::default());
            }
        }
        self.lists_state = None;
        self.refresh();
        self.core.run_checks_if_modified();
        requests.extend(
            std::mem::take(&mut self.pending_errors)
                .into_iter()
                .map(error_notification),
        );
        requests
    }

    /// Upstream `trigger(Next)`.
    fn next_page(&mut self) -> Vec<TabRequest> {
        let mut requests = self.commit();
        if !self.core.wizard_mode {
            return requests;
        }
        if self.core.page_index == 0 {
            self.core.page_index = 1;
            // Initialize the metadata from the component and package.
            if self.core.mode == OpenMode::New
                && let (Some(cmp), Some(pkg)) = (&self.component, &self.package)
            {
                let cm = cmp.metadata();
                let name = ElementName::new(ElementName::clean(&format!(
                    "{} ({})",
                    cm.name(),
                    pkg.metadata().name()
                )))
                .unwrap_or_else(|_| cm.name().clone());
                let cmd = EditElementMetadata {
                    name: Some(name),
                    descriptions: Some(cm.descriptions().clone()),
                    all_keywords: Some(cm.keywords().clone()),
                    categories: Some(cmp.element_metadata().categories().clone()),
                    resources: Some(cmp.element_metadata().resources().clone()),
                    ..EditElementMetadata::default()
                };
                self.execute(cmd);
                self.refresh();
            }
        } else {
            self.core.page_index += 1;
            // Skip the pinout page if there is nothing to assign.
            if self.core.page_index == 2
                && !has_unconnected_pads_and_signals(self.device(), &self.signals())
            {
                self.core.page_index += 1;
            }
            // Skip the parts page if the package is nothing to assemble.
            if self.core.page_index == 3
                && self
                    .package
                    .as_ref()
                    .is_some_and(|p| p.resolved_assembly_type() == AssemblyType::None)
            {
                self.core.page_index += 1;
            }
            if self.core.page_index >= 4 {
                self.core.wizard_mode = false;
                self.core.page_index = 1;
                self.core.invalidate_checks();
            }
            match self.core.save() {
                Ok(()) => requests.push(TabRequest::RescanLibraries),
                Err(e) => requests.push(error_notification(e)),
            }
        }
        requests
    }

    // --- UI data ---

    /// The `TabData`.
    pub fn ui_data(&self) -> ui::TabData {
        self.core.tab_data()
    }

    /// The `DeviceTabData`.
    pub fn derived_ui_data(&self) -> ui::DeviceTabData {
        let m = &self.core.meta;
        let dev = self.device();
        let signals = self.signals();
        let pads = self.pads();
        let idle = !self.interactive.is_active();
        // The first wizard page shows the full descriptions.
        let short = |d: &str| {
            if self.core.wizard_mode && self.core.page_index == 0 {
                d.to_owned()
            } else {
                d.lines().collect::<Vec<_>>().join("; ")
            }
        };
        let sch_bg = ColorScheme::SCHEMATIC_LIGHT.color_or_transparent("schematic_background");
        let brd_bg = ColorScheme::BOARD_DARK.color_or_transparent("board_background");
        let current_pad_number = self
            .interactive
            .current_pad()
            .and_then(|(u, _)| {
                let mut sorted: Vec<(Uuid, String)> = pads
                    .iter()
                    .map(|p| (p.uuid(), p.name().to_string()))
                    .collect();
                sorted.sort_by(|a, b| crate::dialogs::natural_cmp(&a.1, &b.1));
                sorted.iter().position(|(p, _)| *p == u)
            })
            .map_or(0, |i| i as i32 + 1);
        let all_unconnected = dev
            .pad_signal_map()
            .iter()
            .all(|i| i.signal_uuid().is_none());
        ui::DeviceTabData {
            library_index: self.library_index,
            path: self.core.directory_path().as_str().into(),
            wizard_mode: self.core.wizard_mode,
            page_index: self.core.page_index,
            name: m.name.as_str().into(),
            name_error: m.name_error.as_str().into(),
            description: m.description.as_str().into(),
            keywords: m.keywords.as_str().into(),
            author: m.author.as_str().into(),
            age_days: m.age_days,
            version: m.version.as_str().into(),
            version_error: m.version_error.as_str().into(),
            deprecated: m.deprecated,
            categories: m.categories_model(),
            categories_tree: m.categories_tree(),
            choose_category: m.choose_category,
            datasheet_url: self.datasheet_url.as_str().into(),
            datasheet_url_error: self.datasheet_url_error.as_str().into(),
            attributes: self.attributes.borrow().model_rc(),
            component_error: self.component_selected && self.component.is_none(),
            component_name: self
                .component
                .as_ref()
                .map(|c| c.metadata().name().to_string())
                .unwrap_or_default()
                .into(),
            component_description: short(&self.component_description).into(),
            package_error: self.package_selected && self.package.is_none(),
            package_name: self
                .package
                .as_ref()
                .map(|p| p.metadata().name().to_string())
                .unwrap_or_default()
                .into(),
            package_description: short(&self.package_description).into(),
            signal_names: vec_model(
                self.signal_names
                    .iter()
                    .map(|(_, n)| n.as_str().into())
                    .collect(),
            ),
            pinout: model_rc(&self.pinout),
            parts: model_rc(&self.parts),
            checks: self.core.checks_data(),
            component_background_color: color_to_ui(sch_bg).into(),
            component_foreground_color: slint::Color::from_rgb_u8(0, 0, 0).into(),
            package_background_color: color_to_ui(brd_bg).into(),
            package_foreground_color: slint::Color::from_rgb_u8(0xff, 0xff, 0xff).into(),
            interface_broken_msg: self.core.is_interface_broken(),
            element_duplicated_msg: self.core.element_duplicated && !m.deprecated,
            tool_cursor: super::editing::mouse_cursor(None, false),
            has_unconnected_pads: idle && has_unconnected_pads_and_signals(dev, &signals),
            has_auto_connectable_pads: idle && has_auto_connectable_pads(dev, &pads, &signals),
            all_pads_unconnected: idle && all_unconnected,
            interactive_pinout_number: current_pad_number,
            interactive_pinout_pad: self
                .interactive
                .current_pad()
                .map(|(_, n)| n.to_owned())
                .unwrap_or_default()
                .into(),
            interactive_pinout_filter: self.interactive.filter().into(),
            interactive_pinout_signals: vec_model(
                self.interactive
                    .choices()
                    .iter()
                    .map(|c| ui::DeviceInteractivePinoutSignalData {
                        name: c.name.as_str().into(),
                        used: c.used,
                    })
                    .collect(),
            ),
            interactive_pinout_signal_index: self.interactive.current_choice() as i32,
            frame: self.frame,
            new_category: Default::default(),
        }
    }

    /// Applies data written by the UI (upstream `setDerivedUiData()`).
    pub fn set_derived_ui_data(&mut self, data: &ui::DeviceTabData) -> TabUpdate {
        let mut update = TabUpdate {
            data_changed: true,
            ..TabUpdate::default()
        };
        self.core.page_index = data.page_index;
        let category_added = self.core.meta.set_from_ui(
            &data.name,
            &data.description,
            &data.keywords,
            &data.author,
            &data.version,
            data.deprecated,
            &data.new_category,
            data.choose_category,
        );
        if category_added {
            update.requests.extend(self.core.commit_metadata());
        }
        self.datasheet_url = data.datasheet_url.to_string();
        validation::url(&self.datasheet_url, &mut self.datasheet_url_error, true);
        if data.deprecated || !data.element_duplicated_msg {
            self.core.element_duplicated = false;
        }
        // Interactive pinout.
        if self.interactive.is_active() {
            let signals = self.signals();
            let dev = self.core.editor.element();
            self.interactive
                .set_filter(dev, &signals, &data.interactive_pinout_filter);
            if data.interactive_pinout_signal_index != self.interactive.current_choice() as i32 {
                let index = (data.interactive_pinout_signal_index != -2)
                    .then_some(i64::from(data.interactive_pinout_signal_index));
                self.interactive.set_current_choice(index);
            }
            self.previews_state = None;
            update.repaint = true;
        }
        update
    }

    /// A category row was written by the UI (removal).
    pub fn category_row_written(
        &mut self,
        row: usize,
        data: &ui::LibraryElementCategoryData,
    ) -> TabUpdate {
        let mut update = TabUpdate::default();
        if self.core.meta.category_row_written(row, data) {
            update.requests.extend(self.core.commit_metadata());
            update.data_changed = true;
        }
        update
    }

    /// A check message row was written by the UI.
    pub fn check_row_written(&mut self, row: usize, data: &ui::RuleCheckMessageData) -> TabUpdate {
        use librepcb_editor::library_editor::checks::FixRequest;
        let (request, mut update) = self.core.check_row_written(row, data);
        if let Some(FixRequest::ChooseCategories) = request {
            self.core.page_index = 0;
            self.core.meta.choose_category = true;
        }
        self.refresh();
        let after = self.finish();
        update.requests.extend(after.requests);
        update
    }

    // --- Actions ---

    /// Handles a tab action (upstream `DeviceTab::trigger()`).
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        use ui::TabAction as A;
        let mut requests = Vec::new();
        match action {
            A::Next => {
                requests.extend(self.next_page());
                let mut update = self.finish();
                requests.extend(update.requests);
                update.requests = requests;
                return update;
            }
            A::Back => {
                if self.core.wizard_mode && self.core.page_index > 0 {
                    self.core.page_index -= 1;
                }
                return TabUpdate {
                    data_changed: true,
                    ..TabUpdate::default()
                };
            }
            A::Apply | A::Save | A::Undo | A::Redo | A::SaveAs => {
                requests.extend(self.commit());
            }
            _ => {}
        }
        if let Some(mut update) = self.core.trigger_common(action) {
            self.lists_state = None;
            self.refresh();
            let after = self.finish();
            requests.extend(update.requests);
            update.requests = requests;
            update.requests.extend(after.requests);
            return update;
        }
        match action {
            A::Abort => self.interactive.exit(),
            A::Accept => {
                let signals = self.signals();
                let dev = self.core.editor.element();
                if let Some(cmd) = self.interactive.commit(dev, &signals) {
                    self.execute(cmd);
                }
            }
            A::DeviceSelectComponent => {
                self.pending_requests
                    .push(TabRequest::ChooseElement(ChooserPurpose::DeviceComponent));
            }
            A::DeviceSelectPackage => {
                self.pending_requests
                    .push(TabRequest::ChooseElement(ChooserPurpose::DevicePackage));
            }
            A::DeviceOpenComponent | A::DeviceOpenPackage => {
                let (kind, tree_kind, uuid) = if action == A::DeviceOpenComponent {
                    (
                        ElementKind::Component,
                        ui::LibraryTreeViewItemType::Component,
                        self.component.as_ref().map(|c| c.metadata().uuid()),
                    )
                } else {
                    (
                        ElementKind::Package,
                        ui::LibraryTreeViewItemType::Package,
                        self.package.as_ref().map(|p| p.metadata().uuid()),
                    )
                };
                if let Some(path) = uuid.and_then(|u| self.core.db.latest(kind, u).ok().flatten())
                    && let Some(library) = path.parent_dir().and_then(|p| p.parent_dir())
                {
                    self.pending_requests.push(TabRequest::OpenLibraryElement {
                        library,
                        kind: tree_kind,
                        path,
                        duplicate: false,
                    });
                }
            }
            A::DevicePinoutReset => {
                self.interactive.exit();
                self.execute(reset_pinout());
            }
            A::DevicePinoutConnectAuto => {
                self.interactive.exit();
                let cmd = auto_connect_pinout(self.device(), &self.pads(), &self.signals(), false);
                self.execute(cmd);
            }
            A::DevicePinoutConnectInteractively => {
                let (pads, signals) = (self.pads(), self.signals());
                self.interactive = InteractivePinout::start(self.device(), &pads, &signals);
            }
            A::DevicePinoutLoadFromFile => {
                self.interactive.exit();
                self.pending_requests.push(TabRequest::ChoosePinoutFile);
            }
            A::OpenDatasheet => {
                if let Some(url) = validation::parse_user_url(&self.datasheet_url) {
                    self.pending_requests
                        .push(TabRequest::OpenUrl(url.to_string()));
                }
            }
            A::SaveAs => {
                self.pending_requests
                    .push(TabRequest::DuplicateLibraryElement);
            }
            A::ZoomFit | A::ToolMeasure => {}
            _ => return TabUpdate::default(),
        }
        self.previews_state = None;
        let mut update = self.finish();
        requests.extend(update.requests);
        update.requests = requests;
        update
    }

    // --- Previews ---

    fn update_previews(&mut self) {
        let state = self.core.editor.state_id();
        if self.previews_state == Some(state) {
            return;
        }
        self.previews_state = Some(state);
        self.component_scene = self.component.as_ref().and_then(|cmp| {
            let symbol = assemble_variant_symbol(cmp, 0, None, &self.symbols, false)?;
            Some(SymbolScene::build(
                &symbol,
                self.font.as_ref(),
                &ColorScheme::SCHEMATIC_LIGHT,
            ))
        });
        self.package_scene = self.package.as_ref().and_then(|pkg| {
            let fpt = pkg.footprints().iter().next()?;
            Some(FootprintScene::build_for_editor(
                fpt,
                self.font.as_ref(),
                &ColorScheme::BOARD_DARK,
            ))
        });
        // Highlight the pad and the signal pins of the interactive
        // assignment (upstream `updateHighlightedPadsAndPins()`).
        let pad = self.interactive.current_pad().map(|(u, _)| u);
        let signal = self
            .interactive
            .choices()
            .get(self.interactive.current_choice())
            .and_then(|c| c.signal);
        if let (Some(scene), Some(pkg)) = (&mut self.package_scene, &self.package)
            && let Some(fpt) = pkg.footprints().iter().next()
        {
            let items: Vec<_> = fpt
                .pads()
                .iter()
                .filter(|p| pad.is_some() && p.package_pad_uuid() == pad)
                .flat_map(|p| scene.items_of(FootprintSceneObject::Pad(p.uuid())))
                .collect();
            for item in items {
                scene.scene_mut().set_selected(item, true);
            }
        }
        if let (Some(scene), Some(cmp)) = (&mut self.component_scene, &self.component)
            && let Some(var) = cmp.symbol_variants().iter().next()
        {
            let pins: BTreeSet<Uuid> = var
                .symbol_items()
                .iter()
                .flat_map(|g| g.pin_signal_map().iter())
                .filter(|i| signal.is_some() && i.signal_uuid() == signal)
                .map(|i| i.pin_uuid())
                .collect();
            let items: Vec<_> = pins
                .iter()
                .flat_map(|p| scene.items_of(SymbolSceneObject::Pin(*p)))
                .collect();
            for item in items {
                scene.scene_mut().set_selected(item, true);
            }
        }
    }

    /// Renders the component (`scene` 0) or package (1) preview.
    pub fn render_scene(
        &mut self,
        width: f32,
        height: f32,
        scale: f32,
        scene: i32,
    ) -> slint::Image {
        if width < 1.0 || height < 1.0 {
            return slint::Image::default();
        }
        self.update_previews();
        let options = RenderOptions {
            size: RenderSize::Fit {
                width: (width * scale).round() as u32,
                height: (height * scale).round() as u32,
            },
            margin: 10.0 * f64::from(scale),
            background: None,
        };
        let result = match scene {
            0 => self.component_scene.as_ref().map(|s| {
                render_scene(
                    s.scene(),
                    s.content_bounds(),
                    false,
                    s.background(),
                    &options,
                )
            }),
            1 => self.package_scene.as_ref().map(|s| {
                let bg = ColorScheme::BOARD_DARK.color_or_transparent("board_background");
                render_scene(s.scene(), s.content_bounds(), false, bg, &options)
            }),
            _ => None,
        };
        match result {
            Some(Ok(image)) => crate::dialogs::add_component::to_slint_image(&image),
            _ => slint::Image::default(),
        }
    }

    /// The pinout rows (tests): pad UUIDs in the order of the `pinout`
    /// model.
    pub fn pinout_rows(&self) -> &[Uuid] {
        &self.pinout_rows
    }

    /// Whether the interactive pinout assignment is active (tests).
    pub fn is_interactive(&self) -> bool {
        self.interactive.is_active()
    }

    /// Accesses the row sink (keeps it alive with the tab).
    pub fn sink(&self) -> &DeviceRowSink {
        &self.sink
    }
}
