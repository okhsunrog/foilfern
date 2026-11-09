//! Library element tabs on the application side (symbol, package,
//! component, device, category and organization editors, milestone M4b):
//! opening elements from the library tab, new and duplicated elements,
//! the requests of the element tabs and closing the tabs of removed
//! elements.
//!
//! Port of the element tab parts of libs/librepcb/editor/mainwindow.cpp
//! (`openSymbolTab()`, `openNewSymbolTab()`, ...).

use std::rc::Rc;

use librepcb_app_ui as ui;
use librepcb_core::fileio::FilePath;
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_i18n::tr;
use slint::ComponentHandle;

use super::{State, deferred};
use crate::open_library::OpenLibrary;
use crate::tabs::base_element_core::{BaseElementCore, BaseElementKind};
use crate::tabs::element_core::{ElementCore, ElementKindInfo, OpenMode};
use crate::tabs::{
    CategoryTab, ComponentRowEvent, ComponentTab, DeviceRowEvent, DeviceTab, LibraryItemRef,
    OrganizationTab, PackageRowEvent, PackageTab, SymbolTab, Tab, TabId, TabUpdate,
};

impl State {
    /// Opens (or duplicates) a library element in its editor tab.
    pub(crate) fn open_library_element_tab(
        &mut self,
        library: &FilePath,
        kind: ui::LibraryTreeViewItemType,
        path: &FilePath,
        duplicate: bool,
    ) {
        if !duplicate
            && let Some((sec, index)) =
                self.find_tab_where(|t| t.directory_path().as_ref() == Some(path))
        {
            self.set_current_tab(sec, index as i32, true);
            return;
        }
        let Some(lib) = self.open_library(library) else {
            return;
        };
        let Some(relative) = lib.relative_path(path) else {
            log::warn!(
                "Element {} not in library {}",
                path.to_native(),
                library.to_native()
            );
            return;
        };
        let mode = if duplicate {
            OpenMode::Duplicate
        } else {
            OpenMode::Open
        };
        self.open_element_tab(&lib, kind, Some(&relative), mode);
    }

    /// Opens the tab of a new library element (upstream
    /// `openNewSymbolTab()` etc.).
    pub(crate) fn open_new_library_element_tab(
        &mut self,
        library: &FilePath,
        action: ui::LibraryAction,
    ) {
        let kind = match action {
            ui::LibraryAction::NewSymbol => ui::LibraryTreeViewItemType::Symbol,
            ui::LibraryAction::NewPackage => ui::LibraryTreeViewItemType::Package,
            ui::LibraryAction::NewComponent => ui::LibraryTreeViewItemType::Component,
            ui::LibraryAction::NewDevice => ui::LibraryTreeViewItemType::Device,
            ui::LibraryAction::NewComponentCategory => {
                ui::LibraryTreeViewItemType::ComponentCategory
            }
            ui::LibraryAction::NewPackageCategory => ui::LibraryTreeViewItemType::PackageCategory,
            ui::LibraryAction::NewOrganization => ui::LibraryTreeViewItemType::Organization,
            _ => return,
        };
        let Some(lib) = self.open_library(library) else {
            return;
        };
        self.open_element_tab(&lib, kind, None, OpenMode::New);
    }

    /// Creates the element core of a tab with its model handlers.
    fn element_core<E: ElementKindInfo>(
        &mut self,
        id: TabId,
        lib: &Rc<OpenLibrary>,
        relative: Option<&str>,
        mode: OpenMode,
    ) -> Option<ElementCore<E>> {
        let (db, locales, user) = {
            let ws = self.workspace.lock();
            (
                ws.shared_library_db(),
                ws.settings().library_locale_order.get().clone(),
                ws.settings().user_name.get().clone(),
            )
        };
        let w = self.this.clone();
        let on_check = move |row, data| {
            deferred(&w, move |s| {
                s.element_tab_event(id, |t| t.element_check_row(row, &data));
            });
        };
        let w = self.this.clone();
        let on_category = move |row, data| {
            deferred(&w, move |s| {
                s.element_tab_event(id, |t| t.element_category_row(row, &data));
            });
        };
        match ElementCore::<E>::open(
            Rc::clone(lib),
            relative,
            mode,
            db,
            locales,
            user,
            on_check,
            on_category,
        ) {
            Ok(core) => Some(core),
            Err(e) => {
                self.notifications
                    .borrow_mut()
                    .push(crate::notifications::Notification {
                        auto_popup: true,
                        ..crate::notifications::Notification::new(
                            ui::NotificationType::Critical,
                            tr!("librepcb::editor::MainWindow", "Error"),
                            e,
                        )
                    });
                None
            }
        }
    }

