//! Port of libs/librepcb/eagleimport/eaglelibraryimport.{h,cpp} (and the
//! non-UI logic of the EAGLE library import wizard).
//!
//! Usage (the steps of the wizard):
//! 1. [`EagleLibraryImport::open()`] parses a `*.lbr` file and lists its
//!    elements (all unchecked).
//! 2. Select elements with `set_*_checked()` (dependencies are selected
//!    automatically as "partially checked") and set the options (name
//!    prefix, categories, see [`EagleLibraryImport::set_add_name_prefix()`]
//!    and [`EagleLibraryImport::set_component_category()`]).
//! 3. [`EagleLibraryImport::run()`] converts the selected elements and
//!    writes them into the destination library, reporting progress and
//!    honoring cancellation. It only needs `&self`, so it can run on
//!    another thread.
//!
//! Differences to upstream:
//! - Not a `QThread`: [`run()`](EagleLibraryImport::run) is a blocking call
//!   which the caller runs on a thread of its choice; signals are replaced
//!   by return values ([`CheckStateChange`]) and [`Progress`].
//! - The options of the wizard context (`EAGLE_` name prefix, import
//!   category plus an optional user category) are part of this type.

use std::collections::BTreeSet;

use librepcb_core::fileio::FilePath;
use librepcb_core::library::cat::{ComponentCategoryKind, PackageCategoryKind};
use librepcb_core::types::Uuid;
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_core::utils::toolbox::compare_numeric;
use librepcb_i18n::{tr, trn};

use super::error::Result;
use super::library_converter::{EagleLibraryConverter, EagleLibraryConverterSettings};
use super::model;
use super::type_converter::EagleTypeConverter;
use crate::library_writer::{save_element, try_create_category_if_required};
use crate::{CheckState, CheckStateChange, ElementKind, ImportSummary, Progress, UuidGenerator};

const CTX: &str = "librepcb::eagleimport::EagleLibraryImport";

/// Name of the categories created for imported elements.
pub const CATEGORY_NAME: &str = "EAGLE Import";

/// Name prefix added to the imported elements if enabled
/// ([`EagleLibraryImport::set_add_name_prefix()`]).
pub const NAME_PREFIX: &str = "EAGLE_";

/// A symbol of the opened library.
#[derive(Debug, Clone)]
pub struct ImportSymbol {
    /// Name.
    pub display_name: String,
    /// Description (may contain HTML).
    pub description: String,
    /// Selection state.
    pub check_state: CheckState,
    /// The EAGLE symbol.
    pub symbol: model::Symbol,
}

/// A package of the opened library.
#[derive(Debug, Clone)]
pub struct ImportPackage {
    /// Name.
    pub display_name: String,
    /// Description (may contain HTML).
    pub description: String,
    /// Selection state.
    pub check_state: CheckState,
    /// The EAGLE package.
    pub package: model::Package,
}

/// A component (EAGLE device set) of the opened library.
#[derive(Debug, Clone)]
pub struct ImportComponent {
    /// Name (device set name without trailing `-`/`_`).
    pub display_name: String,
    /// Description (may contain HTML).
    pub description: String,
    /// Selection state.
    pub check_state: CheckState,
    /// Names of the symbols used by the gates.
    pub symbol_display_names: BTreeSet<String>,
    /// The EAGLE device set.
    pub device_set: model::DeviceSet,
}

/// A device of the opened library.
#[derive(Debug, Clone)]
pub struct ImportDevice {
    /// Name (built from the names of the device set and the device).
    pub display_name: String,
    /// Description (may contain HTML).
    pub description: String,
    /// Selection state.
    pub check_state: CheckState,
    /// Display name of the component.
    pub component_display_name: String,
    /// Display name of the package.
    pub package_display_name: String,
    /// The EAGLE device.
    pub device: model::Device,
    /// The EAGLE device set.
    pub device_set: model::DeviceSet,
}

trait Element {
    fn name(&self) -> &str;
    fn state(&mut self) -> &mut CheckState;
    fn is_checked(&self) -> bool;
}

macro_rules! impl_element {
    ($($t:ty),*) => {$(
        impl Element for $t {
            fn name(&self) -> &str {
                &self.display_name
            }
            fn state(&mut self) -> &mut CheckState {
                &mut self.check_state
            }
            fn is_checked(&self) -> bool {
                self.check_state.is_imported()
            }
        }
    )*};
}
impl_element!(ImportSymbol, ImportPackage, ImportComponent, ImportDevice);

