//! Port of libs/librepcb/kicadimport/kicadlibraryimport.{h,cpp} (and the
//! non-UI logic of the KiCad library import wizard).
//!
//! Usage (the steps of the wizard):
//! 1. [`KiCadLibraryImport::scan()`] searches a file or directory for
//!    symbol libraries (`*.kicad_sym`), footprint libraries (`*.pretty`)
//!    and 3D model libraries (`*.3dshapes`).
//! 2. [`KiCadLibraryImport::parse()`] parses the found libraries and lists
//!    their elements (all checked; elements imported before are marked as
//!    already imported and skipped).
//! 3. Select elements with `set_*_checked()` (dependencies are selected
//!    automatically as "partially checked") and set the options.
//! 4. [`KiCadLibraryImport::import()`] converts the selected elements and
//!    writes them into the destination library.
//!
//! Differences to upstream:
//! - The steps are blocking calls (upstream: `QtConcurrent` futures with
//!   `*Finished()` signals); the caller runs them on a thread of its choice
//!   and observes/cancels them through [`Progress`]. Dependency state
//!   changes are returned ([`CheckStateChange`]) instead of signals.
//! - Already imported elements are looked up through
//!   [`ImportedElementLookup`] (upstream: the workspace library database,
//!   waiting for a running library scan first).
//! - Directory entries are sorted by name with Rust string ordering
//!   (upstream `QDir::Name`), see COMPAT.md.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_core::library::cat::{ComponentCategoryKind, PackageCategoryKind};
use librepcb_core::types::Uuid;
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_core::workspace::ElementKind as DbKind;
use librepcb_i18n::{tr, trn};

use super::ImportedElementLookup;
use super::library_converter::{KiCadLibraryConverter, KiCadLibraryConverterSettings};
use super::type_converter::KiCadTypeConverter;
use super::types::{KiCadFootprint, KiCadSymbolGate, KiCadSymbolGateStyle, KiCadSymbolLibrary};
use crate::library_writer::{save_element, try_create_category_if_required};
use crate::{CheckState, CheckStateChange, ElementKind, ImportSummary, Progress, UuidGenerator};

const CTX: &str = "librepcb::kicadimport::KiCadLibraryImport";

/// Name of the categories created for imported elements.
pub const CATEGORY_NAME: &str = "KiCad Import";

/// Name prefix added to the imported elements if enabled
/// ([`KiCadLibraryImport::set_add_name_prefix()`]).
pub const NAME_PREFIX: &str = "KICAD_";

/// The KiCad version whose libraries are supported.
pub const COMPATIBLE_KICAD_VERSION: &str = "9.x";

/// State of the import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ImportState {
    /// Nothing scanned yet.
    Reset,
    /// Libraries found.
    Scanned,
    /// Libraries parsed, elements listed.
    Parsed,
    /// Elements imported.
    Imported,
}

/// A gate (unit) of a symbol, imported as a symbol.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportGate {
    /// Index as specified in the KiCad symbol.
    pub index: i32,
    /// "generated_by" of the symbol.
    pub sym_generated_by: String,
    /// Whether the symbol was imported before.
    pub already_imported: bool,
}

/// A KiCad symbol, imported as symbols (one per gate), a component and a
/// device (if it has a footprint).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportSymbol {
    /// Name.
    pub name: String,
    /// "generated_by" of the component.
    pub cmp_generated_by: String,
    /// "generated_by" of the device.
    pub dev_generated_by: String,
    /// "generated_by" of the package (empty if there is no footprint).
    pub pkg_generated_by: String,
    /// Whether all gates were imported before.
    pub sym_already_imported: bool,
    /// Whether the component was imported before.
    pub cmp_already_imported: bool,
    /// Whether the device was imported before.
    pub dev_already_imported: bool,
    /// Name of the extended symbol (empty if none).
    pub extends: String,
    /// Gates.
    pub gates: Vec<ImportGate>,
    /// Selection state of the symbols.
    pub sym_checked: CheckState,
    /// Selection state of the component.
    pub cmp_checked: CheckState,
    /// Selection state of the device.
    pub dev_checked: CheckState,
}

/// A symbol library file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolLibrary {
    /// The `*.kicad_sym` file.
    pub file: FilePath,
    /// Symbols (after parsing).
    pub symbols: Vec<ImportSymbol>,
}

/// A footprint, imported as a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFootprint {
    /// The `*.kicad_mod` file.
    pub file: FilePath,
    /// Name.
    pub name: String,
    /// "generated_by" of the package.
    pub generated_by: String,
    /// Whether the package was imported before.
    pub already_imported: bool,
    /// Selection state.
    pub checked: CheckState,
}

/// A footprint library directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FootprintLibrary {
    /// The `*.pretty` directory.
    pub dir: FilePath,
    /// Footprint files.
    pub files: Vec<FilePath>,
    /// Footprints (after parsing).
    pub footprints: Vec<ImportFootprint>,
}