    /// Creates the core of a category or organization tab.
    fn base_core<E: BaseElementKind>(
        &mut self,
        id: TabId,
        lib: &Rc<OpenLibrary>,
        relative: Option<&str>,
        mode: OpenMode,
    ) -> Option<BaseElementCore<E>> {
        let (db, locales, user) = {
            let ws = self.workspace.lock();
            (
                ws.shared_library_db(),
                ws.settings().library_locale_order.get().clone(),
                ws.settings().user_name.get().clone(),
            )
        };
        let w = self.this.clone();
        let on_check = move |row, data| {
            deferred(&w, move |s| {
                s.element_tab_event(id, |t| t.element_check_row(row, &data));
            });
        };
        match BaseElementCore::<E>::open(
            Rc::clone(lib),
            relative,
            mode,
            db,
            locales,
            user,
            on_check,
        ) {
            Ok(core) => Some(core),
            Err(e) => {
                self.notifications
                    .borrow_mut()
                    .push(crate::notifications::Notification {
                        auto_popup: true,
                        ..crate::notifications::Notification::new(
                            ui::NotificationType::Critical,
                            tr!("librepcb::editor::MainWindow", "Error"),
                            e,
                        )
                    });
                None
            }
        }
    }

    /// Opens an element tab of a kind.
    fn open_element_tab(
        &mut self,
        lib: &Rc<OpenLibrary>,
        kind: ui::LibraryTreeViewItemType,
        relative: Option<&str>,
        mode: OpenMode,
    ) {
        let id = TabId::new();
        let library_index = self
            .libraries
            .iter()
            .position(|l| Rc::ptr_eq(l, lib))
            .map_or(-1, |i| i as i32);
        let mut tab = match kind {
            ui::LibraryTreeViewItemType::Symbol => {
                let Some(core) = self.element_core::<Symbol>(id, lib, relative, mode) else {
                    return;
                };
                let style = *self.workspace.lock().settings().schematic_grid_style.get();
                Tab::Symbol(Box::new(SymbolTab::new(core, style).with_id(id)))
            }
            ui::LibraryTreeViewItemType::Package => {
                let Some(core) = self.element_core::<Package>(id, lib, relative, mode) else {
                    return;
                };
                let style = *self.workspace.lock().settings().board_grid_style.get();
                let w = self.this.clone();
                let sink = Rc::new(move |event: PackageRowEvent| {
                    deferred(&w, move |s| {
                        s.element_tab_event(id, |t| t.package_row_written(event));
                    });
                });
                Tab::Package(Box::new(PackageTab::new(core, style, sink).with_id(id)))
            }
            ui::LibraryTreeViewItemType::Component => {
                let Some(core) = self.element_core::<Component>(id, lib, relative, mode) else {
                    return;
                };
                let w = self.this.clone();
                let sink = Rc::new(move |event: ComponentRowEvent| {
                    deferred(&w, move |s| {
                        s.element_tab_event(id, |t| t.component_row_written(event));
                    });
                });
                Tab::Component(Box::new(ComponentTab::new(core, sink).with_id(id)))
            }
            ui::LibraryTreeViewItemType::ComponentCategory => {
                let Some(core) = self.base_core(id, lib, relative, mode) else {
                    return;
                };
                Tab::ComponentCategory(Box::new(CategoryTab::new(core).with_id(id)))
            }
            ui::LibraryTreeViewItemType::PackageCategory => {
                let Some(core) = self.base_core(id, lib, relative, mode) else {
                    return;
                };
                Tab::PackageCategory(Box::new(CategoryTab::new(core).with_id(id)))
            }
            ui::LibraryTreeViewItemType::Organization => {
                let Some(core) = self.base_core(id, lib, relative, mode) else {
                    return;
                };
                let w = self.this.clone();
                let on_row = move |row, data: ui::OrganizationPcbDesignRulesData| {
                    deferred(&w, move |s| {
                        s.element_tab_event(id, |t| t.organization_rules_row(row, &data));
                    });
                };
                Tab::Organization(Box::new(OrganizationTab::new(core, on_row).with_id(id)))
            }
            ui::LibraryTreeViewItemType::Device => {
                let Some(core) = self.element_core::<Device>(id, lib, relative, mode) else {
                    return;
                };
                let w = self.this.clone();
                let sink = Rc::new(move |event: DeviceRowEvent| {
                    deferred(&w, move |s| {
                        s.element_tab_event(id, |t| t.device_row_written(event));
                    });
                });
                Tab::Device(Box::new(DeviceTab::new(core, sink).with_id(id)))
            }
            other => {
                self.not_implemented(&format!("{other:?}"));
                return;
            }
        };
        tab.set_library_index(library_index);
        self.add_tab(tab);
        if let Some(w) = self.window() {
            let d = w.global::<ui::Data>();
            d.invoke_set_current_library(library_index);
            d.invoke_set_panel_page(ui::PanelPage::Documents);
        }
    }

