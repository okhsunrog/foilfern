//! Port of libs/librepcb/eagleimport/eagleprojectimport.{h,cpp}.
//!
//! Imports an EAGLE schematic (and optionally its board) into a (new)
//! LibrePCB project: the elements of the embedded libraries are converted
//! into the project library, parts become component instances, sheets
//! become schematics (symbols, graphics, texts, buses and nets), and the
//! board gets the devices, graphics, keepout zones, holes, traces, vias and
//! planes, plus the layer setup, design rules and DRC settings.
//!
//! All changes are applied with [`Mutation`]s. UUIDs are created in the same
//! order as upstream, so with a deterministic [`UuidGenerator`] the result
//! is identical to upstream's golden sample.
//!
//! Differences to upstream:
//! - Not a `QObject`; [`EagleProjectImport::import()`] takes the logger.
//! - Footprint pads attached to traces are looked up in the order the
//!   component signals were connected to the net (upstream: registration
//!   order of `NetSignal::getComponentSignals()`, which is the same).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use librepcb_core::attribute::{Attribute, AttributeKey, AttributeList, AttributeType};
use librepcb_core::fileio::{CleanFileNameOptions, FileNameCase, FilePath};
use librepcb_core::geometry::{
    Junction, NetLabel, NetLine, NetLineAnchor, Text, Trace, TraceAnchor, Via, ZoneLayers,
    ZoneRules,
};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::dev::{Part, PartList};
use librepcb_core::project::board::{
    Board, BoardDevice, BoardItem, BoardNetSegment, BoardNetSegmentSplitter, BoardPlane,
    BoardPolygonData, BoardStrokeTextData, BoardZoneData, PlaneConnectStyle,
};
use librepcb_core::project::circuit::{Bus, ComponentAssemblyOption, ComponentInstance, NetSignal};
use librepcb_core::project::schematic::{
    Schematic, SchematicBusSegment, SchematicNetSegment, SchematicNetSegmentSplitter,
    SchematicSymbol,
};
use librepcb_core::project::{
    AssemblyVariantId, BoardId, BoardMutation, BusId, ComponentInstanceId, ComponentSignalRef,
    Mutation, NetSignalId, Project, SchematicId, SchematicMutation,
};
use librepcb_core::types::{
    BoundedUnsignedRatio, BusName, CircuitIdentifier, ElementName, Layer, Length, MaskConfig,
    Point, PositiveLength, SimpleString, UnsignedLength, Uuid,
};
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_core::utils::toolbox;
use librepcb_core::utils::transform::Transform;
use librepcb_i18n::{tr, trn};

use super::error::{Error, Result};
use super::library_converter::{EagleLibraryConverter, EagleLibraryConverterSettings};
use super::model;
use super::type_converter::EagleTypeConverter;
use crate::UuidGenerator;

const CTX: &str = "librepcb::eagleimport::EagleProjectImport";

type LibKey = (String, String, String);

fn key(lib: &str, urn: &str, name: &str) -> LibKey {
    (lib.to_owned(), urn.to_owned(), name.to_owned())
}

fn runtime(msg: String) -> Error {
    Error::Other(msg)
}

/// The component instance created for a part.
#[derive(Debug, Clone)]
struct ComponentMapEntry {
    lib_name: String,
    lib_urn: String,
    dev_set_name: String,
    dev_name: String,
    uuid: ComponentInstanceId,
}

/// Import state (upstream members filled during `import()`).
#[derive(Default)]
struct State {
    lib_symbol_map: HashMap<LibKey, Uuid>,
    lib_component_map: HashMap<LibKey, Uuid>,
    /// Key: (library component UUID, gate name).
    lib_component_gate_map: HashMap<(Uuid, String), Uuid>,
    lib_package_map: HashMap<LibKey, Uuid>,
    /// Key: (library name, URN, device set name, device name).
    lib_device_map: HashMap<(String, String, String, String), Uuid>,
    component_map: HashMap<String, ComponentMapEntry>,
    schematic_dir_names: HashSet<String>,
    net_signal_map: HashMap<String, NetSignalId>,
    bus_map: HashMap<String, BusId>,
    /// Component signals connected to each net, in connection order.
    net_component_signals: HashMap<NetSignalId, Vec<ComponentSignalRef>>,
}

/// The EAGLE project import (upstream `EagleProjectImport`).
#[derive(Debug)]
pub struct EagleProjectImport {
    uuids: UuidGenerator,
    created: DateTime<Utc>,
    tc: EagleTypeConverter,
    project_name: String,
    schematic: Option<model::Schematic>,
    board: Option<model::Board>,
    symbols: HashMap<LibKey, model::Symbol>,
    packages: HashMap<LibKey, model::Package>,
    device_sets: HashMap<LibKey, model::DeviceSet>,
}

impl Default for EagleProjectImport {
    /// Random UUIDs, the current time as creation date.
    fn default() -> Self {
        Self::new(UuidGenerator::random(), Utc::now())
    }
}

impl EagleProjectImport {
    /// Creates an import creating UUIDs with `uuids` and using `created` as
    /// creation date of the library elements.
    pub fn new(uuids: UuidGenerator, created: DateTime<Utc>) -> Self {
        Self {
            tc: EagleTypeConverter::new(uuids.clone()),
            uuids,
            created,
            project_name: String::new(),
            schematic: None,
            board: None,
            symbols: HashMap::new(),
            packages: HashMap::new(),
            device_sets: HashMap::new(),
        }
    }

    /// Returns whether a project is opened.
    pub fn is_ready(&self) -> bool {
        self.schematic.is_some()
    }

    /// Returns the project name (base name of the schematic file).
    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    /// Returns the number of schematic sheets.
    pub fn sheet_count(&self) -> usize {
        self.schematic.as_ref().map_or(0, |s| s.sheets.len())
    }

    /// Returns whether a board is opened.
    pub fn has_board(&self) -> bool {
        self.board.is_some()
    }

    /// Forgets the opened project.
    pub fn reset(&mut self) {
        self.device_sets.clear();
        self.packages.clear();
        self.symbols.clear();
        self.board = None;
        self.schematic = None;
        self.project_name.clear();
    }

    /// Opens a schematic (`*.sch`) and optionally a board (`*.brd`).
    /// Returns (non-fatal) warnings.
    pub fn open(&mut self, sch: &FilePath, brd: Option<&FilePath>) -> Result<Vec<String>> {
        self.reset();
        let mut warnings = Vec::new();
        let read = |fp: &FilePath| -> Result<Vec<u8>> {
            if !fp.is_existing_file() {
                return Err(Error::FileNotFound(fp.to_native()));
            }
            std::fs::read(fp.as_path()).map_err(|e| Error::Io {
                path: fp.to_native(),
                message: e.to_string(),
            })
        };
        let schematic = model::Schematic::from_bytes(&read(sch)?, &mut warnings)?;
        if schematic.sheets.is_empty() {
            warnings.push(tr!(CTX, "Project contains no schematic sheets."));
        }
        if !schematic.modules.is_empty() {
            warnings.push(tr!(
                CTX,
                "Project contains modules which are not supported yet!"
            ));
        }
        let board = brd
            .map(|brd| model::Board::from_bytes(&read(brd)?, &mut warnings))
            .transpose()?;

        self.project_name = sch.complete_basename().to_owned();
        self.import_libraries(&schematic.libraries, false);
        if let Some(board) = &board {
            self.import_libraries(&board.libraries, true);
        }
        self.schematic = Some(schematic);
        self.board = board;
        Ok(warnings)
    }

    fn import_libraries(&mut self, libs: &[model::Library], is_board: bool) {
        for lib in libs {
            let (name, urn) = (&lib.embedded_name, &lib.embedded_urn);
            for sym in &lib.symbols {
                let k = key(name, urn, &sym.name);
                if !self.symbols.contains_key(&k) || !is_board {
                    self.symbols.insert(k, sym.clone());
                }
            }
            for pkg in &lib.packages {
                let k = key(name, urn, &pkg.name);
                if !self.packages.contains_key(&k) || is_board {
                    self.packages.insert(k, pkg.clone());
                }
            }
            for dev in &lib.device_sets {
                let k = key(name, urn, &dev.name);
                if !self.device_sets.contains_key(&k) || !is_board {
                    self.device_sets.insert(k, dev.clone());
                }
            }
        }
    }