fn count_checked<T: Element>(elements: &[T]) -> usize {
    elements.iter().filter(|e| e.is_checked()).count()
}

fn set_checked<T: Element>(elements: &mut [T], name: &str, checked: bool) -> bool {
    let state = if checked {
        CheckState::Checked
    } else {
        CheckState::Unchecked
    };
    let mut modified = false;
    for e in elements.iter_mut() {
        if (e.name() == name) && (*e.state() != state) {
            *e.state() = state;
            modified = true;
        }
    }
    modified
}

/// The EAGLE library import (upstream `EagleLibraryImport` plus the options
/// of `EagleLibraryImportWizardContext`).
#[derive(Debug, Clone)]
pub struct EagleLibraryImport {
    destination: FilePath,
    settings: EagleLibraryConverterSettings,
    add_to_category: bool,
    component_category: Option<Uuid>,
    package_category: Option<Uuid>,
    loaded_file_path: Option<FilePath>,
    symbols: Vec<ImportSymbol>,
    packages: Vec<ImportPackage>,
    components: Vec<ImportComponent>,
    devices: Vec<ImportDevice>,
}

impl EagleLibraryImport {
    /// Creates an import into the library directory `destination`, with
    /// random UUIDs. Imported elements are added to the "EAGLE Import"
    /// categories by default (like the wizard).
    pub fn new(destination: FilePath) -> Self {
        Self::with_uuid_generator(destination, UuidGenerator::random())
    }

    /// Like [`new()`](Self::new), with a custom UUID generator.
    pub fn with_uuid_generator(destination: FilePath, uuids: UuidGenerator) -> Self {
        let mut obj = Self {
            destination,
            settings: EagleLibraryConverterSettings {
                uuids,
                ..EagleLibraryConverterSettings::default()
            },
            add_to_category: true,
            component_category: None,
            package_category: None,
            loaded_file_path: None,
            symbols: Vec::new(),
            packages: Vec::new(),
            components: Vec::new(),
            devices: Vec::new(),
        };
        obj.update_categories();
        obj
    }

    /// Returns the destination library directory.
    pub fn destination(&self) -> &FilePath {
        &self.destination
    }

    /// Returns the converter settings (name prefix, categories, ...).
    pub fn settings(&self) -> &EagleLibraryConverterSettings {
        &self.settings
    }

    /// Returns the converter settings for modification.
    pub fn settings_mut(&mut self) -> &mut EagleLibraryConverterSettings {
        &mut self.settings
    }

    /// Returns the opened `*.lbr` file.
    pub fn loaded_file_path(&self) -> Option<&FilePath> {
        self.loaded_file_path.as_ref()
    }

    /// Returns the number of elements of the opened library.
    pub fn total_elements_count(&self) -> usize {
        self.symbols.len() + self.packages.len() + self.components.len() + self.devices.len()
    }

    /// Returns the number of elements to import.
    pub fn checked_elements_count(&self) -> usize {
        self.checked_symbols_count()
            + self.checked_packages_count()
            + self.checked_components_count()
            + self.checked_devices_count()
    }

    /// Returns the number of symbols to import.
    pub fn checked_symbols_count(&self) -> usize {
        count_checked(&self.symbols)
    }

    /// Returns the number of packages to import.
    pub fn checked_packages_count(&self) -> usize {
        count_checked(&self.packages)
    }

    /// Returns the number of components to import.
    pub fn checked_components_count(&self) -> usize {
        count_checked(&self.components)
    }

    /// Returns the number of devices to import.
    pub fn checked_devices_count(&self) -> usize {
        count_checked(&self.devices)
    }

    /// Returns the symbols (sorted by name).
    pub fn symbols(&self) -> &[ImportSymbol] {
        &self.symbols
    }

    /// Returns the packages (sorted by name).
    pub fn packages(&self) -> &[ImportPackage] {
        &self.packages
    }

    /// Returns the components (sorted by name).
    pub fn components(&self) -> &[ImportComponent] {
        &self.components
    }

    /// Returns the devices (sorted by name).
    pub fn devices(&self) -> &[ImportDevice] {
        &self.devices
    }