/// A 3D model library directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package3dLibrary {
    /// The `*.3dshapes` directory.
    pub dir: FilePath,
    /// STEP files.
    pub step_files: Vec<FilePath>,
}

/// Found libraries and their elements.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanResult {
    /// Symbol libraries.
    pub symbol_libs: Vec<SymbolLibrary>,
    /// Footprint libraries.
    pub footprint_libs: Vec<FootprintLibrary>,
    /// 3D model libraries.
    pub package_3d_libs: Vec<Package3dLibrary>,
    /// Number of found files.
    pub file_count: usize,
}

fn generated_by(lib_name: &str, keys: &[&str]) -> String {
    let mut parts = vec!["KiCadImport", lib_name];
    parts.extend_from_slice(keys);
    parts.join("::")
}

fn merge_gate(out: &mut KiCadSymbolGate, input: &KiCadSymbolGate) {
    out.arcs.extend(input.arcs.iter().cloned());
    out.circles.extend(input.circles.iter().cloned());
    out.rectangles.extend(input.rectangles.iter().cloned());
    out.polylines.extend(input.polylines.iter().cloned());
    out.pins.extend(input.pins.iter().cloned());
}

/// Merges the gates of a symbol: gates of the base style with the same
/// index are merged, the common gate (index 0) is merged into all others if
/// there are several, De Morgan styles are dropped.
pub fn merge_symbol_gates(gates: &[KiCadSymbolGate], symbol_name: &str) -> Vec<KiCadSymbolGate> {
    // Collect all gates.
    let mut map: BTreeMap<i32, KiCadSymbolGate> = BTreeMap::new();
    for gate in gates.iter().filter(|g| {
        matches!(
            g.style,
            KiCadSymbolGateStyle::Base | KiCadSymbolGateStyle::Common
        )
    }) {
        match map.get_mut(&gate.index) {
            Some(existing) => merge_gate(existing, gate),
            None => {
                map.insert(gate.index, gate.clone());
            }
        }
    }

    // Only if we have multiple gates, apply common geometry to other gates
    // (confusing KiCad logic).
    if map.len() > 1
        && let Some(common) = map.remove(&0)
    {
        for gate in map.values_mut() {
            merge_gate(gate, &common);
        }
    }

    // Update gate properties.
    let count = map.len();
    map.into_values()
        .map(|mut gate| {
            gate.name = if count > 1 {
                format!("{symbol_name}:{}", gate.index)
            } else {
                symbol_name.to_owned()
            };
            gate.style = KiCadSymbolGateStyle::Base;
            gate
        })
        .collect()
}

/// Lists the files (`dirs == false`) or directories in `dir` whose name ends
/// with `suffix` (case insensitive, empty = all), sorted by name. Hidden
/// entries are skipped (like `QDir` without `QDir::Hidden`).
fn find_items(dir: &FilePath, dirs: bool, suffix: &str) -> Vec<FilePath> {
    let Ok(entries) = std::fs::read_dir(dir.as_path()) else {
        return Vec::new();
    };
    let mut items: Vec<FilePath> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let is_dir = e.path().is_dir();
            (is_dir == dirs) && !name.starts_with('.') && name.ends_with(suffix)
        })
        .filter_map(|e| FilePath::new(e.path()))
        .collect();
    items.sort_by(|a, b| a.file_name().cmp(b.file_name()));
    items
}

/// The KiCad library import (upstream `KiCadLibraryImport` plus the options
/// of `KiCadLibraryImportWizardContext`).
#[derive(Debug, Clone)]
pub struct KiCadLibraryImport {
    destination: FilePath,
    settings: KiCadLibraryConverterSettings,
    add_to_category: bool,
    component_category: Option<Uuid>,
    package_category: Option<Uuid>,
    loaded_libs_path: Option<FilePath>,
    loaded_shapes_3d_path: Option<FilePath>,
    state: ImportState,
    result: ScanResult,
}

impl KiCadLibraryImport {
    /// Creates an import into the library directory `destination`, with
    /// random UUIDs. Unlike [`EagleLibraryImport`](crate::eagle::EagleLibraryImport)
    /// and like upstream, no categories are set by default (see
    /// [`set_add_to_category()`](Self::set_add_to_category)).
    pub fn new(destination: FilePath) -> Self {
        Self::with_uuid_generator(destination, UuidGenerator::random())
    }

    /// Like [`new()`](Self::new), with a custom UUID generator.
    pub fn with_uuid_generator(destination: FilePath, uuids: UuidGenerator) -> Self {
        Self {
            destination,
            settings: KiCadLibraryConverterSettings {
                uuids,
                ..KiCadLibraryConverterSettings::default()
            },
            add_to_category: false,
            component_category: None,
            package_category: None,
            loaded_libs_path: None,
            loaded_shapes_3d_path: None,
            state: ImportState::Reset,
            result: ScanResult::default(),
        }
    }