    /// Imports the opened project into `project` (usually a new, empty
    /// project). Messages are logged to `log`.
    pub fn import(&self, project: &mut Project, log: &MessageLogger<'_>) -> Result<()> {
        let result = self.import_impl(project, log);
        if let Err(e) = &result {
            log.critical(&format!("{} {e}", tr!(CTX, "Import failed:")));
        }
        result
    }

    fn uuid(&self) -> Uuid {
        self.uuids.generate()
    }

    fn import_impl(&self, project: &mut Project, log: &MessageLogger<'_>) -> Result<()> {
        let schematic = self
            .schematic
            .as_ref()
            .ok_or(Error::Other("No project opened.".to_owned()))?;
        log.info(&tr!(CTX, "Importing project, this may take a moment..."));
        log.info(&tr!(
            CTX,
            "If you experience any issues with the import, please <a href=\"{0}\">let us \
             know</a> so we can improve it.",
            "https://librepcb.org/help/"
        ));

        // Try to apply the automatic THT annular width to get correct pad
        // sizes in footprints.
        let mut settings = EagleLibraryConverterSettings {
            uuids: self.uuids.clone(),
            created: self.created,
            ..EagleLibraryConverterSettings::default()
        };
        match self.try_get_drc_ratio("rvPadBottom", "rlMinPadBottom", "rlMaxPadBottom") {
            Ok(Some(r)) => settings.auto_tht_annular_width = r,
            Ok(None) => {}
            Err(e) => log.critical(&format!("Could not configure automatic pad sizes: {e}")),
        }
        let mut converter = EagleLibraryConverter::new(settings);
        let mut state = State::default();

        // Add components.
        for part in &schematic.parts {
            let plog = MessageLogger::child(log, &part.name);
            let lib_cmp_uuid = self.import_library_component(
                &mut converter,
                &mut state,
                project,
                &part.library,
                &part.library_urn,
                &part.device_set,
                log,
            )?;
            let lib_cmp = project
                .library()
                .component(&lib_cmp_uuid)
                .ok_or(Error::Other("Component not in project library.".to_owned()))?;
            let symb_var = lib_cmp
                .symbol_variants()
                .first()
                .ok_or(Error::Other("Component without symbol variant.".to_owned()))?
                .uuid();
            let mut name = CircuitIdentifier::clean(&part.name);
            if name.is_empty() {
                name = lib_cmp_uuid.to_string()[..8].to_owned(); // Not so nice...
            }
            let mut cmp = ComponentInstance::new(
                self.uuid(),
                lib_cmp,
                symb_var,
                CircuitIdentifier::new(name)?,
                false,
            )?;
            if !part.value.is_empty() {
                cmp.set_value(part.value.clone());
            }
            let mut attributes = cmp.attributes().clone();
            self.tc
                .try_convert_attributes(&part.attributes, &mut attributes, &plog);
            let eagle_dev_set =
                self.device_set(&part.library, &part.library_urn, &part.device_set)?;
            let eagle_dev = device(eagle_dev_set, &part.device)?;
            if let Some(tech) = eagle_dev
                .technologies
                .iter()
                .find(|t| t.name == part.technology)
            {
                self.tc
                    .try_convert_attributes(&tech.attributes, &mut attributes, &plog);
                if !attributes.contains_name("MPN")
                    && !attributes.contains_name("MANUFACTURER_PART_NUMBER")
                    && !attributes.contains_name("PART_NUMBER")
                    && !tech.name.trim().is_empty()
                {
                    // Memorize it since it could be an MPN.
                    attributes.push(Attribute::new(
                        AttributeKey::new("EAGLE_TECHNOLOGY")?,
                        AttributeType::String,
                        tech.name.clone(),
                        None,
                    )?);
                }
            }
            cmp.set_attributes(attributes);
            state.component_map.insert(
                part.name.clone(),
                ComponentMapEntry {
                    lib_name: part.library.clone(),
                    lib_urn: part.library_urn.clone(),
                    dev_set_name: part.device_set.clone(),
                    dev_name: part.device.clone(),
                    uuid: cmp.id(),
                },
            );
            project.apply(Mutation::AddComponentInstance(cmp))?;
        }

        // Warn about unsupported objects.
        if !schematic.modules.is_empty() {
            log.critical(&tr!(
                CTX,
                "Skipped modules because they are not supported yet!"
            ));
        }

        // Import schematics.
        for sheet in &schematic.sheets {
            self.import_schematic(project, &converter, &mut state, sheet, log)?;
        }

        // Import board, if given.
        if self.board.is_some() {
            self.import_board(project, &mut converter, &mut state, log)?;
        }

        // Status messages.
        log.info(&trn!(
            CTX,
            "Imported {n} schematic sheet(s). Please check the ERC messages in the schematic \
             editor.",
            "Imported {n} schematic sheet(s). Please check the ERC messages in the schematic \
             editor.",
            schematic.sheets.len()
        ));
        if self.board.is_some() {
            log.info(&tr!(
                CTX,
                "Imported a board. Please run the DRC in the board editor and fix remaining \
                 issues manually."
            ));
        }
        Ok(())
    }

    // --- Library elements ---

    #[allow(clippy::too_many_arguments)]
    fn import_library_symbol(
        &self,
        converter: &mut EagleLibraryConverter,
        state: &mut State,
        project: &mut Project,
        lib: &str,
        urn: &str,
        sym_name: &str,
        log: &MessageLogger<'_>,
    ) -> Result<Uuid> {
        let k = key(lib, urn, sym_name);
        if let Some(uuid) = state.lib_symbol_map.get(&k) {
            return Ok(*uuid);
        }
        let eagle_symbol = self.symbol(lib, urn, sym_name)?;
        let slog = MessageLogger::child(log, &eagle_symbol.name);
        let sym = converter.create_symbol(lib, urn, eagle_symbol, &slog)?;
        let uuid = sym.metadata().uuid();
        state.lib_symbol_map.insert(k, uuid);
        project.add_library_symbol(sym)?;
        Ok(uuid)
    }

    #[allow(clippy::too_many_arguments)]
    fn import_library_component(
        &self,
        converter: &mut EagleLibraryConverter,
        state: &mut State,
        project: &mut Project,
        lib: &str,
        urn: &str,
        dev_set_name: &str,
        log: &MessageLogger<'_>,
    ) -> Result<Uuid> {
        let k = key(lib, urn, dev_set_name);
        if let Some(uuid) = state.lib_component_map.get(&k) {
            return Ok(*uuid);
        }
        let eagle_dev_set = self.device_set(lib, urn, dev_set_name)?;
        for gate in &eagle_dev_set.gates {
            self.import_library_symbol(converter, state, project, lib, urn, &gate.symbol, log)?;
        }
        let clog = MessageLogger::child(log, &eagle_dev_set.name);
        let cmp = converter.create_component(lib, urn, eagle_dev_set, &clog)?;
        let items = cmp
            .symbol_variants()
            .first()
            .map(|v| v.symbol_items().uuids())
            .unwrap_or_default();
        if (cmp.symbol_variants().len() != 1) || (items.len() != eagle_dev_set.gates.len()) {
            return Err(Error::Other(
                "Unexpected symbol variant structure.".to_owned(),
            ));
        }
        let cmp_uuid = cmp.metadata().uuid();
        for (gate, item) in eagle_dev_set.gates.iter().zip(items) {
            state
                .lib_component_gate_map
                .insert((cmp_uuid, gate.name.clone()), item);
        }
        state.lib_component_map.insert(k, cmp_uuid);
        project.add_library_component(cmp)?;
        Ok(cmp_uuid)
    }