    /// Sets the name prefix of all imported elements.
    pub fn set_name_prefix(&mut self, prefix: impl Into<String>) {
        self.settings.name_prefix = prefix.into();
    }

    /// Enables or disables the [`NAME_PREFIX`] (wizard option).
    pub fn set_add_name_prefix(&mut self, add: bool) {
        self.set_name_prefix(if add { NAME_PREFIX } else { "" });
    }

    /// Sets the categories of imported symbols.
    pub fn set_symbol_categories(&mut self, uuids: BTreeSet<Uuid>) {
        self.settings.symbol_categories = uuids;
    }

    /// Sets the categories of imported packages.
    pub fn set_package_categories(&mut self, uuids: BTreeSet<Uuid>) {
        self.settings.package_categories = uuids;
    }

    /// Sets the categories of imported components.
    pub fn set_component_categories(&mut self, uuids: BTreeSet<Uuid>) {
        self.settings.component_categories = uuids;
    }

    /// Sets the categories of imported devices.
    pub fn set_device_categories(&mut self, uuids: BTreeSet<Uuid>) {
        self.settings.device_categories = uuids;
    }

    /// Enables or disables adding the elements to the "EAGLE Import"
    /// categories (wizard option, enabled by default).
    pub fn set_add_to_category(&mut self, add: bool) {
        self.add_to_category = add;
        self.update_categories();
    }

    /// Sets an additional component category for symbols, components and
    /// devices (wizard option).
    pub fn set_component_category(&mut self, uuid: Option<Uuid>) {
        self.component_category = uuid;
        self.update_categories();
    }

    /// Sets an additional package category (wizard option).
    pub fn set_package_category(&mut self, uuid: Option<Uuid>) {
        self.package_category = uuid;
        self.update_categories();
    }

    fn update_categories(&mut self) {
        let mut cmp = BTreeSet::new();
        let mut pkg = BTreeSet::new();
        if self.add_to_category {
            cmp.insert(Self::component_category_uuid());
            pkg.insert(Self::package_category_uuid());
        }
        cmp.extend(self.component_category);
        pkg.extend(self.package_category);
        self.set_symbol_categories(cmp.clone());
        self.set_component_categories(cmp.clone());
        self.set_device_categories(cmp);
        self.set_package_categories(pkg);
    }

    /// Checks or unchecks symbols by name. Returns the dependency state
    /// changes of other elements.
    pub fn set_symbol_checked(&mut self, name: &str, checked: bool) -> Vec<CheckStateChange> {
        if set_checked(&mut self.symbols, name, checked) {
            self.update_dependencies()
        } else {
            Vec::new()
        }
    }

    /// Checks or unchecks packages by name. Returns the dependency state
    /// changes of other elements.
    pub fn set_package_checked(&mut self, name: &str, checked: bool) -> Vec<CheckStateChange> {
        if set_checked(&mut self.packages, name, checked) {
            self.update_dependencies()
        } else {
            Vec::new()
        }
    }

    /// Checks or unchecks components by name. Returns the dependency state
    /// changes of other elements.
    pub fn set_component_checked(&mut self, name: &str, checked: bool) -> Vec<CheckStateChange> {
        if set_checked(&mut self.components, name, checked) {
            self.update_dependencies()
        } else {
            Vec::new()
        }
    }

    /// Checks or unchecks devices by name. Returns the dependency state
    /// changes of other elements.
    pub fn set_device_checked(&mut self, name: &str, checked: bool) -> Vec<CheckStateChange> {
        if set_checked(&mut self.devices, name, checked) {
            self.update_dependencies()
        } else {
            Vec::new()
        }
    }

    /// Checks or unchecks all elements.
    pub fn set_all_checked(&mut self, checked: bool) {
        let state = if checked {
            CheckState::Checked
        } else {
            CheckState::Unchecked
        };
        self.symbols.iter_mut().for_each(|e| e.check_state = state);
        self.packages.iter_mut().for_each(|e| e.check_state = state);
        self.components
            .iter_mut()
            .for_each(|e| e.check_state = state);
        self.devices.iter_mut().for_each(|e| e.check_state = state);
        self.update_dependencies();
    }

    /// Forgets the opened library.
    pub fn reset(&mut self) {
        self.symbols.clear();
        self.packages.clear();
        self.components.clear();
        self.devices.clear();
        self.loaded_file_path = None;
    }