    /// Runs `f` on the tab `id` and applies its update.
    pub(crate) fn element_tab_event(&mut self, id: TabId, f: impl FnOnce(&mut Tab) -> TabUpdate) {
        let Some((si, ti)) = self.find_tab(id) else {
            return;
        };
        let update = match self.sections[si].tab_mut(ti) {
            Some(t) => f(t),
            None => return,
        };
        self.apply_update(si, ti, update);
    }

    /// Opens a copy of the element of tab `id` as new element (upstream
    /// "save as" of the element tabs).
    pub(crate) fn duplicate_element_tab(&mut self, id: TabId) {
        let Some((si, ti)) = self.find_tab(id) else {
            return;
        };
        let (lib, kind, path) = match &mut self.sections[si].tabs_mut()[ti] {
            Tab::Symbol(t) => {
                t.core_mut().element_duplicated = true;
                (
                    Rc::clone(&t.core().library),
                    ui::LibraryTreeViewItemType::Symbol,
                    t.core().directory_path(),
                )
            }
            Tab::Package(t) => {
                t.core_mut().element_duplicated = true;
                (
                    Rc::clone(&t.core().library),
                    ui::LibraryTreeViewItemType::Package,
                    t.core().directory_path(),
                )
            }
            Tab::Component(t) => {
                t.core_mut().element_duplicated = true;
                (
                    Rc::clone(&t.core().library),
                    ui::LibraryTreeViewItemType::Component,
                    t.core().directory_path(),
                )
            }
            Tab::Device(t) => {
                t.core_mut().element_duplicated = true;
                (
                    Rc::clone(&t.core().library),
                    ui::LibraryTreeViewItemType::Device,
                    t.core().directory_path(),
                )
            }
            _ => return,
        };
        self.sections[si].refresh_tab(ti, &self.projects);
        let Some(relative) = lib.relative_path(&path) else {
            return;
        };
        if path.is_existing_dir() {
            self.open_element_tab(&lib, kind, Some(&relative), OpenMode::Duplicate);
        }
    }