    #[allow(clippy::too_many_arguments)]
    fn import_library_package(
        &self,
        converter: &mut EagleLibraryConverter,
        state: &mut State,
        project: &mut Project,
        lib: &str,
        urn: &str,
        pkg_name: &str,
        log: &MessageLogger<'_>,
    ) -> Result<Uuid> {
        let k = key(lib, urn, pkg_name);
        if let Some(uuid) = state.lib_package_map.get(&k) {
            return Ok(*uuid);
        }
        let eagle_package = self.package(lib, urn, pkg_name)?;
        let plog = MessageLogger::child(log, &eagle_package.name);
        let pkg = converter.create_package(lib, urn, eagle_package, &plog)?;
        let uuid = pkg.metadata().uuid();
        state.lib_package_map.insert(k, uuid);
        project.add_library_package(pkg)?;
        Ok(uuid)
    }

    #[allow(clippy::too_many_arguments)]
    fn import_library_device(
        &self,
        converter: &mut EagleLibraryConverter,
        state: &mut State,
        project: &mut Project,
        entry: &ComponentMapEntry,
        pkg_lib: &str,
        pkg_urn: &str,
        log: &MessageLogger<'_>,
    ) -> Result<Uuid> {
        let k = (
            entry.lib_name.clone(),
            entry.lib_urn.clone(),
            entry.dev_set_name.clone(),
            entry.dev_name.clone(),
        );
        if let Some(uuid) = state.lib_device_map.get(&k) {
            return Ok(*uuid);
        }
        let eagle_dev_set =
            self.device_set(&entry.lib_name, &entry.lib_urn, &entry.dev_set_name)?;
        let eagle_dev = device(eagle_dev_set, &entry.dev_name)?;
        self.import_library_package(
            converter,
            state,
            project,
            pkg_lib,
            pkg_urn,
            &eagle_dev.package,
            log,
        )?;
        let dlog = MessageLogger::child(log, &eagle_dev.name);
        let dev = converter.create_device(
            &entry.lib_name,
            &entry.lib_urn,
            eagle_dev_set,
            eagle_dev,
            pkg_lib,
            pkg_urn,
            &dlog,
        )?;
        let uuid = dev.metadata().uuid();
        state.lib_device_map.insert(k, uuid);
        project.add_library_device(dev)?;
        Ok(uuid)
    }

    // --- Circuit ---

    fn import_net(
        &self,
        project: &mut Project,
        state: &mut State,
        net: &model::Net,
    ) -> Result<NetSignalId> {
        if let Some(id) = state.net_signal_map.get(&net.name) {
            return Ok(*id);
        }
        let uuid = self.uuid();
        let uuid_str = uuid.to_string();
        let mut name = self
            .tc
            .convert_inversion_syntax(&CircuitIdentifier::clean(&net.name));
        if name.is_empty() {
            name = uuid_str[..8].to_owned();
        } else if project.circuit().net_signal_by_name(&name).is_some() {
            name = format!("{}_{}", truncate(&name, 20), &uuid_str[..8]);
        }
        let net_classes = project.circuit().net_classes();
        if net_classes.len() != 1 {
            return Err(Error::Other("Unexpected count of net classes.".to_owned()));
        }
        let net_class = *net_classes.keys().next().expect("one net class exists");
        let signal = NetSignal::new(uuid, net_class, CircuitIdentifier::new(name)?, true);
        project.apply(Mutation::AddNetSignal(signal))?;
        let id = NetSignalId(uuid);
        state.net_signal_map.insert(net.name.clone(), id);
        Ok(id)
    }

    fn import_bus(
        &self,
        project: &mut Project,
        state: &mut State,
        eagle_bus_name: &str,
    ) -> Result<BusId> {
        if let Some(id) = state.bus_map.get(eagle_bus_name) {
            return Ok(*id);
        }
        let uuid = self.uuid();
        let uuid_str = uuid.to_string();
        // Eagle bus names use format "ALIAS" or "ALIAS:net1,net2" or
        // "ALIAS:net[i..j]" where "ALIAS:" is optional. If present, use it
        // as name.
        let mut name = BusName::clean(eagle_bus_name.split(':').next().unwrap_or_default());
        if name.is_empty() {
            name = uuid_str[..8].to_owned();
        } else if project.circuit().bus_by_name(&name).is_some() {
            name = format!("{}_{}", truncate(&name, 20), &uuid_str[..8]);
        }
        let bus = Bus::new(uuid, BusName::new(name)?, true, false, None);
        project.apply(Mutation::AddBus(bus))?;
        let id = BusId(uuid);
        state.bus_map.insert(eagle_bus_name.to_owned(), id);
        Ok(id)
    }

    fn set_net_manual_name(project: &mut Project, net: NetSignalId) -> Result<()> {
        let mut signal = project
            .circuit()
            .net_signal(net)
            .ok_or(Error::Other("Net signal not found.".to_owned()))?
            .clone();
        if signal.has_auto_name() {
            let name = signal.name().clone();
            signal.set_name(name, false);
            project.apply(Mutation::UpdateNetSignal(signal))?;
        }
        Ok(())
    }

    // --- Schematic ---