    /// Opens a `*.lbr` file and lists its elements (all unchecked). Returns
    /// the (non-fatal) parse errors.
    pub fn open(&mut self, lbr: &FilePath) -> Result<Vec<String>> {
        self.reset();
        let mut errors = Vec::new();
        let lib = model::Library::open(lbr.as_path(), &mut errors).inspect_err(|e| {
            log::warn!("Failed to parse EAGLE library: {e}");
        })?;
        self.load(lib);
        self.loaded_file_path = Some(lbr.clone());
        Ok(errors)
    }

    /// Lists the elements of a parsed library (all unchecked).
    pub fn load(&mut self, lib: model::Library) {
        self.reset();
        let tc = EagleTypeConverter::new(self.settings.uuids.clone());
        for symbol in lib.symbols {
            self.symbols.push(ImportSymbol {
                display_name: symbol.name.clone(),
                description: symbol.description.clone(),
                check_state: CheckState::Unchecked,
                symbol,
            });
        }
        for package in lib.packages {
            self.packages.push(ImportPackage {
                display_name: package.name.clone(),
                description: package.description.clone(),
                check_state: CheckState::Unchecked,
                package,
            });
        }
        for device_set in lib.device_sets {
            let cmp_name = tc.convert_component_name(&device_set.name).into_string();
            for device in &device_set.devices {
                self.devices.push(ImportDevice {
                    display_name: tc
                        .convert_device_name(&device_set.name, &device.name)
                        .into_string(),
                    description: device_set.description.clone(),
                    check_state: CheckState::Unchecked,
                    component_display_name: cmp_name.clone(),
                    package_display_name: device.package.clone(),
                    device: device.clone(),
                    device_set: device_set.clone(),
                });
            }
            self.components.push(ImportComponent {
                display_name: cmp_name,
                description: device_set.description.clone(),
                check_state: CheckState::Unchecked,
                symbol_display_names: device_set.gates.iter().map(|g| g.symbol.clone()).collect(),
                device_set,
            });
        }

        // Sort all elements by name to improve readability.
        self.symbols
            .sort_by(|a, b| compare_numeric(&a.display_name, &b.display_name));
        self.packages
            .sort_by(|a, b| compare_numeric(&a.display_name, &b.display_name));
        self.components
            .sort_by(|a, b| compare_numeric(&a.display_name, &b.display_name));
        self.devices
            .sort_by(|a, b| compare_numeric(&a.display_name, &b.display_name));
    }

    fn update_dependencies(&mut self) -> Vec<CheckStateChange> {
        let mut changes = Vec::new();
        let mut dependent_packages = BTreeSet::new();
        let mut dependent_components = BTreeSet::new();
        for dev in self.devices.iter().filter(|d| d.is_checked()) {
            dependent_components.insert(dev.component_display_name.clone());
            dependent_packages.insert(dev.package_display_name.clone());
        }

        let mut dependent_symbols = BTreeSet::new();
        for cmp in &mut self.components {
            if cmp
                .check_state
                .set_dependent(dependent_components.contains(&cmp.display_name))
            {
                changes.push(CheckStateChange::new(
                    ElementKind::Component,
                    "",
                    &cmp.display_name,
                    cmp.check_state,
                ));
            }
            if cmp.is_checked() {
                dependent_symbols.extend(cmp.symbol_display_names.iter().cloned());
            }
        }
        for pkg in &mut self.packages {
            if pkg
                .check_state
                .set_dependent(dependent_packages.contains(&pkg.display_name))
            {
                changes.push(CheckStateChange::new(
                    ElementKind::Package,
                    "",
                    &pkg.display_name,
                    pkg.check_state,
                ));
            }
        }
        for sym in &mut self.symbols {
            if sym
                .check_state
                .set_dependent(dependent_symbols.contains(&sym.display_name))
            {
                changes.push(CheckStateChange::new(
                    ElementKind::Symbol,
                    "",
                    &sym.display_name,
                    sym.check_state,
                ));
            }
        }
        changes
    }