    /// Opens the properties dialog of an item of a library element tab
    /// (upstream: the element FSMs open the dialogs).
    pub fn open_library_item_properties(&mut self, tab: TabId, item: LibraryItemRef) {
        use crate::dialogs::library_items::{
            LibraryCircleDialog, LibraryPolygonDialog, ObjectOwner, PinDialog, SymbolTextDialog,
        };
        use librepcb_editor::fsm::library::{ElementHost, SymbolHost};
        use librepcb_editor::library_editor::commands::SymbolObject;
        let Some((si, ti)) = self.find_tab(tab) else {
            return;
        };
        let dialog: Option<Box<dyn crate::dialogs::FormDialog>> =
            match (&self.sections[si].tabs()[ti], item) {
                (Tab::Symbol(t), LibraryItemRef::Symbol(item)) => {
                    let unit = t.length_unit();
                    match SymbolObject::from_symbol(t.core().editor.element(), item) {
                        Some(SymbolObject::Pin(pin)) => Some(Box::new(PinDialog::new(pin, unit))),
                        Some(SymbolObject::Polygon(p)) => {
                            Some(Box::new(LibraryPolygonDialog::new(
                                ObjectOwner::Symbol,
                                p,
                                &SymbolHost::polygon_layers(),
                                unit,
                            )))
                        }
                        Some(SymbolObject::Circle(c)) => Some(Box::new(LibraryCircleDialog::new(
                            ObjectOwner::Symbol,
                            c,
                            &SymbolHost::polygon_layers(),
                            unit,
                        ))),
                        Some(SymbolObject::Text(text)) => Some(Box::new(SymbolTextDialog::new(
                            text,
                            &SymbolHost::text_layers(),
                            unit,
                        ))),
                        _ => None,
                    }
                }
                _ => self.library_item_dialog(si, ti, item),
            };
        if let Some(dialog) = dialog {
            self.open_library_dialog(Some(tab), dialog);
        }
    }

    /// The properties dialogs of package items.
    fn library_item_dialog(
        &self,
        si: usize,
        ti: usize,
        item: LibraryItemRef,
    ) -> Option<Box<dyn crate::dialogs::FormDialog>> {
        use crate::dialogs::library_items::{
            FootprintHoleDialog, FootprintPadDialog, FootprintStrokeTextDialog,
            FootprintZoneDialog, LibraryCircleDialog, LibraryPolygonDialog, ObjectOwner,
        };
        use librepcb_editor::fsm::library::{ElementHost, PackageHost};
        use librepcb_editor::library_editor::commands::FootprintObject;
        let (Tab::Package(t), LibraryItemRef::Footprint(fpt, item)) =
            (&self.sections[si].tabs()[ti], item)
        else {
            return None;
        };
        let unit = t.length_unit();
        let fpt = fpt.or(t.current_footprint());
        let footprint = t.core().editor.element().footprints().by_uuid(&fpt?)?;
        Some(match FootprintObject::from_footprint(footprint, item)? {
            FootprintObject::Pad(pad) => {
                Box::new(FootprintPadDialog::new(fpt, pad, &t.package_pads(), unit))
            }
            FootprintObject::Polygon(p) => Box::new(LibraryPolygonDialog::new(
                ObjectOwner::Footprint(fpt),
                p,
                &PackageHost::polygon_layers(),
                unit,
            )),
            FootprintObject::Circle(c) => Box::new(LibraryCircleDialog::new(
                ObjectOwner::Footprint(fpt),
                c,
                &PackageHost::polygon_layers(),
                unit,
            )),
            FootprintObject::StrokeText(text) => Box::new(FootprintStrokeTextDialog::new(
                fpt,
                text,
                &PackageHost::text_layers(),
                unit,
            )),
            FootprintObject::Hole(hole) => Box::new(FootprintHoleDialog::new(fpt, hole, unit)),
            FootprintObject::Zone(zone) => Box::new(FootprintZoneDialog::new(fpt, zone, unit)),
        })
    }

    /// Opens a library element chooser dialog for a tab.
    pub(crate) fn open_element_chooser(
        &mut self,
        tab: TabId,
        purpose: crate::dialogs::chooser::ChooserPurpose,
    ) {
        let (db, locales) = {
            let ws = self.workspace.lock();
            (
                ws.shared_library_db(),
                ws.settings().library_locale_order.get().clone(),
            )
        };
        let dialog = crate::dialogs::chooser::ElementChooserDialog::new(purpose, db, locales);
        self.open_library_dialog(Some(tab), Box::new(dialog));
    }