    fn import_schematic(
        &self,
        project: &mut Project,
        converter: &EagleLibraryConverter,
        state: &mut State,
        sheet: &model::Sheet,
        log: &MessageLogger<'_>,
    ) -> Result<()> {
        // Determine directory name.
        let mut dir_name = FilePath::clean_file_name(
            &sheet.description,
            CleanFileNameOptions::new(true, FileNameCase::Lower),
            120,
        );
        if dir_name.starts_with("sheet_") {
            dir_name.clear(); // Avoid conflicts!
        }
        if dir_name.is_empty() {
            dir_name = format!("sheet_{}", state.schematic_dir_names.len() + 1);
        }
        state.schematic_dir_names.insert(dir_name.clone());

        // Determine schematic name.
        let mut name = ElementName::clean(&sheet.description);
        if name.is_empty() {
            // No translation to avoid exceptions!
            name = format!("Sheet {}", state.schematic_dir_names.len());
        }

        // Create schematic.
        let slog = MessageLogger::child(log, &name);
        let mut schematic = Schematic::new(self.uuid(), ElementName::new(name)?, dir_name);
        let mut grid_interval = schematic.grid_interval();
        let mut grid_unit = schematic.grid_unit();
        if let Some(s) = &self.schematic {
            self.tc
                .convert_grid(&s.grid, &mut grid_interval, &mut grid_unit);
        }
        schematic.set_grid_interval(grid_interval);
        schematic.set_grid_unit(grid_unit);
        let sid: SchematicId = schematic.id();
        project.apply(Mutation::AddSchematic {
            schematic,
            index: None,
        })?;

        // Symbols
        for inst in &sheet.instances {
            let entry = state
                .component_map
                .get(&inst.part)
                .ok_or_else(|| runtime(format!("Component instance not found: {}", inst.part)))?;
            let cmp_id = entry.uuid;
            let cmp = project
                .circuit()
                .component_instance(cmp_id)
                .ok_or(Error::Other("Component instance not found.".to_owned()))?;
            let gate = *state
                .lib_component_gate_map
                .get(&(cmp.lib_component(), inst.gate.clone()))
                .ok_or(Error::Other("Gate not found.".to_owned()))?;
            let mirror = inst.rotation.mirror;
            let rotation = self.tc.convert_angle(if mirror {
                -inst.rotation.angle
            } else {
                inst.rotation.angle
            })?;
            let mut symbol = SchematicSymbol::new(
                self.uuid(),
                cmp_id,
                gate,
                self.tc.convert_point(inst.position)?,
                rotation,
                mirror,
            );
            let lib_symbol = {
                let lib = project.library();
                let lib_cmp = lib
                    .component(&cmp.lib_component())
                    .ok_or(Error::Other("Component not in project library.".to_owned()))?;
                let item = lib_cmp
                    .symbol_variants()
                    .iter()
                    .find_map(|v| v.symbol_items().by_uuid(&gate))
                    .ok_or(Error::Other("Gate not found.".to_owned()))?;
                lib.symbol(&item.symbol_uuid())
                    .ok_or(Error::Other("Symbol not in project library.".to_owned()))?
            };
            if !inst.smashed {
                for text in symbol.default_texts(lib_symbol) {
                    symbol.insert_text(text);
                }
            } else {
                for attr in &inst.attributes {
                    if attr.display != model::AttributeDisplay::Value {
                        continue;
                    }
                    match self.tc.try_convert_schematic_attribute(attr) {
                        Ok(Some(text)) => {
                            symbol.insert_text(text);
                        }
                        Ok(None) => slog.warning(&tr!(
                            CTX,
                            "Skipped text on layer {0} ({1}).",
                            attr.layer,
                            EagleTypeConverter::layer_name(attr.layer, "unknown")
                        )),
                        Err(e) => slog.warning(&format!("Skipped attribute text: {e}")),
                    }
                }
                let transform = Transform::from_obj(&symbol);
                let texts: Vec<Text> = lib_symbol
                    .texts()
                    .iter()
                    .filter(|t| !t.text().starts_with("{{"))
                    .map(|text| {
                        let mut copy = text.clone();
                        copy.set_position(transform.map(&copy.position()));
                        copy.set_rotation(transform.map_non_mirrorable(copy.rotation()));
                        if symbol.mirrored() {
                            copy.set_align(copy.align().mirrored_v());
                        }
                        copy.set_locked(true);
                        copy
                    })
                    .collect();
                for text in texts {
                    symbol.insert_text(text);
                }
            }
            project.apply(Mutation::Schematic(SchematicMutation::AddSymbol {
                schematic: sid,
                symbol,
            }))?;
        }

        // Geometry
        let mut geometries = self.tc.convert_and_join_wires(&sheet.wires, true, &slog);
        for obj in &sheet.rectangles {
            geometries.push(self.tc.convert_rectangle(obj, true)?);
        }
        for obj in &sheet.polygons {
            geometries.push(self.tc.convert_polygon(obj, true)?);
        }
        for obj in &sheet.circles {
            geometries.push(self.tc.convert_circle(obj, true)?);
        }
        for obj in &sheet.frames {
            geometries.push(self.tc.convert_frame(obj)?);
        }
        for g in &geometries {
            if let Some(mut o) = self.tc.try_convert_to_schematic_polygon(g) {
                let grab = g.grab_area && o.is_filled();
                o.set_is_grab_area(grab);
                project.apply(Mutation::Schematic(SchematicMutation::AddPolygon {
                    schematic: sid,
                    polygon: o,
                }))?;
            } else {
                slog.warning(&tr!(
                    CTX,
                    "Skipped graphics object on layer {0} ({1}).",
                    g.layer_id,
                    EagleTypeConverter::layer_name(g.layer_id, "unknown")
                ));
            }
        }

        // Texts
        for obj in &sheet.texts {
            match self.tc.try_convert_schematic_text(obj, false) {
                Ok(Some(text)) => {
                    project.apply(Mutation::Schematic(SchematicMutation::AddText {
                        schematic: sid,
                        text,
                    }))?;
                }
                Ok(None) => slog.warning(&tr!(
                    CTX,
                    "Skipped text on layer {0} ({1}).",
                    obj.layer,
                    EagleTypeConverter::layer_name(obj.layer, "unknown")
                )),
                Err(e) => slog.warning(&format!("Skipped text: {e}")),
            }
        }

        // Collect net wire endpoints up-front so that buses can insert
        // junctions at those positions. In EAGLE, netlines touching a bus
        // are implicitly connected, but in LibrePCB we explicitly connect
        // wires to buses.
        let mut net_wire_endpoints: Vec<Point> = Vec::new();
        for net in &sheet.nets {
            for segment in &net.segments {
                for wire in segment.wires.iter().filter(|w| w.p1 != w.p2) {
                    for p in [wire.p1, wire.p2] {
                        let p = self.tc.convert_point(p)?;
                        if !net_wire_endpoints.contains(&p) {
                            net_wire_endpoints.push(p);
                        }
                    }
                }
            }
        }

        // Buses (imported before nets so netlines can anchor on bus
        // junctions).
        for eagle_bus in &sheet.buses {
            let result = (|| -> Result<()> {
                let bus = self.import_bus(project, state, &eagle_bus.name)?;
                for seg in &eagle_bus.segments {
                    let mut bus_segment = SchematicBusSegment::new(self.uuid(), bus);
                    let mut junctions: Vec<Junction> = Vec::new();
                    let get_or_create = |pos: Point, junctions: &mut Vec<Junction>| {
                        if let Some(j) = junctions.iter().find(|j| j.position() == pos) {
                            j.uuid()
                        } else {
                            let j = Junction::new(self.uuid(), pos);
                            let uuid = j.uuid();
                            junctions.push(j);
                            uuid
                        }
                    };
                    let mut lines: Vec<NetLine> = Vec::new();
                    for wire in &seg.wires {
                        let p1 = self.tc.convert_point(wire.p1)?;
                        let p2 = self.tc.convert_point(wire.p2)?;
                        if p1 == p2 {
                            continue;
                        }
                        if wire.wire_style != model::WireStyle::Continuous {
                            slog.warning(&tr!(
                                CTX,
                                "Dashed/dotted line is not supported, converting to continuous."
                            ));
                        }
                        // Find intermediate junction positions that lie
                        // strictly between p1 and p2 (e.g. where netlines
                        // touch the bus). The wire needs to be split into
                        // multiple bus lines so each intermediate point
                        // becomes an actual bus junction which netlines can
                        // anchor on.
                        let mut intermediates: Vec<Point> = net_wire_endpoints
                            .iter()
                            .copied()
                            .filter(|&p| (p != p1) && (p != p2))
                            .filter(|&p| {
                                let (d, _) =
                                    toolbox::shortest_distance_between_point_and_line(p, p1, p2);
                                *d < Length::new(1000)
                            })
                            .collect();
                        intermediates.sort_by_key(|p| *(*p - p1).length());
                        let mut path_points = vec![p1];
                        path_points.extend(intermediates);
                        path_points.push(p2);
                        for pair in path_points.windows(2) {
                            let a = get_or_create(pair[0], &mut junctions);
                            let b = get_or_create(pair[1], &mut junctions);
                            lines.push(NetLine::new(
                                self.uuid(),
                                SchematicBusSegment::default_line_width(),
                                NetLineAnchor::Junction(a),
                                NetLineAnchor::Junction(b),
                            ));
                        }
                    }
                    if lines.is_empty() {
                        slog.warning(&format!(
                            "Skipped segment of bus '{}' since it doesn't contain any lines.",
                            eagle_bus.name
                        ));
                        continue;
                    }
                    for label in &seg.labels {
                        let rot = self.tc.convert_angle(label.rotation.angle)?;
                        let mirror = label.rotation.mirror;
                        let pos = self.tc.convert_point(label.position)?;
                        bus_segment.insert_label(NetLabel::new(
                            self.uuid(),
                            pos,
                            if mirror { -rot } else { rot },
                            mirror,
                        ));
                    }
                    for j in junctions {
                        bus_segment.insert_junction(j);
                    }
                    for l in lines {
                        bus_segment.insert_line(l);
                    }
                    project.apply(Mutation::Schematic(SchematicMutation::AddBusSegment {
                        schematic: sid,
                        segment: bus_segment,
                    }))?;
                }
                Ok(())
            })();
            if let Err(e) = result {
                slog.critical(&format!(
                    "Failed to import segment of bus '{}': {e}",
                    eagle_bus.name
                ));
            }
        }

        // Nets
        for eagle_net in &sheet.nets {
            let net = self.import_net(project, state, eagle_net)?;
            let result =
                self.import_net_segments(project, converter, state, sid, eagle_net, net, &slog);
            if let Err(e) = result {
                slog.critical(&format!(
                    "Failed to import segment of net '{}': {e}",
                    eagle_net.name
                ));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn import_net_segments(
        &self,
        project: &mut Project,
        converter: &EagleLibraryConverter,
        state: &mut State,
        sid: SchematicId,
        eagle_net: &model::Net,
        net: NetSignalId,
        log: &MessageLogger<'_>,
    ) -> Result<()> {
        let mut splitter = SchematicNetSegmentSplitter::new();
        let mut anchor_map: HashMap<Point, NetLineAnchor> = HashMap::new();

        for segment in &eagle_net.segments {
            // Collect pin refs.
            for pin_ref in &segment.pin_refs {
                let entry = state
                    .component_map
                    .get(&pin_ref.part)
                    .ok_or(Error::Other("Component not found.".to_owned()))?;
                let cmp_id = entry.uuid;
                let part = self.part(&pin_ref.part)?;
                let sig_uuid = converter.component_signal_of_symbol_pin(
                    &part.library,
                    &part.library_urn,
                    &part.device_set,
                    &pin_ref.gate,
                    &pin_ref.pin,
                )?;
                let signal = ComponentSignalRef {
                    component: cmp_id,
                    signal: sig_uuid,
                };
                let cmp = project
                    .circuit()
                    .component_instance(cmp_id)
                    .ok_or(Error::Other("Component not found.".to_owned()))?;
                if cmp.signal(&sig_uuid).is_none() {
                    return Err(Error::Other("Component signal not found.".to_owned()));
                }
                if cmp.signal(&sig_uuid).and_then(|s| s.net()) != Some(net) {
                    project.apply(Mutation::SetComponentSignalNet {
                        signal,
                        net: Some(net),
                    })?;
                    for list in state.net_component_signals.values_mut() {
                        list.retain(|s| *s != signal);
                    }
                    state
                        .net_component_signals
                        .entry(net)
                        .or_default()
                        .push(signal);
                }
                let forced = project
                    .circuit()
                    .component_instance(cmp_id)
                    .and_then(|c| project.library().component(&c.lib_component()))
                    .and_then(|c| c.signals().by_uuid(&sig_uuid))
                    .is_some_and(|s| s.is_net_signal_name_forced());
                if forced {
                    Self::set_net_manual_name(project, net)?; // No auto-name
                }
                let pins = registered_symbol_pins(project, signal)?;
                let [(pin_anchor, pin_pos)] = pins.as_slice() else {
                    return Err(Error::Other("unexpected symbol pin count.".to_owned()));
                };
                let (pin_anchor, pin_pos) = (*pin_anchor, *pin_pos);
                splitter.add_fixed_anchor(pin_anchor, pin_pos, false);
                if let Some(existing) = anchor_map.get(&pin_pos) {
                    // There's another pin on the same position -> add a
                    // netline to connect them since EAGLE implicitly
                    // considers them as connected.
                    splitter.add_net_line(NetLine::new(
                        self.uuid(),
                        SchematicNetSegment::default_line_width(),
                        *existing,
                        pin_anchor,
                    ));
                }
                anchor_map.insert(pin_pos, pin_anchor);
            }

            // Collect bus anchors.
            let sch = project
                .schematic(sid)
                .ok_or(Error::Other("Schematic not found.".to_owned()))?;
            for (seg_id, seg) in sch.bus_segments() {
                for j in seg.junctions().values() {
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        anchor_map.entry(j.position())
                    {
                        let anchor = NetLineAnchor::BusJunction {
                            segment: seg_id.0,
                            junction: j.uuid(),
                        };
                        splitter.add_fixed_anchor(anchor, j.position(), false);
                        e.insert(anchor);
                    }
                }
            }

            // Convert wires.
            let mut get_or_create_anchor =
                |pos: Point, splitter: &mut SchematicNetSegmentSplitter| -> NetLineAnchor {
                    if let Some(a) = anchor_map.get(&pos) {
                        *a
                    } else {
                        let junction = Junction::new(self.uuid(), pos);
                        let anchor = NetLineAnchor::Junction(junction.uuid());
                        splitter.add_junction(junction);
                        anchor_map.insert(pos, anchor);
                        anchor
                    }
                };
            for wire in &segment.wires {
                // Skip zero-length wires to avoid possible redundant wires.
                if wire.p1 == wire.p2 {
                    continue;
                }
                let p1 = get_or_create_anchor(self.tc.convert_point(wire.p1)?, &mut splitter);
                let p2 = get_or_create_anchor(self.tc.convert_point(wire.p2)?, &mut splitter);
                splitter.add_net_line(NetLine::new(
                    self.uuid(),
                    SchematicNetSegment::default_line_width(),
                    p1,
                    p2,
                ));
                if wire.wire_style != model::WireStyle::Continuous {
                    log.warning(&tr!(
                        CTX,
                        "Dashed/dotted line is not supported, converting to continuous."
                    ));
                }
                if wire.wire_cap != model::WireCap::Round {
                    log.warning(&tr!(
                        CTX,
                        "Flat line end is not supported, converting to round."
                    ));
                }
            }
            for label in &segment.labels {
                let rot = self.tc.convert_angle(label.rotation.angle)?;
                let mirror = label.rotation.mirror;
                let mut pos = self.tc.convert_point(label.position)?;
                if label.xref {
                    pos += Point::new(
                        Length::new(if mirror { -254_000 } else { 254_000 }),
                        Length::new(-1_270_000),
                    )
                    .rotated(if mirror { -rot } else { rot }, Point::ORIGIN);
                    log.warning(&tr!(
                        CTX,
                        "XRef-style net label is not supported, converting to normal net label."
                    ));
                }
                splitter.add_net_label(NetLabel::new(
                    self.uuid(),
                    pos,
                    if mirror { -rot } else { rot },
                    mirror,
                ));
            }
        }

        // Determine segments and add them to the schematic.
        for part in splitter.split() {
            let mut segment = SchematicNetSegment::new(self.uuid(), net);
            for j in part.junctions {
                segment.insert_junction(j);
            }
            for l in part.lines {
                segment.insert_line(l);
            }
            let has_labels = !part.labels.is_empty();
            for label in part.labels {
                segment.insert_label(label);
            }
            project.apply(Mutation::Schematic(SchematicMutation::AddNetSegment {
                schematic: sid,
                segment,
            }))?;
            if has_labels {
                Self::set_net_manual_name(project, net)?; // No auto-name
            }
        }
        Ok(())
    }

    // --- Board ---

    fn import_board(
        &self,
        project: &mut Project,
        converter: &mut EagleLibraryConverter,
        state: &mut State,
        log: &MessageLogger<'_>,
    ) -> Result<()> {
        let eagle_board = self.board.as_ref().expect("checked by caller");
        let blog = MessageLogger::child(log, "BOARD");
        let mut board = Board::new(self.uuid(), ElementName::new("default")?, "default");
        let bid: BoardId = board.id();
        let mut settings = board.settings().clone();

        // Grid settings
        self.tc.convert_grid(
            &eagle_board.grid,
            &mut settings.grid_interval,
            &mut settings.grid_unit,
        );

        // Layer setup
        let mut copper_layer_map: BTreeMap<Layer, Layer> = BTreeMap::new();
        if let Some(p) = eagle_board.design_rules.param("layerSetup") {
            copper_layer_map = self.tc.convert_layer_setup(&p.value)?;
        }
        copper_layer_map.insert(Layer::TOP_COPPER, Layer::TOP_COPPER);
        copper_layer_map.insert(Layer::BOT_COPPER, Layer::BOT_COPPER);
        settings.inner_layer_count = (copper_layer_map.len() - 2) as u32;
        let map_layer = |layer: Option<Layer>| -> Option<Layer> {
            layer.map(|l| copper_layer_map.get(&l).copied().unwrap_or(l))
        };
        let rules = &eagle_board.design_rules;

        // Design rules
        let mut copper_board_distance: Option<UnsignedLength> = None;
        let result = (|| -> Result<()> {
            let r = &mut settings.design_rules;
            r.set_pad_cmp_side_auto_annular_ring(false); // Not sure if EAGLE supports this.
            r.set_pad_inner_auto_annular_ring(true); // Not sure if EAGLE supports this.
            if let Some(p) = rules.param("mdCopperDimension") {
                copper_board_distance = Some(self.tc.convert_param_to_unsigned_length(p)?);
            }
            if let Some(p) = rules.param("mlViaStopLimit") {
                r.set_stop_mask_max_via_drill_diameter(
                    self.tc.convert_param_to_unsigned_length(p)?,
                );
            }
            if settings.inner_layer_count > 0 {
                // Take automatic THT annular width from inner layers since it
                // is probably the smaller one and thus should reduce the
                // risk of DRC warnings.
                if let Some(v) =
                    self.try_get_drc_ratio("rvPadInner", "rlMinPadInner", "rlMaxPadInner")?
                {
                    r.set_pad_annular_ring(v);
                }
            } else if let Some(v) =
                self.try_get_drc_ratio("rvPadBottom", "rlMinPadBottom", "rlMaxPadBottom")?
            {
                // Take automatic THT annular width from bottom layer since
                // inner layers are not disabled so these values might not
                // be reasonable.
                r.set_pad_annular_ring(v);
            }
            if let Some(v) =
                self.try_get_drc_ratio("rvViaOuter", "rlMinViaOuter", "rlMaxViaOuter")?
            {
                r.set_via_annular_ring(v);
            } else {
                blog.warning(
                    "Design rules for via annular rings were missing, thus the automatic size \
                     of some vias may be wrong. Please check & correct the via design rules \
                     manually.",
                );
            }
            if let Some(v) =
                self.try_get_drc_ratio("mvStopFrame", "mlMinStopFrame", "mlMaxStopFrame")?
            {
                r.set_stop_mask_clearance(v);
            }
            if let Some(v) =
                self.try_get_drc_ratio("mvCreamFrame", "mlMinCreamFrame", "mlMaxCreamFrame")?
            {
                r.set_solder_paste_clearance(v);
            }
            Ok(())
        })();
        if let Err(e) = result {
            // Note: Like upstream, rules set before the error are kept.
            blog.critical(&format!("Failed to import design rules: {e}"));
        }

        // DRC settings
        let mut drc = settings.drc_settings.clone();
        let result = (|| -> Result<()> {
            let len = |name: &str| -> Result<Option<UnsignedLength>> {
                rules
                    .param(name)
                    .map(|p| self.tc.convert_param_to_unsigned_length(p))
                    .transpose()
            };
            if let Some(v) = len("mdWireWire")? {
                drc.set_min_copper_copper_clearance(v);
            }
            if let Some(v) = len("mdCopperDimension")? {
                drc.set_min_copper_board_clearance(v);
                drc.set_min_copper_npth_clearance(v);
            }
            if let Some(v) = len("mdDrill")? {
                drc.set_min_drill_drill_clearance(v);
                drc.set_min_drill_board_clearance(v);
            }
            if let Some(v) = len("msWidth")? {
                drc.set_min_copper_width(v);
            }
            if let Some(v) = len("rlMinViaInner")? {
                drc.set_min_pth_annular_ring(v);
            }
            if let Some(v) = len("msDrill")? {
                drc.set_min_npth_drill_diameter(v);
                drc.set_min_pth_drill_diameter(v);
            }
            Ok(())
        })();
        match result {
            Ok(()) => settings.drc_settings = drc,
            Err(e) => blog.critical(&format!("Failed to import DRC settings: {e}")),
        }
        let min_copper_copper_clearance = settings.drc_settings.min_copper_copper_clearance();
        let inner_layer_count = settings.inner_layer_count;
        board.set_settings(settings);
        project.apply(Mutation::AddBoard { board, index: None })?;

        // Devices
        let assembly_variants: BTreeSet<AssemblyVariantId> = project
            .circuit()
            .assembly_variants()
            .uuids()
            .into_iter()
            .map(AssemblyVariantId)
            .collect();
        for elem in &eagle_board.elements {
            let Some(entry) = state.component_map.get(&elem.name).cloned() else {
                blog.critical(&format!(
                    "Component '{}' ({}) not found in circuit. Note that LibrePCB does not yet \
                     support placing devices on the board which don't exist in the schematic.",
                    elem.name, elem.package
                ));
                continue;
            };
            let pkg_uuid = self.import_library_package(
                converter,
                state,
                project,
                &elem.library,
                &elem.library_urn,
                &elem.package,
                &blog,
            )?;
            let dev_uuid = self.import_library_device(
                converter,
                state,
                project,
                &entry,
                &elem.library,
                &elem.library_urn,
                &blog,
            )?;
            let lib = project.library();
            let lib_pkg = lib
                .package(&pkg_uuid)
                .ok_or(Error::Other("Package not in project library.".to_owned()))?;
            if lib_pkg.footprints().len() != 1 {
                return Err(Error::Other("Unexpected footprint count.".to_owned()));
            }
            let footprint = lib_pkg.footprints().as_slice()[0].clone();
            let lib_dev = lib
                .device(&dev_uuid)
                .ok_or(Error::Other("Device not in project library.".to_owned()))?;
            let mirror = elem.rotation.mirror;
            let rotation = self.tc.convert_angle(elem.rotation.angle)?;
            let mut dev = BoardDevice::from_library(
                entry.uuid,
                lib_dev,
                lib_pkg,
                footprint.uuid(),
                self.tc.convert_point(elem.position)?,
                if mirror { -rotation } else { rotation },
                mirror,
                elem.locked,
                true,
                !elem.smashed,
            )?;

            // Add stroke texts.
            if elem.smashed {
                for attr in &elem.attributes {
                    if attr.display != model::AttributeDisplay::Value {
                        continue;
                    }
                    match self.tc.try_convert_board_attribute(attr) {
                        Ok(Some(t)) => {
                            dev.insert_stroke_text(BoardStrokeTextData::new(
                                t.uuid(),
                                t.layer(),
                                t.text().clone(),
                                t.position(),
                                t.rotation(),
                                t.height(),
                                t.stroke_width(),
                                t.letter_spacing(),
                                t.line_spacing(),
                                t.align(),
                                t.mirrored(),
                                t.auto_rotate(),
                                false,
                            ));
                        }
                        Ok(None) => blog.warning(&tr!(
                            CTX,
                            "Skipped text on layer {0} ({1}).",
                            attr.layer,
                            EagleTypeConverter::layer_name(attr.layer, "unknown")
                        )),
                        Err(e) => blog.warning(&format!("Skipped attribute text: {e}")),
                    }
                }
                let transform = Transform::from_obj(&dev);
                for text in footprint.stroke_texts().iter() {
                    if !text.text().starts_with("{{") {
                        dev.insert_stroke_text(BoardStrokeTextData::new(
                            text.uuid(),
                            text.layer(),
                            text.text().clone(),
                            transform.map(&text.position()),
                            transform.map_mirrorable(text.rotation()),
                            text.height(),
                            text.stroke_width(),
                            text.letter_spacing(),
                            text.line_spacing(),
                            text.align(),
                            dev.mirrored() != text.mirrored(),
                            text.auto_rotate(),
                            false,
                        ));
                    }
                }
            }
            project.apply(Mutation::Board(BoardMutation::AddDevice {
                board: bid,
                device: dev,
            }))?;

            // Add assembly options.
            let cmp = project
                .circuit()
                .component_instance(entry.uuid)
                .ok_or(Error::Other("Component not found.".to_owned()))?;
            let mut props = cmp.properties();
            let mut attributes = props.attributes.clone();
            let mut option = ComponentAssemblyOption::new(
                dev_uuid,
                AttributeList::new(),
                BTreeSet::new(),
                PartList::new(),
            );
            if elem.populate {
                option.set_assembly_variants(assembly_variants.clone());
            }
            let mut mpn = SimpleString::clean("");
            let mut manufacturer = SimpleString::clean("");
            self.tc
                .try_extract_mpn_and_manufacturer(&mut attributes, &mut mpn, &mut manufacturer);
            if !mpn.is_empty() {
                // Convert attributes to a Part.
                option
                    .parts_mut()
                    .push(Part::new(mpn, manufacturer, AttributeList::new()));
                props.attributes = attributes;
            }
            let mut options = librepcb_core::project::circuit::ComponentAssemblyOptionList::new();
            options.push(option);
            props.assembly_options = options;
            project.apply(Mutation::UpdateComponentInstance(props))?;
        }

        // Geometry
        let mut geometries = self
            .tc
            .convert_and_join_wires(&eagle_board.wires, true, &blog);
        for obj in &eagle_board.rectangles {
            geometries.push(self.tc.convert_rectangle(obj, true)?);
        }
        for obj in &eagle_board.polygons {
            geometries.push(self.tc.convert_polygon(obj, true)?);
        }
        for obj in &eagle_board.circles {
            geometries.push(self.tc.convert_circle(obj, true)?);
        }
        let mut polygons: Vec<BoardPolygonData> = Vec::new();
        for g in &geometries {
            let zones = self.tc.try_convert_to_board_zones(g)?;
            if !zones.is_empty() {
                for zone in zones {
                    let mut layers = BTreeSet::new();
                    if zone.layers().contains(ZoneLayers::TOP) {
                        layers.insert(Layer::TOP_COPPER);
                    }
                    if zone.layers().contains(ZoneLayers::INNER) {
                        for i in 1..=inner_layer_count as usize {
                            layers.extend(Layer::inner_copper(i));
                        }
                    }
                    if zone.layers().contains(ZoneLayers::BOTTOM) {
                        layers.insert(Layer::BOT_COPPER);
                    }
                    let data = BoardZoneData::new(
                        zone.uuid(),
                        layers,
                        zone.rules(),
                        zone.outline().clone(),
                        false,
                    )?;
                    project.apply(Mutation::Board(BoardMutation::AddItem {
                        board: bid,
                        item: BoardItem::Zone(data),
                    }))?;
                }
            } else if let Some(o) = self.tc.try_convert_to_board_polygon(g) {
                let layer = map_layer(Some(o.layer())).unwrap_or(o.layer());
                polygons.push(BoardPolygonData::new(
                    o.uuid(),
                    layer,
                    o.line_width(),
                    o.path().clone(),
                    o.is_filled(),
                    g.grab_area && o.is_filled(),
                    false,
                ));
            } else {
                blog.warning(&tr!(
                    CTX,
                    "Skipped graphics object on layer {0} ({1}).",
                    g.layer_id,
                    EagleTypeConverter::layer_name(g.layer_id, "unknown")
                ));
            }
        }

        // Make the longest polygon the board outline, and all others
        // cutouts. This logic is needed because EAGLE does not distinguish
        // these two layers. Candidates are evaluated in UUID order (upstream
        // iterates the board's polygon map).
        let mut sorted: Vec<usize> = (0..polygons.len()).collect();
        sorted.sort_by_key(|&i| polygons[i].uuid());
        let mut outline_candidate: Option<usize> = None;
        for i in sorted {
            let layer = polygons[i].layer();
            if (layer == Layer::BOARD_OUTLINES) || (layer == Layer::BOARD_CUTOUTS) {
                if outline_candidate.is_none_or(|c| {
                    polygons[i].path().total_straight_length()
                        > polygons[c].path().total_straight_length()
                }) {
                    outline_candidate = Some(i);
                }
                polygons[i].set_layer(Layer::BOARD_CUTOUTS);
            }
        }
        if let Some(c) = outline_candidate {
            polygons[c].set_layer(Layer::BOARD_OUTLINES);
        }
        for p in polygons {
            project.apply(Mutation::Board(BoardMutation::AddItem {
                board: bid,
                item: BoardItem::Polygon(p),
            }))?;
        }

        // Texts
        for obj in &eagle_board.texts {
            match self.tc.try_convert_board_text(obj, false) {
                Ok(Some(t)) => {
                    let data = BoardStrokeTextData::new(
                        t.uuid(),
                        map_layer(Some(t.layer())).unwrap_or(t.layer()),
                        t.text().clone(),
                        t.position(),
                        t.rotation(),
                        t.height(),
                        t.stroke_width(),
                        t.letter_spacing(),
                        t.line_spacing(),
                        t.align(),
                        t.mirrored(),
                        t.auto_rotate(),
                        false,
                    );
                    project.apply(Mutation::Board(BoardMutation::AddItem {
                        board: bid,
                        item: BoardItem::StrokeText(data),
                    }))?;
                }
                Ok(None) => blog.warning(&tr!(
                    CTX,
                    "Skipped text on layer {0} ({1}).",
                    obj.layer,
                    EagleTypeConverter::layer_name(obj.layer, "unknown")
                )),
                Err(e) => blog.warning(&format!("Skipped text: {e}")),
            }
        }

        // Holes
        for obj in &eagle_board.holes {
            let result = self.tc.convert_hole(obj).and_then(|h| {
                let data = librepcb_core::project::board::BoardHoleData::new(
                    h.uuid(),
                    h.diameter(),
                    h.path().clone(),
                    h.stop_mask_config(),
                    false,
                );
                project.apply(Mutation::Board(BoardMutation::AddItem {
                    board: bid,
                    item: BoardItem::Hole(data),
                }))?;
                Ok(())
            });
            if let Err(e) = result {
                blog.critical(&format!("Skipped hole: {e}"));
            }
        }

        // Net segments
        for signal in &eagle_board.signals {
            let net = state.net_signal_map.get(&signal.name).copied();
            let result =
                self.import_board_signal(project, state, bid, signal, net, &map_layer, &blog);
            if let Err(e) = result {
                blog.critical(&format!(
                    "Failed to import segment of net '{}': {e}",
                    signal.name
                ));
            }

            // Add planes and keepout zones.
            for obj in &signal.polygons {
                let result = (|| -> Result<()> {
                    let layer = map_layer(self.tc.try_convert_board_layer(obj.layer))
                        .filter(|l| l.is_copper())
                        .ok_or(Error::Other("Plane not on copper layer.".to_owned()))?;
                    if obj.pour == model::PolygonPour::Cutout {
                        let path = self.tc.convert_vertices(&obj.vertices, false)?;
                        let line_width = self.tc.convert_length(obj.width)?;
                        for outline in self.tc.convert_board_zone_outline(&path, line_width)? {
                            let zone = BoardZoneData::new(
                                self.uuid(),
                                BTreeSet::from([layer]),
                                ZoneRules::NO_PLANES,
                                outline,
                                false,
                            )?;
                            project.apply(Mutation::Board(BoardMutation::AddItem {
                                board: bid,
                                item: BoardItem::Zone(zone),
                            }))?;
                        }
                    } else {
                        let isolate = self.tc.convert_length(obj.isolate)?;
                        let mut plane = BoardPlane::new(
                            self.uuid(),
                            layer,
                            net,
                            self.tc.convert_vertices(&obj.vertices, false)?,
                        );
                        plane.set_min_width(UnsignedLength::new(
                            self.tc.convert_length(obj.width)?,
                        )?);
                        plane.set_min_clearance_to_copper(if isolate > Length::ZERO {
                            UnsignedLength::new(isolate)?
                        } else {
                            min_copper_copper_clearance
                        });
                        plane.set_min_clearance_to_board(
                            copper_board_distance.unwrap_or(plane.min_clearance_to_copper()),
                        );
                        plane.set_min_clearance_to_npth(plane.min_clearance_to_board());
                        plane.set_connect_style(if obj.thermals {
                            PlaneConnectStyle::ThermalRelief
                        } else {
                            PlaneConnectStyle::Solid
                        });
                        if *plane.thermal_spoke_width() < *plane.min_width() {
                            // Avoid possibly disappearing planes.
                            plane.set_thermal_spoke_width(PositiveLength::new(*plane.min_width())?);
                        }
                        plane.set_priority(6 - obj.rank); // EAGLE: 1..6
                        plane.set_keep_islands(obj.orphans);
                        project.apply(Mutation::Board(BoardMutation::AddPlane {
                            board: bid,
                            plane,
                        }))?;
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    blog.critical(&format!("Skipped plane: {e}"));
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn import_board_signal(
        &self,
        project: &mut Project,
        state: &State,
        bid: BoardId,
        signal: &model::Signal,
        net: Option<NetSignalId>,
        map_layer: &dyn Fn(Option<Layer>) -> Option<Layer>,
        log: &MessageLogger<'_>,
    ) -> Result<()> {
        let mut splitter = BoardNetSegmentSplitter::new();
        // Key: (layer, position), `None` layer for vias.
        let mut anchor_map: HashMap<(Option<Layer>, Point), TraceAnchor> = HashMap::new();

        // Convert vias.
        for via in &signal.vias {
            let start = map_layer(
                self.tc
                    .try_convert_board_layer(via.start_layer().unwrap_or(0)),
            );
            let end = map_layer(
                self.tc
                    .try_convert_board_layer(via.end_layer().unwrap_or(0)),
            );
            let (Some(start), Some(end), true, true) = (
                start,
                end,
                via.start_layer().is_some(),
                via.end_layer().is_some(),
            ) else {
                return Err(runtime(format!("Invalid via layer extent: {}", via.extent)));
            };
            if !start.is_copper()
                || !end.is_copper()
                || (start.copper_number() > end.copper_number())
            {
                return Err(runtime(format!("Invalid via layer extent: {}", via.extent)));
            }
            let drill = PositiveLength::new(self.tc.convert_length(via.drill)?)?;
            let size_raw = self.tc.convert_length(via.diameter)?;
            let mut size = None;
            if size_raw >= *drill {
                size = Some(PositiveLength::new(size_raw)?);
            } else if size_raw != Length::ZERO {
                log.warning("Via size smaller than drill diameter, using automatic size instead.");
            }
            let exposure = if via.always_stop {
                MaskConfig::Automatic
            } else {
                MaskConfig::Off
            };
            if via.shape != model::ViaShape::Round {
                log.warning(&tr!(
                    CTX,
                    "Square/octagon via shape not supported, converting to circular."
                ));
            }
            let v = Via::new(
                self.uuid(),
                start,
                end,
                self.tc.convert_point(via.position)?,
                Some(drill),
                size,
                exposure,
            )?;
            anchor_map.insert((None, v.position()), TraceAnchor::Via(v.uuid()));
            splitter.add_via(v, false);
        }

        // Pads of the net, in the order the component signals were
        // connected.
        let mut net_pads: Vec<(TraceAnchor, Point, BTreeSet<Layer>)> = Vec::new();
        if let Some(net) = net {
            let board = project
                .board(bid)
                .ok_or(Error::Other("Board not found.".to_owned()))?;
            for signal_ref in state.net_component_signals.get(&net).into_iter().flatten() {
                let Some(dev) = board.devices().get(&signal_ref.component) else {
                    continue;
                };
                for pad in dev.pads(project.library(), project.circuit())? {
                    if pad.component_signal() == Some(*signal_ref) {
                        let layers = Layer::all()
                            .iter()
                            .copied()
                            .filter(|l| l.is_copper() && pad.is_on_layer(*l))
                            .collect();
                        net_pads.push((pad.trace_anchor(), pad.position(), layers));
                    }
                }
            }
        }

        // Convert traces.
        let mut get_or_create_anchor =
            |layer: Layer, pos: Point, splitter: &mut BoardNetSegmentSplitter| -> TraceAnchor {
                // Find via.
                if let Some(a) = anchor_map.get(&(None, pos)) {
                    return *a;
                }
                // Find net point.
                if let Some(a) = anchor_map.get(&(Some(layer), pos)) {
                    return *a;
                }
                // Find pad.
                for (anchor, pad_pos, layers) in &net_pads {
                    if layers.contains(&layer) && (*(*pad_pos - pos).length() < Length::new(100)) {
                        return *anchor;
                    }
                }
                // Add net point.
                let junction = Junction::new(self.uuid(), pos);
                let anchor = TraceAnchor::Junction(junction.uuid());
                splitter.add_junction(junction);
                anchor_map.insert((Some(layer), pos), anchor);
                anchor
            };
        for wire in &signal.wires {
            // Skip zero-length wires to avoid possible redundant wires.
            if wire.p1 == wire.p2 {
                continue;
            }
            // Skip "unrouted" wires (i.e. airwires) since they will be
            // rebuilt.
            if wire.layer == 19 {
                continue;
            }
            let Some(layer) =
                map_layer(self.tc.try_convert_board_layer(wire.layer)).filter(|l| l.is_copper())
            else {
                log.critical(&format!("Skipped trace on invalid layer: {}", wire.layer));
                continue;
            };
            let p1 = get_or_create_anchor(layer, self.tc.convert_point(wire.p1)?, &mut splitter);
            let p2 = get_or_create_anchor(layer, self.tc.convert_point(wire.p2)?, &mut splitter);
            if p1 == p2 {
                log.info("Attaching a trace to a pad removed a short trace segment.");
                continue;
            }
            if wire.wire_style != model::WireStyle::Continuous {
                log.critical(&tr!(
                    CTX,
                    "Dashed/dotted trace is not supported, converting to continuous."
                ));
            }
            if wire.wire_cap != model::WireCap::Round {
                log.critical(&tr!(
                    CTX,
                    "Flat trace end is not supported, converting to round."
                ));
            }
            if wire.curve != 0.0 {
                log.critical(&tr!(
                    CTX,
                    "Curved trace is not supported, converting to straight."
                ));
            }
            let trace = Trace::new(
                self.uuid(),
                layer,
                PositiveLength::new(self.tc.convert_length(wire.width)?)?,
                p1,
                p2,
            );
            splitter.add_trace(&trace, || self.uuid());
        }

        // Determine segments and add them to the board.
        for part in splitter.split() {
            let segment = BoardNetSegment::with_elements(
                self.uuid(),
                net,
                part.pads,
                part.vias,
                part.junctions,
                part.traces,
            );
            project.apply(Mutation::Board(BoardMutation::AddNetSegment {
                board: bid,
                segment,
            }))?;
        }
        Ok(())
    }

    // --- Helpers ---

    fn try_get_drc_ratio(
        &self,
        nr: &str,
        nmin: &str,
        nmax: &str,
    ) -> Result<Option<BoundedUnsignedRatio>> {
        let Some(board) = &self.board else {
            return Ok(None);
        };
        let rules = &board.design_rules;
        let (Some(pr), Some(pmin), Some(pmax)) =
            (rules.param(nr), rules.param(nmin), rules.param(nmax))
        else {
            return Ok(None);
        };
        let vr = self.tc.convert_param_to_unsigned_ratio(pr)?;
        let vmin = self.tc.convert_param_to_unsigned_length(pmin)?;
        let vmax = self.tc.convert_param_to_unsigned_length(pmax)?;
        // Note: Eagle allows to specify min>max so we have to correct this
        // case. It seems the min value is ignored then.
        Ok(Some(BoundedUnsignedRatio::new(vr, vmin.min(vmax), vmax)?))
    }

    fn symbol(&self, lib: &str, urn: &str, name: &str) -> Result<&model::Symbol> {
        self.symbols.get(&key(lib, urn, name)).ok_or_else(|| {
            runtime(format!(
                "Symbol not found in embedded library: {lib}::{urn}::{name}"
            ))
        })
    }

    fn package(&self, lib: &str, urn: &str, name: &str) -> Result<&model::Package> {
        self.packages.get(&key(lib, urn, name)).ok_or_else(|| {
            runtime(format!(
                "Package not found in embedded library: {lib}::{urn}::{name}"
            ))
        })
    }

    fn device_set(&self, lib: &str, urn: &str, name: &str) -> Result<&model::DeviceSet> {
        self.device_sets.get(&key(lib, urn, name)).ok_or_else(|| {
            runtime(format!(
                "Device set not found in embedded library: {lib}::{urn}::{name}"
            ))
        })
    }

    fn part(&self, name: &str) -> Result<&model::Part> {
        self.schematic
            .as_ref()
            .and_then(|s| s.parts.iter().find(|p| p.name == name))
            .ok_or_else(|| runtime(format!("Part not found: {name}")))
    }
}

fn device<'a>(dev_set: &'a model::DeviceSet, name: &str) -> Result<&'a model::Device> {
    dev_set
        .devices
        .iter()
        .find(|d| d.name == name)
        .ok_or_else(|| runtime(format!("Device not found in embedded library: {name}")))
}

/// Returns the first `n` characters of `s` (upstream `QString::left()`).
fn truncate(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

/// Returns the anchors and positions of the placed symbol pins connected to
/// a component signal (upstream `getRegisteredSymbolPins()`).
fn registered_symbol_pins(
    project: &Project,
    signal: ComponentSignalRef,
) -> Result<Vec<(NetLineAnchor, Point)>> {
    let mut pins = Vec::new();
    for sch in project.schematics() {
        for symbol in sch.symbols().values() {
            if symbol.component() != signal.component {
                continue;
            }
            for pin in symbol.pins(project.view())? {
                if pin.signal() == signal {
                    pins.push((pin.anchor(), pin.position()));
                }
            }
        }
    }
    Ok(pins)
}