    /// Converts the checked elements and writes them into the destination
    /// library. Errors of single elements are logged and skip the element.
    /// Stops early if `progress` gets canceled.
    pub fn run(&self, log: &MessageLogger<'_>, progress: &Progress) -> ImportSummary {
        let mut converter = EagleLibraryConverter::new(self.settings.clone());
        let total = self.checked_elements_count();
        let mut count = 0;
        let mut imported = 0;
        let report = |count: usize| {
            progress.set_percent(((100 * count) / total.max(1)) as u32);
        };

        // Create categories, if required.
        let cmp_categories: BTreeSet<Uuid> = self
            .settings
            .symbol_categories
            .iter()
            .chain(&self.settings.component_categories)
            .chain(&self.settings.device_categories)
            .copied()
            .collect();
        let names = ("EAGLE Import", CATEGORY_NAME, "eagle");
        try_create_category_if_required::<ComponentCategoryKind>(
            &self.destination,
            Self::component_category_uuid(),
            &cmp_categories,
            names,
            log,
        );
        try_create_category_if_required::<PackageCategoryKind>(
            &self.destination,
            Self::package_category_uuid(),
            &self.settings.package_categories,
            names,
            log,
        );

        let mut canceled = false;
        for sym in self.symbols.iter().filter(|e| e.is_checked()) {
            if progress.is_canceled() {
                canceled = true;
                break;
            }
            let elog = MessageLogger::child(log, &sym.display_name);
            progress.set_status(sym.display_name.clone());
            let result = converter
                .create_symbol("", "", &sym.symbol, &elog)
                .and_then(|mut e| Ok(save_element(&self.destination, &mut e)?));
            match result {
                Ok(()) => imported += 1,
                Err(e) => elog.critical(&tr!(CTX, "Skipped symbol due to error: {0}", e)),
            }
            count += 1;
            report(count);
        }
        for pkg in self.packages.iter().filter(|e| e.is_checked()) {
            if canceled || progress.is_canceled() {
                canceled = true;
                break;
            }
            let elog = MessageLogger::child(log, &pkg.display_name);
            progress.set_status(pkg.display_name.clone());
            let result = converter
                .create_package("", "", &pkg.package, &elog)
                .and_then(|mut e| Ok(save_element(&self.destination, &mut e)?));
            match result {
                Ok(()) => imported += 1,
                Err(e) => elog.critical(&tr!(CTX, "Skipped package due to error: {0}", e)),
            }
            count += 1;
            report(count);
        }
        for cmp in self.components.iter().filter(|e| e.is_checked()) {
            if canceled || progress.is_canceled() {
                canceled = true;
                break;
            }
            let elog = MessageLogger::child(log, &cmp.display_name);
            progress.set_status(cmp.display_name.clone());
            let result = converter
                .create_component("", "", &cmp.device_set, &elog)
                .and_then(|mut e| Ok(save_element(&self.destination, &mut e)?));
            match result {
                Ok(()) => imported += 1,
                Err(e) => elog.critical(&tr!(CTX, "Skipped component due to error: {0}", e)),
            }
            count += 1;
            report(count);
        }
        for dev in self.devices.iter().filter(|e| e.is_checked()) {
            if canceled || progress.is_canceled() {
                canceled = true;
                break;
            }
            let elog = MessageLogger::child(log, &dev.display_name);
            progress.set_status(dev.display_name.clone());
            let result = converter
                .create_device("", "", &dev.device_set, &dev.device, "", "", &elog)
                .and_then(|mut e| Ok(save_element(&self.destination, &mut e)?));
            match result {
                Ok(()) => imported += 1,
                Err(e) => elog.critical(&tr!(CTX, "Skipped device due to error: {0}", e)),
            }
            count += 1;
            report(count);
        }

        progress.set_percent(100);
        progress.set_status(trn!(
            CTX,
            "Finished: {0} of {1} element(s) imported",
            "Finished: {0} of {1} element(s) imported",
            total,
            count,
            total
        ));
        ImportSummary {
            total,
            processed: count,
            imported,
            canceled,
        }
    }

    /// UUID of the component category created for imported symbols,
    /// components and devices.
    pub fn component_category_uuid() -> Uuid {
        "5d23e248-e18b-4d98-b9d7-541c54747b45"
            .parse()
            .expect("valid UUID")
    }

    /// UUID of the package category created for imported packages.
    pub fn package_category_uuid() -> Uuid {
        "f52fabb6-7cdf-4e61-b26a-9d8ec0b8bc49"
            .parse()
            .expect("valid UUID")
    }
}