    /// Lets the user choose a pinout CSV file for a device tab (upstream
    /// `DevicePinoutBuilder::loadFromFile()`).
    pub(crate) fn choose_pinout_file(&mut self, tab: TabId) {
        let title = tr!(
            "librepcb::editor::DevicePinoutBuilder",
            "Choose Pinout File"
        );
        let filters = [crate::file_dialog::Filter {
            name: "Comma-Separated Values (*.csv)".to_owned(),
            extensions: vec!["csv", "CSV"],
        }];
        let Some(path) = crate::file_dialog::open_file(&title, &filters, None) else {
            return;
        };
        match std::fs::read(&path) {
            Ok(content) => {
                let text = String::from_utf8_lossy(&content).into_owned();
                self.element_tab_event(tab, |t| t.pinout_file_chosen(&text));
            }
            Err(e) => self
                .notifications
                .borrow_mut()
                .push(crate::notifications::Notification {
                    auto_popup: true,
                    ..crate::notifications::Notification::new(
                        ui::NotificationType::Critical,
                        tr!("librepcb::editor::DevicePinoutBuilder", "Error"),
                        format!("{}: {e}", path.display()),
                    )
                }),
        }
    }

    /// Opens the "courtyard excess" dialog of a package tab.
    pub(crate) fn open_courtyard_offset_dialog(&mut self, tab: TabId) {
        let unit = self
            .find_tab(tab)
            .and_then(|(si, ti)| self.sections[si].tabs()[ti].length_unit())
            .unwrap_or_default();
        let dialog = crate::dialogs::library_items::CourtyardOffsetDialog::new(unit);
        self.open_library_dialog(Some(tab), Box::new(dialog));
    }

    /// Opens the "move/align" dialog of a package tab.
    pub(crate) fn open_move_align_dialog(
        &mut self,
        tab: TabId,
        positions: Vec<librepcb_core::types::Point>,
    ) {
        let unit = self
            .find_tab(tab)
            .and_then(|(si, ti)| self.sections[si].tabs()[ti].length_unit())
            .unwrap_or_default();
        let dialog = crate::dialogs::move_align::MoveAlignDialog::new(positions, unit);
        self.open_library_dialog(Some(tab), Box::new(dialog));
    }

    /// Lets the user choose a STEP file for a package tab (upstream
    /// `PackageModelListModel::chooseStepFile()`).
    pub(crate) fn choose_step_file(
        &mut self,
        tab: TabId,
        model: Option<librepcb_core::types::Uuid>,
    ) {
        let title = tr!(
            "librepcb::editor::PackageModelListModel",
            "Choose STEP Model"
        );
        let filters = [crate::file_dialog::Filter {
            name: "STEP Models (*.step *.stp *.STEP *.STP *.Step *.Stp)".to_owned(),
            extensions: vec!["step", "stp", "STEP", "STP"],
        }];
        let Some(path) = crate::file_dialog::open_file(&title, &filters, None) else {
            return;
        };
        let content = match std::fs::read(&path) {
            Ok(c) => c,
            Err(e) => {
                self.notifications
                    .borrow_mut()
                    .push(crate::notifications::Notification {
                        auto_popup: true,
                        ..crate::notifications::Notification::new(
                            ui::NotificationType::Critical,
                            tr!("librepcb::editor::PackageModelListModel", "Error"),
                            format!("{}: {e}", path.display()),
                        )
                    });
                return;
            }
        };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.element_tab_event(tab, |t| t.step_file_chosen(model, &stem, content));
    }

    /// Opens the "import pins" dialog of a symbol tab.
    pub(crate) fn open_import_pins_dialog(&mut self, tab: TabId) {
        let dialog = crate::dialogs::library_items::ImportPinsDialog::new();
        self.open_library_dialog(Some(tab), Box::new(dialog));
    }

    /// Closes the tabs of the elements in `paths` (before removing them).
    pub(crate) fn close_library_element_tabs(&mut self, paths: &[FilePath]) {
        for si in 0..self.sections.len() {
            while let Some(ti) = self.sections[si].tabs().iter().position(|t| {
                !matches!(t, Tab::Library(_))
                    && t.directory_path().is_some_and(|p| paths.contains(&p))
            }) {
                self.sections[si].remove_tab(ti);
            }
            self.update_section_row(si);
        }
    }
}