    /// Returns the destination library directory.
    pub fn destination(&self) -> &FilePath {
        &self.destination
    }

    /// Returns the converter settings.
    pub fn settings(&self) -> &KiCadLibraryConverterSettings {
        &self.settings
    }

    /// Returns the converter settings for modification.
    pub fn settings_mut(&mut self) -> &mut KiCadLibraryConverterSettings {
        &mut self.settings
    }

    /// Returns the state.
    pub fn state(&self) -> ImportState {
        self.state
    }

    /// Returns the scanned libraries path.
    pub fn loaded_libs_path(&self) -> Option<&FilePath> {
        self.loaded_libs_path.as_ref()
    }

    /// Returns the scanned 3D models path.
    pub fn loaded_shapes_3d_path(&self) -> Option<&FilePath> {
        self.loaded_shapes_3d_path.as_ref()
    }

    /// Returns the found libraries and elements.
    pub fn result(&self) -> &ScanResult {
        &self.result
    }

    /// Returns whether parsing can be started (libraries found).
    pub fn can_start_parsing(&self) -> bool {
        matches!(
            self.state,
            ImportState::Scanned | ImportState::Parsed | ImportState::Imported
        ) && (self.result.file_count > 0)
    }

    /// Returns whether there are elements to select.
    pub fn can_start_selecting(&self) -> bool {
        matches!(self.state, ImportState::Parsed | ImportState::Imported)
            && (self
                .result
                .symbol_libs
                .iter()
                .any(|l| !l.symbols.is_empty())
                || self
                    .result
                    .footprint_libs
                    .iter()
                    .any(|l| !l.footprints.is_empty()))
    }

    /// Returns whether there are elements to import.
    pub fn can_start_import(&self) -> bool {
        matches!(self.state, ImportState::Parsed | ImportState::Imported)
            && (self.count_to_import() > 0)
    }

    fn count_to_import(&self) -> usize {
        let mut count = 0;
        for lib in &self.result.footprint_libs {
            count += lib
                .footprints
                .iter()
                .filter(|f| f.checked.is_imported() && !f.already_imported)
                .count();
        }
        for lib in &self.result.symbol_libs {
            for sym in &lib.symbols {
                if sym.sym_checked.is_imported() && !sym.sym_already_imported {
                    count += sym.gates.iter().filter(|g| !g.already_imported).count();
                }
                if sym.cmp_checked.is_imported()
                    && !sym.cmp_already_imported
                    && sym.extends.is_empty()
                {
                    count += 1;
                }
                if sym.dev_checked.is_imported()
                    && !sym.dev_already_imported
                    && !sym.pkg_generated_by.is_empty()
                {
                    count += 1;
                }
            }
        }
        count
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

    /// Enables or disables adding the elements to the "KiCad Import"
    /// categories (wizard option, enabled by default in the wizard).
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

    fn set_symbol_flag(
        &mut self,
        lib_name: &str,
        sym_name: &str,
        checked: bool,
        field: fn(&mut ImportSymbol) -> &mut CheckState,
    ) -> Vec<CheckStateChange> {
        let state = if checked {
            CheckState::Checked
        } else {
            CheckState::Unchecked
        };
        let mut modified = false;
        for lib in self
            .result
            .symbol_libs
            .iter_mut()
            .filter(|l| l.file.complete_basename() == lib_name)
        {
            for sym in lib.symbols.iter_mut().filter(|s| s.name == sym_name) {
                let s = field(sym);
                if *s != state {
                    *s = state;
                    modified = true;
                }
            }
        }
        if modified {
            self.update_dependencies()
        } else {
            Vec::new()
        }
    }

    /// Checks or unchecks the symbols (gates) of a symbol.
    pub fn set_symbol_checked(
        &mut self,
        lib_name: &str,
        sym_name: &str,
        checked: bool,
    ) -> Vec<CheckStateChange> {
        self.set_symbol_flag(lib_name, sym_name, checked, |s| &mut s.sym_checked)
    }

    /// Checks or unchecks the component of a symbol.
    pub fn set_component_checked(
        &mut self,
        lib_name: &str,
        sym_name: &str,
        checked: bool,
    ) -> Vec<CheckStateChange> {
        self.set_symbol_flag(lib_name, sym_name, checked, |s| &mut s.cmp_checked)
    }

    /// Checks or unchecks the device of a symbol.
    pub fn set_device_checked(
        &mut self,
        lib_name: &str,
        sym_name: &str,
        checked: bool,
    ) -> Vec<CheckStateChange> {
        self.set_symbol_flag(lib_name, sym_name, checked, |s| &mut s.dev_checked)
    }

    /// Checks or unchecks a footprint.
    pub fn set_package_checked(
        &mut self,
        lib_name: &str,
        fpt_name: &str,
        checked: bool,
    ) -> Vec<CheckStateChange> {
        let state = if checked {
            CheckState::Checked
        } else {
            CheckState::Unchecked
        };
        let mut modified = false;
        for lib in self
            .result
            .footprint_libs
            .iter_mut()
            .filter(|l| l.dir.complete_basename() == lib_name)
        {
            for fpt in lib.footprints.iter_mut().filter(|f| f.name == fpt_name) {
                if fpt.checked != state {
                    fpt.checked = state;
                    modified = true;
                }
            }
        }
        if modified {
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
        for sym in self
            .result
            .symbol_libs
            .iter_mut()
            .flat_map(|l| l.symbols.iter_mut())
        {
            sym.sym_checked = state;
            sym.cmp_checked = state;
            sym.dev_checked = state;
        }
        for fpt in self
            .result
            .footprint_libs
            .iter_mut()
            .flat_map(|l| l.footprints.iter_mut())
        {
            fpt.checked = state;
        }
        self.update_dependencies();
    }

    /// Forgets the scanned libraries.
    pub fn reset(&mut self) {
        self.state = ImportState::Reset;
        self.loaded_libs_path = None;
        self.loaded_shapes_3d_path = None;
        self.result = ScanResult::default();
    }

    /// Searches libraries in `libs` (a `*.kicad_sym` file, a `*.kicad_mod`
    /// file, a `*.pretty` directory or any directory, which is scanned
    /// together with its direct subdirectories) and 3D models in
    /// `shapes_3d` (default: `libs`). Returns `false` if the state is not
    /// [`ImportState::Reset`].
    pub fn scan(
        &mut self,
        libs: &FilePath,
        shapes_3d: Option<&FilePath>,
        log: &MessageLogger<'_>,
        progress: &Progress,
    ) -> bool {
        if self.state != ImportState::Reset {
            log.critical("Unexpected state.");
            return false;
        }
        self.loaded_libs_path = Some(libs.clone());
        self.loaded_shapes_3d_path = shapes_3d.cloned();
        let mut result = ScanResult::default();
        let mut footprint_count = 0;
        let mut step_file_count = 0;

        fn add_footprint_lib(result: &mut ScanResult, dir: &FilePath, count: &mut usize) {
            let files = find_items(dir, false, ".kicad_mod");
            *count += files.len();
            result.file_count += files.len();
            result.footprint_libs.push(FootprintLibrary {
                dir: dir.clone(),
                files,
                footprints: Vec::new(),
            });
        }
        fn find_symbol_libs(result: &mut ScanResult, dir: &FilePath) {
            for fp in find_items(dir, false, ".kicad_sym") {
                result.symbol_libs.push(SymbolLibrary {
                    file: fp,
                    symbols: Vec::new(),
                });
                result.file_count += 1;
            }
        }
        fn find_footprint_libs(result: &mut ScanResult, dir: &FilePath, count: &mut usize) {
            for fp in find_items(dir, true, ".pretty") {
                add_footprint_lib(result, &fp, count);
            }
        }
        fn add_shapes_3d_lib(result: &mut ScanResult, dir: &FilePath, count: &mut usize) {
            let step_files = find_items(dir, false, ".step");
            *count += step_files.len();
            result.file_count += step_files.len();
            result.package_3d_libs.push(Package3dLibrary {
                dir: dir.clone(),
                step_files,
            });
        }

        // Scan selected libraries.
        match libs.suffix().to_lowercase().as_str() {
            "kicad_sym" => {
                // Symbol library selected.
                result.symbol_libs.push(SymbolLibrary {
                    file: libs.clone(),
                    symbols: Vec::new(),
                });
                result.file_count += 1;
            }
            "kicad_mod" => {
                // Footprint selected.
                match libs.parent_dir() {
                    Some(parent) if parent.suffix().to_lowercase() == "pretty" => {
                        footprint_count += 1;
                        result.file_count += 1;
                        result.footprint_libs.push(FootprintLibrary {
                            dir: parent,
                            files: vec![libs.clone()],
                            footprints: Vec::new(),
                        });
                    }
                    _ => log.critical("Parent directory is not a *.pretty library."),
                }
            }
            "pretty" => add_footprint_lib(&mut result, libs, &mut footprint_count),
            _ if libs.is_existing_dir() => {
                // Any other directory selected, scan it for content.
                find_symbol_libs(&mut result, libs);
                find_footprint_libs(&mut result, libs, &mut footprint_count);
                // Scan subdirectories for libraries (not recursive).
                for sub in find_items(libs, true, "") {
                    find_symbol_libs(&mut result, &sub);
                    find_footprint_libs(&mut result, &sub, &mut footprint_count);
                }
            }
            _ => {}
        }

        // Look for 3D models.
        let models_fp = shapes_3d.unwrap_or(libs);
        if models_fp.suffix().to_lowercase() == "3dshapes" {
            add_shapes_3d_lib(&mut result, models_fp, &mut step_file_count);
        } else {
            for fp in find_items(models_fp, true, ".3dshapes") {
                add_shapes_3d_lib(&mut result, &fp, &mut step_file_count);
            }
            for sub in find_items(models_fp, true, "") {
                for fp in find_items(&sub, true, ".3dshapes") {
                    add_shapes_3d_lib(&mut result, &fp, &mut step_file_count);
                }
            }
        }

        // Finished! Report status.
        log.info(&tr!(
            CTX,
            "Found {0} symbol libraries.",
            result.symbol_libs.len()
        ));
        log.info(&tr!(
            CTX,
            "Found {0} footprints in {1} libraries.",
            footprint_count,
            result.footprint_libs.len()
        ));
        log.info(&tr!(
            CTX,
            "Found {0} STEP files in {1} libraries.",
            step_file_count,
            result.package_3d_libs.len()
        ));
        self.result = result;
        self.state = if progress.is_canceled() {
            ImportState::Reset
        } else {
            ImportState::Scanned
        };
        true
    }

    /// Parses the found libraries and lists their elements (all checked).
    /// Returns `false` if the state is not [`ImportState::Scanned`].
    pub fn parse(
        &mut self,
        lookup: &dyn ImportedElementLookup,
        log: &MessageLogger<'_>,
        progress: &Progress,
    ) -> bool {
        if self.state != ImportState::Scanned {
            log.critical("Unexpected state.");
            return false;
        }
        log.info(&tr!(CTX, "Parsing libraries..."));
        progress.set_percent(5);
        let already = |kind: DbKind, gen_by: &str| !lookup.generated(kind, gen_by).is_empty();

        // Load symbols.
        let mut symbol_count = 0;
        let lib_count = self.result.symbol_libs.len();
        for (i, lib) in self.result.symbol_libs.iter_mut().enumerate() {
            lib.symbols.clear(); // Might be a leftover from previous run.
            if progress.is_canceled() {
                break;
            }
            let lib_name = lib.file.complete_basename().to_owned();
            let sym_log = MessageLogger::child(log, &lib_name);
            let parsed = file_utils::read_file(&lib.file)
                .map_err(Into::into)
                .and_then(|c| KiCadSymbolLibrary::parse_file(&c, lib.file.as_path(), &sym_log));
            match parsed {
                Ok(ki_lib) => {
                    for ki_symbol in &ki_lib.symbols {
                        let cmp_generated_by = generated_by(
                            &lib_name,
                            &[if ki_symbol.extends.is_empty() {
                                &ki_symbol.name
                            } else {
                                &ki_symbol.extends
                            }],
                        );
                        let dev_generated_by = generated_by(&lib_name, &[&ki_symbol.name]);
                        let footprint =
                            KiCadTypeConverter::find_property(&ki_symbol.properties, "footprint")
                                .map(|p| p.value.trim().to_owned())
                                .unwrap_or_default();
                        let pkg_generated_by = if footprint.is_empty() {
                            String::new()
                        } else {
                            let split: Vec<&str> = footprint.split(':').collect();
                            generated_by(split[0], &split[1..])
                        };
                        if !ki_symbol.extends.is_empty() && !ki_symbol.gates.is_empty() {
                            sym_log.critical(&format!(
                                "Symbol '{}' extends another symbol and contains gates.",
                                ki_symbol.name
                            ));
                            continue;
                        } else if ki_symbol.extends.is_empty() && ki_symbol.gates.is_empty() {
                            sym_log.critical(&format!(
                                "Symbol '{}' does not contain any gates.",
                                ki_symbol.name
                            ));
                            continue;
                        }
                        let mut sym = ImportSymbol {
                            name: ki_symbol.name.clone(),
                            cmp_already_imported: already(DbKind::Component, &cmp_generated_by),
                            dev_already_imported: already(DbKind::Device, &dev_generated_by),
                            cmp_generated_by,
                            dev_generated_by,
                            pkg_generated_by,
                            sym_already_imported: true, // Might be set to false below.
                            extends: ki_symbol.extends.clone(),
                            gates: Vec::new(),
                            sym_checked: CheckState::Checked,
                            cmp_checked: CheckState::Checked,
                            dev_checked: CheckState::Checked,
                        };
                        for gate in merge_symbol_gates(&ki_symbol.gates, &ki_symbol.name) {
                            let gen_by = generated_by(
                                &lib_name,
                                &[&ki_symbol.name, &gate.index.to_string()],
                            );
                            let already_imported = already(DbKind::Symbol, &gen_by);
                            if !already_imported {
                                sym.sym_already_imported = false;
                            }
                            sym.gates.push(ImportGate {
                                index: gate.index,
                                sym_generated_by: gen_by,
                                already_imported,
                            });
                        }
                        lib.symbols.push(sym);
                        symbol_count += 1;
                    }
                }
                Err(e) => sym_log.critical(&format!(
                    "Failed to parse symbol library '{}': {e}",
                    lib.file.file_name()
                )),
            }
            progress.set_percent((5 + 45 * (i + 1) / lib_count.max(1)) as u32);
        }

        // Load footprints.
        let mut footprint_count = 0;
        let lib_count = self.result.footprint_libs.len();
        for (i, lib) in self.result.footprint_libs.iter_mut().enumerate() {
            lib.footprints.clear(); // Might be a leftover from previous run.
            let lib_name = lib.dir.complete_basename().to_owned();
            for fpt_fp in &lib.files {
                if progress.is_canceled() {
                    break;
                }
                let fpt_log = MessageLogger::child(
                    log,
                    &format!("{lib_name}:{}", fpt_fp.complete_basename()),
                );
                let parsed = file_utils::read_file(fpt_fp)
                    .map_err(Into::into)
                    .and_then(|c| KiCadFootprint::parse_file(&c, fpt_fp.as_path(), &fpt_log));
                match parsed {
                    Ok(ki_fpt) => {
                        let pkg_generated_by =
                            generated_by(&lib_name, &[fpt_fp.complete_basename()]);
                        lib.footprints.push(ImportFootprint {
                            file: fpt_fp.clone(),
                            name: ki_fpt.name,
                            already_imported: already(DbKind::Package, &pkg_generated_by),
                            generated_by: pkg_generated_by,
                            checked: CheckState::Checked,
                        });
                        footprint_count += 1;
                    }
                    Err(e) => fpt_log.critical(&format!(
                        "Failed to parse footprint '{}:{}': {e}",
                        lib.dir.file_name(),
                        fpt_fp.file_name()
                    )),
                }
            }
            progress.set_percent((50 + 45 * (i + 1) / lib_count.max(1)) as u32);
        }

        if progress.is_canceled() {
            log.info(&tr!(CTX, "Aborted."));
            self.state = ImportState::Scanned;
        } else {
            log.info(&tr!(
                CTX,
                "Found {0} symbols and {1} footprints.",
                symbol_count,
                footprint_count
            ));
            if symbol_count + footprint_count > 1000 {
                log.warning(&tr!(
                    CTX,
                    "Due to the large amount of elements, please be patient during the \
                     following steps."
                ));
            }
            log.info(&tr!(
                CTX,
                "Please review the messages (if any) before continuing."
            ));
            self.state = ImportState::Parsed;
        }
        progress.set_percent(100);
        true
    }

    /// Converts the selected elements and writes them into the destination
    /// library. Returns `None` if the state is not [`ImportState::Parsed`].
    pub fn import(
        &mut self,
        lookup: &dyn ImportedElementLookup,
        log: &MessageLogger<'_>,
        progress: &Progress,
    ) -> Option<ImportSummary> {
        if self.state != ImportState::Parsed {
            log.critical("Unexpected state.");
            return None;
        }
        log.info(&tr!(CTX, "Importing libraries..."));
        progress.set_percent(5);

        let mut converter = KiCadLibraryConverter::new(lookup, self.settings.clone());
        let total = self.count_to_import();
        let mut processed = 0;
        let mut imported = 0;
        let report = |processed: usize| {
            progress.set_percent(((100 * processed) / total.max(1)) as u32);
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
        let names = ("KiCad Import", CATEGORY_NAME, "kicad");
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

        // Import packages.
        let mut missing_3d_shape_libs = BTreeSet::new();
        'libs: for lib in &self.result.footprint_libs {
            let lib_name = lib.dir.complete_basename();
            for fpt in &lib.footprints {
                if progress.is_canceled() {
                    break 'libs;
                }
                if !fpt.checked.is_imported() || fpt.already_imported {
                    continue;
                }
                let name = format!("{lib_name}:{}", fpt.file.complete_basename());
                let fpt_log = MessageLogger::child(log, &name);
                progress.set_status(name);
                let result = file_utils::read_file(&fpt.file)
                    .map_err(Into::into)
                    .and_then(|c| KiCadFootprint::parse_file(&c, fpt.file.as_path(), &fpt_log))
                    .and_then(|ki_fpt| {
                        // Find 3D models.
                        let mut models = BTreeMap::new();
                        for model in &ki_fpt.models {
                            let segments: Vec<&str> = model.path.split('/').collect();
                            let n = segments.len();
                            let model_lib = if n >= 2 { segments[n - 2] } else { "" };
                            let file_name = segments[n - 1].replace(".wrl", ".step");
                            if !model_lib.ends_with(".3dshapes") || !file_name.ends_with(".step") {
                                fpt_log
                                    .warning(&format!("Unknown 3D model file: '{}'", model.path));
                                continue;
                            }
                            let mut lib_found = false;
                            for lib3d in self
                                .result
                                .package_3d_libs
                                .iter()
                                .filter(|l| l.dir.file_name() == model_lib)
                            {
                                lib_found = true;
                                for fp in &lib3d.step_files {
                                    if fp.file_name() == file_name {
                                        models.insert(model.path.clone(), fp.clone());
                                    }
                                }
                            }
                            if !lib_found {
                                missing_3d_shape_libs.insert(model_lib.to_owned());
                            }
                        }
                        // Create package.
                        let mut package = converter.create_package(
                            &lib.dir,
                            &ki_fpt,
                            &fpt.generated_by,
                            &models,
                            &fpt_log,
                        )?;
                        save_element(&self.destination, &mut package)?;
                        Ok(())
                    });
                match result {
                    Ok(()) => imported += 1,
                    Err(e) => fpt_log.critical(&tr!(CTX, "Skipped footprint due to error: {0}", e)),
                }
                processed += 1;
                report(processed);
            }
        }

        // Import symbols, components & devices.
        for lib in &self.result.symbol_libs {
            if progress.is_canceled() {
                break;
            }
            let lib_name = lib.file.complete_basename();
            let lib_log = MessageLogger::child(log, lib_name);
            let result = file_utils::read_file(&lib.file)
                .map_err(Into::into)
                .and_then(|c| KiCadSymbolLibrary::parse_file(&c, lib.file.as_path(), &lib_log))
                .map(|ki_lib| {
                    self.import_symbol_library(
                        lib,
                        &ki_lib,
                        &mut converter,
                        &lib_log,
                        progress,
                        &mut |ok| {
                            imported += usize::from(ok);
                            processed += 1;
                            report(processed);
                        },
                    );
                });
            if let Err(e) = result {
                lib_log.critical(&tr!(CTX, "Skipped symbol library due to error: {0}", e));
            }
        }

        // Warn about missing 3D shape libraries.
        for lib_name in missing_3d_shape_libs {
            log.info(&format!("3D model library not found: '{lib_name}'"));
        }

        let canceled = progress.is_canceled();
        if canceled {
            log.info(&tr!(CTX, "Aborted."));
            self.state = ImportState::Scanned; // Parse result not valid anymore!
        } else {
            log.info(&tr!(
                CTX,
                "Done! Please check all messages (if any) before proceeding."
            ));
            log.info(&tr!(
                CTX,
                "Note that the importer might not cover all cases correctly yet."
            ));
            log.info(&tr!(
                CTX,
                "If you experience any issue, please <a href=\"{0}\">let us know</a>. Thanks!",
                "https://librepcb.org/help/"
            ));
            self.state = ImportState::Imported;
        }
        progress.set_percent(100);
        progress.set_status(trn!(
            CTX,
            "Finished: {0} of {1} element(s) imported",
            "Finished: {0} of {1} element(s) imported",
            total,
            imported,
            total
        ));
        Some(ImportSummary {
            total,
            processed,
            imported,
            canceled,
        })
    }

    fn import_symbol_library(
        &self,
        lib: &SymbolLibrary,
        ki_lib: &KiCadSymbolLibrary,
        converter: &mut KiCadLibraryConverter<'_>,
        lib_log: &MessageLogger<'_>,
        progress: &Progress,
        done: &mut dyn FnMut(bool),
    ) {
        let lib_name = lib.file.complete_basename();
        if lib.symbols.len() != ki_lib.symbols.len() {
            lib_log.critical(&tr!(
                CTX,
                "Skipped symbol library due to error: {0}",
                "symbol count mismatch"
            ));
            return;
        }
        for (sym, ki_sym) in lib.symbols.iter().zip(&ki_lib.symbols) {
            let sym_log = MessageLogger::child(lib_log, &ki_sym.name);
            let mut ki_gates = merge_symbol_gates(&ki_sym.gates, &ki_sym.name);
            if sym.gates.len() != ki_gates.len() {
                lib_log.critical(&tr!(
                    CTX,
                    "Skipped symbol library due to error: {0}",
                    "gate count mismatch"
                ));
                return;
            }

            // Import gates as symbols.
            for (gate, ki_gate) in sym.gates.iter().zip(&ki_gates) {
                if progress.is_canceled() {
                    break;
                }
                if !sym.sym_checked.is_imported()
                    || gate.already_imported
                    || !ki_sym.extends.is_empty()
                {
                    continue;
                }
                let gate_log = MessageLogger::child(&sym_log, &ki_gate.index.to_string());
                progress.set_status(format!("{lib_name}:{}", ki_gate.name));
                let result = converter
                    .create_symbol(
                        &lib.file,
                        ki_sym,
                        ki_gate,
                        &gate.sym_generated_by,
                        &gate_log,
                    )
                    .and_then(|mut e| Ok(save_element(&self.destination, &mut e)?));
                if let Err(e) = &result {
                    gate_log.critical(&tr!(CTX, "Skipped symbol due to error: {0}", e));
                }
                done(result.is_ok());
            }

            // Import symbol as component, if it's not extending another
            // symbol.
            if sym.cmp_checked.is_imported()
                && !sym.cmp_already_imported
                && ki_sym.extends.is_empty()
            {
                if progress.is_canceled() {
                    return;
                }
                progress.set_status(format!("{lib_name}:{}:CMP", ki_sym.name));
                let sym_generated_by: Vec<String> = sym
                    .gates
                    .iter()
                    .map(|g| g.sym_generated_by.clone())
                    .collect();
                let result = converter
                    .create_component(
                        &lib.file,
                        ki_sym,
                        &ki_gates,
                        &sym.cmp_generated_by,
                        &sym_generated_by,
                        &sym_log,
                    )
                    .and_then(|mut e| Ok(save_element(&self.destination, &mut e)?));
                if let Err(e) = &result {
                    sym_log.critical(&tr!(CTX, "Skipped component due to error: {0}", e));
                }
                done(result.is_ok());
            }

            // Import symbol as device.
            if sym.dev_checked.is_imported()
                && !sym.dev_already_imported
                && !sym.pkg_generated_by.is_empty()
            {
                if progress.is_canceled() {
                    return;
                }
                progress.set_status(format!("{lib_name}:{}:DEV", ki_sym.name));
                if !ki_sym.extends.is_empty() {
                    // Note: Like upstream, a missing base symbol is not
                    // reported here (the device then has no pad mappings).
                    for base in ki_lib.symbols.iter().filter(|s| s.name == ki_sym.extends) {
                        ki_gates = merge_symbol_gates(&base.gates, &base.name);
                    }
                }
                let result = converter
                    .create_device(
                        &lib.file,
                        ki_sym,
                        &ki_gates,
                        &sym.dev_generated_by,
                        &sym.cmp_generated_by,
                        &sym.pkg_generated_by,
                        &sym_log,
                    )
                    .and_then(|mut e| Ok(save_element(&self.destination, &mut e)?));
                if let Err(e) = &result {
                    sym_log.critical(&tr!(CTX, "Skipped device due to error: {0}", e));
                }
                done(result.is_ok());
            }
        }
    }

    fn update_dependencies(&mut self) -> Vec<CheckStateChange> {
        let mut changes = Vec::new();
        let mut dependent_packages = BTreeSet::new();
        let mut dependent_components = BTreeSet::new();
        for sym in self.result.symbol_libs.iter().flat_map(|l| &l.symbols) {
            if sym.dev_checked.is_imported()
                && !sym.dev_already_imported
                && !sym.pkg_generated_by.is_empty()
            {
                dependent_components.insert(sym.cmp_generated_by.clone());
                dependent_packages.insert(sym.pkg_generated_by.clone());
            }
        }
        for lib in &mut self.result.symbol_libs {
            let lib_name = lib.file.complete_basename().to_owned();
            for sym in lib.symbols.iter_mut().filter(|s| s.extends.is_empty()) {
                if sym
                    .cmp_checked
                    .set_dependent(dependent_components.contains(&sym.cmp_generated_by))
                {
                    changes.push(CheckStateChange::new(
                        ElementKind::Component,
                        &lib_name,
                        &sym.name,
                        sym.cmp_checked,
                    ));
                }
                if sym.sym_checked.set_dependent(sym.cmp_checked.is_imported()) {
                    changes.push(CheckStateChange::new(
                        ElementKind::Symbol,
                        &lib_name,
                        &sym.name,
                        sym.sym_checked,
                    ));
                }
            }
        }
        for lib in &mut self.result.footprint_libs {
            let lib_name = lib.dir.complete_basename().to_owned();
            for fpt in &mut lib.footprints {
                if fpt
                    .checked
                    .set_dependent(dependent_packages.contains(&fpt.generated_by))
                {
                    changes.push(CheckStateChange::new(
                        ElementKind::Package,
                        &lib_name,
                        &fpt.name,
                        fpt.checked,
                    ));
                }
            }
        }
        changes
    }

    /// UUID of the component category created for imported symbols,
    /// components and devices.
    pub fn component_category_uuid() -> Uuid {
        "7bbabe3b-da71-423e-8e50-fcdbce820fac"
            .parse()
            .expect("valid UUID")
    }

    /// UUID of the package category created for imported packages.
    pub fn package_category_uuid() -> Uuid {
        "52a596cb-7d7e-446a-944c-f631be387ec9"
            .parse()
            .expect("valid UUID")
    }
}
