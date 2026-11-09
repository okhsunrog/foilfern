//! Port of libs/librepcb/kicadimport/kicadlibraryconverter.{h,cpp}.
//!
//! Converts KiCad footprints into packages and KiCad symbols into symbols
//! (one per gate), components and devices. Dependencies which were not
//! converted by the same converter are looked up in the workspace library
//! (elements imported earlier, identified by their "generated_by" key).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use librepcb_core::fileio::{
    FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem,
};
use librepcb_core::library::cmp::{
    CmpSigPinDisplayType, Component, ComponentPinSignalMapItem, ComponentPrefix, ComponentSignal,
    ComponentSymbolVariant, ComponentSymbolVariantItem, ComponentSymbolVariantItemSuffix,
    NormDependentPrefixMap,
};
use librepcb_core::library::dev::{Device, DevicePadSignalMapItem};
use librepcb_core::library::pkg::{AssemblyType, Footprint, Package, PackageModel, PackagePad};
use librepcb_core::library::sym::Symbol;
use librepcb_core::library::{BaseMetadata, LibraryBaseElement, LibraryElement};
use librepcb_core::types::{
    Angle, CircuitIdentifier, ElementName, Length, Point, SignalRole, Uuid, Version,
};
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_core::utils::tangent_path_joiner;
use librepcb_core::workspace::ElementKind;

use super::ImportedElementLookup;
use super::error::{Error, Result};
use super::type_converter::{KiCadTypeConverter as C, Line};
use super::types::{KiCadFootprint, KiCadPinStyle, KiCadPinType, KiCadSymbol, KiCadSymbolGate};
use crate::UuidGenerator;

/// Settings of the [`KiCadLibraryConverter`].
#[derive(Debug, Clone)]
pub struct KiCadLibraryConverterSettings {
    /// Source of the UUIDs of created objects.
    pub uuids: UuidGenerator,
    /// Prefix added to the names of all elements (e.g. `"KICAD_"`).
    pub name_prefix: String,
    /// Version of the created elements.
    pub version: Version,
    /// Author of the created elements.
    pub author: String,
    /// Creation date of the created elements.
    pub created: DateTime<Utc>,
    /// Keywords of the created elements.
    pub keywords: String,
    /// Categories of the created symbols.
    pub symbol_categories: BTreeSet<Uuid>,
    /// Categories of the created packages.
    pub package_categories: BTreeSet<Uuid>,
    /// Categories of the created components.
    pub component_categories: BTreeSet<Uuid>,
    /// Categories of the created devices.
    pub device_categories: BTreeSet<Uuid>,
}

impl Default for KiCadLibraryConverterSettings {
    fn default() -> Self {
        Self {
            uuids: UuidGenerator::random(),
            name_prefix: String::new(),
            version: "0.1".parse().expect("valid version"),
            author: "KiCad Import".to_owned(),
            created: Utc::now(),
            keywords: "kicad,import".to_owned(),
            symbol_categories: BTreeSet::new(),
            package_categories: BTreeSet::new(),
            component_categories: BTreeSet::new(),
            device_categories: BTreeSet::new(),
        }
    }
}

fn try_or_log(log: &MessageLogger<'_>, f: impl FnOnce() -> Result<()>) {
    if let Err(e) = f() {
        log.critical(&e.to_string());
    }
}

fn open_dir(fp: &FilePath) -> Result<TransactionalDirectory> {
    Ok(TransactionalDirectory::new(
        Arc::new(TransactionalFileSystem::open_ro(fp)?),
        "",
    ))
}

/// Converter of KiCad library elements into LibrePCB library elements
/// (upstream `KiCadLibraryConverter`).
pub struct KiCadLibraryConverter<'a> {
    lookup: &'a dyn ImportedElementLookup,
    settings: KiCadLibraryConverterSettings,
    tc: C,
    /// Package UUIDs by generated_by.
    package_map: HashMap<String, Uuid>,
    /// Pad name → package pad UUID by package generated_by.
    package_pad_map: HashMap<String, BTreeMap<String, Uuid>>,
    /// Symbol UUIDs by generated_by.
    symbol_map: HashMap<String, Uuid>,
    /// Symbol pin UUIDs by (symbol generated_by, pin name).
    symbol_pin_map: HashMap<(String, String), Uuid>,
    /// Component UUIDs by generated_by.
    component_map: HashMap<String, Uuid>,
    /// Component signal UUIDs by (component generated_by, signal name).
    component_signal_map: HashMap<(String, String), Uuid>,
}

impl<'a> KiCadLibraryConverter<'a> {
    /// Creates a converter looking up already imported dependencies in
    /// `lookup`.
    pub fn new(
        lookup: &'a dyn ImportedElementLookup,
        settings: KiCadLibraryConverterSettings,
    ) -> Self {
        let tc = C::new(settings.uuids.clone());
        Self {
            lookup,
            settings,
            tc,
            package_map: HashMap::new(),
            package_pad_map: HashMap::new(),
            symbol_map: HashMap::new(),
            symbol_pin_map: HashMap::new(),
            component_map: HashMap::new(),
            component_signal_map: HashMap::new(),
        }
    }

    fn metadata(&self, name: ElementName, description: String, keywords: String) -> BaseMetadata {
        let s = &self.settings;
        BaseMetadata::new(
            s.uuids.generate(),
            s.version.clone(),
            s.author.clone(),
            s.created,
            name,
            description,
            keywords,
        )
    }

    fn name(&self, name: &str) -> Result<ElementName> {
        C::convert_element_name(&format!("{}{}", self.settings.name_prefix, name))
    }

    /// Converts a footprint. `lib_fp` is the `*.pretty` directory, `models`
    /// maps the model paths of the footprint to STEP files.
    pub fn create_package(
        &mut self,
        lib_fp: &FilePath,
        ki_fpt: &KiCadFootprint,
        generated_by: &str,
        models: &BTreeMap<String, FilePath>,
        log: &MessageLogger<'_>,
    ) -> Result<Package> {
        if self.package_map.contains_key(generated_by) {
            return Err(Error::DuplicateImport);
        }
        if ki_fpt.layer != super::types::KiCadLayer::FrontCopper {
            return Err(Error::UnsupportedFootprintSide);
        }
        let mut package = Package::new(
            self.metadata(
                self.name(&ki_fpt.name)?,
                C::convert_element_description(lib_fp.basename(), &ki_fpt.name, &ki_fpt.properties),
                C::convert_element_keywords(&self.settings.keywords, &ki_fpt.properties),
            ),
            AssemblyType::Auto,
        )?;
        let meta = package.element_metadata_mut();
        meta.set_generated_by(generated_by.to_owned());
        meta.set_categories(self.settings.package_categories.clone());
        meta.set_resources(C::convert_resources(&ki_fpt.properties));

        // Assembly type.
        let exclude = ki_fpt.exclude_from_bom || ki_fpt.exclude_from_pos_files;
        if exclude {
            package.set_assembly_type(AssemblyType::None);
        } else if ki_fpt.is_smd && !ki_fpt.is_through_hole {
            package.set_assembly_type(AssemblyType::Smt);
        } else if !ki_fpt.is_smd && ki_fpt.is_through_hole {
            package.set_assembly_type(AssemblyType::Tht);
        } else if ki_fpt.is_smd && ki_fpt.is_through_hole {
            package.set_assembly_type(AssemblyType::Mixed);
        }

        // Footprint.
        let mut footprint = Footprint::new(
            self.settings.uuids.generate(),
            ElementName::new("default")?,
            String::new(),
        );
        let tc = &self.tc;

        // Geometry.
        let mut lines: Vec<Line> = Vec::new();
        for line in &ki_fpt.lines {
            try_or_log(log, || {
                lines.push(C::convert_footprint_line(line)?);
                Ok(())
            });
        }
        for arc in &ki_fpt.arcs {
            try_or_log(log, || {
                lines.push(C::convert_footprint_arc(arc)?);
                Ok(())
            });
        }
        let mut timed_out = false;
        let timeout = if generated_by.contains("TerminalBlock_WAGO") {
            500
        } else {
            5000
        };
        for group in C::group_lines_by_layer_and_width(&lines) {
            let joined =
                tangent_path_joiner::join(group.paths, Some(Duration::from_millis(timeout)));
            timed_out |= joined.timed_out;
            for path in joined.paths {
                footprint
                    .polygons_mut()
                    .push(librepcb_core::geometry::Polygon::new(
                        self.settings.uuids.generate(),
                        group.layer,
                        group.width,
                        false,
                        false,
                        path,
                    ));
            }
        }
        if timed_out {
            log.info(
                "Aborted joining tangent line segments to polygons due to timeout, keeping them \
                 separate.",
            );
        }
        for circle in &ki_fpt.circles {
            try_or_log(log, || {
                footprint
                    .circles_mut()
                    .push(tc.convert_footprint_circle(circle)?);
                Ok(())
            });
        }
        for rect in &ki_fpt.rectangles {
            try_or_log(log, || {
                footprint
                    .polygons_mut()
                    .push(tc.convert_footprint_rectangle(rect)?);
                Ok(())
            });
        }
        for polygon in &ki_fpt.polygons {
            try_or_log(log, || {
                footprint
                    .polygons_mut()
                    .push(tc.convert_footprint_polygon(polygon)?);
                Ok(())
            });
        }
        for property in &ki_fpt.properties {
            try_or_log(log, || {
                if let Some(st) = tc.convert_footprint_property_to_text(property)? {
                    footprint.stroke_texts_mut().push(st);
                }
                Ok(())
            });
        }
        for ki_text in &ki_fpt.texts {
            try_or_log(log, || {
                if let Some(st) = tc.convert_footprint_text(ki_text)? {
                    footprint.stroke_texts_mut().push(st);
                }
                Ok(())
            });
        }

        // Zones.
        for ki_zone in &ki_fpt.zones {
            try_or_log(log, || {
                if let Some(zone) = tc.convert_footprint_zone(ki_zone, log)? {
                    footprint.zones_mut().push(zone);
                }
                Ok(())
            });
        }

        // Pads.
        let mut pad_map = BTreeMap::new();
        for obj in &ki_fpt.pads {
            try_or_log(log, || {
                let res = tc.convert_pad(
                    obj,
                    ki_fpt.solder_mask_margin,
                    ki_fpt.solder_paste_margin,
                    ki_fpt.solder_paste_ratio,
                    ki_fpt.clearance,
                    log,
                )?;
                if let Some(mut fpt_pad) = res.fpt_pad {
                    let mut pkg_pad_uuid = package
                        .pads()
                        .by_name(&obj.number, true)
                        .map(PackagePad::uuid);
                    if pkg_pad_uuid.is_none() && !obj.number.is_empty() {
                        let pkg_pad =
                            PackagePad::new(fpt_pad.uuid(), CircuitIdentifier::new(&obj.number)?);
                        pad_map.insert(pkg_pad.name().as_str().to_owned(), pkg_pad.uuid());
                        pkg_pad_uuid = Some(pkg_pad.uuid());
                        package.pads_mut().push(pkg_pad);
                    }
                    if pkg_pad_uuid.is_some() {
                        fpt_pad.set_package_pad_uuid(pkg_pad_uuid);
                    }
                    footprint.pads_mut().push(fpt_pad);
                }
                if let Some(hole) = res.hole {
                    footprint.holes_mut().push(hole);
                }
                for polygon in res.polygons {
                    footprint.polygons_mut().push(polygon);
                }
                Ok(())
            });
        }

        // 3D models.
        for ki_model in &ki_fpt.models {
            try_or_log(log, || {
                if let Some(fp) = models.get(&ki_model.path) {
                    let uuid = self.settings.uuids.generate();
                    let model = PackageModel::new(
                        uuid,
                        ElementName::new(ElementName::clean(fp.complete_basename()))?,
                    );
                    let content = librepcb_core::fileio::file_utils::read_file(fp)?;
                    package
                        .directory_mut()
                        .write(&model.file_name(), &content)?;
                    package.models_mut().push(model);
                    let [x, y, z] = ki_model.offset.map(f64::from);
                    footprint.set_model_position((
                        Length::from_mm(x)?,
                        Length::from_mm(y)?,
                        Length::from_mm(z)?,
                    ));
                    let [rx, ry, rz] = ki_model.rotate.map(f64::from);
                    footprint.set_model_rotation((
                        Angle::from_deg(rx)?,
                        Angle::from_deg(ry)?,
                        -Angle::from_deg(rz)?,
                    ));
                    if ki_model.scale == [1.0, 1.0, 1.0] {
                        footprint.set_models(BTreeSet::from([uuid]));
                    } else {
                        log.warning("Scale factor on 3D model is not supported, will be ignored.");
                    }
                }
                Ok(())
            });
        }

        package.footprints_mut().push(footprint);
        self.package_pad_map
            .insert(generated_by.to_owned(), pad_map);
        self.package_map
            .insert(generated_by.to_owned(), package.metadata().uuid());
        Ok(package)
    }

    /// Converts a gate of a symbol into a symbol.
    pub fn create_symbol(
        &mut self,
        lib_fp: &FilePath,
        ki_sym: &KiCadSymbol,
        ki_gate: &KiCadSymbolGate,
        generated_by: &str,
        log: &MessageLogger<'_>,
    ) -> Result<Symbol> {
        if self.symbol_map.contains_key(generated_by) {
            return Err(Error::DuplicateImport);
        }
        let mut symbol = Symbol::new(self.metadata(
            self.name(&ki_gate.name)?,
            C::convert_element_description(lib_fp.basename(), &ki_gate.name, &ki_sym.properties),
            C::convert_element_keywords(&self.settings.keywords, &ki_sym.properties),
        ))?;
        let meta = symbol.element_metadata_mut();
        meta.set_generated_by(generated_by.to_owned());
        meta.set_categories(self.settings.symbol_categories.clone());
        meta.set_resources(C::convert_resources(&ki_sym.properties));
        let tc = &self.tc;

        // Geometries.
        for arc in &ki_gate.arcs {
            try_or_log(log, || {
                symbol.polygons_mut().push(tc.convert_symbol_arc(arc)?);
                Ok(())
            });
        }
        for circle in &ki_gate.circles {
            try_or_log(log, || {
                symbol.circles_mut().push(tc.convert_symbol_circle(circle)?);
                Ok(())
            });
        }
        for rect in &ki_gate.rectangles {
            try_or_log(log, || {
                symbol
                    .polygons_mut()
                    .push(tc.convert_symbol_rectangle(rect)?);
                Ok(())
            });
        }
        for polyline in &ki_gate.polylines {
            try_or_log(log, || {
                symbol
                    .polygons_mut()
                    .push(tc.convert_symbol_polyline(polyline)?);
                Ok(())
            });
        }
        for ki_text in &ki_gate.texts {
            try_or_log(log, || {
                symbol.texts_mut().push(tc.convert_symbol_text(ki_text)?);
                Ok(())
            });
        }
        for property in &ki_sym.properties {
            try_or_log(log, || {
                if let Some(t) = tc.convert_symbol_property_to_text(property)? {
                    symbol.texts_mut().push(t);
                }
                Ok(())
            });
        }

        // Pins.
        let pin_names = C::convert_symbol_pin_names(&ki_gate.pins);
        for (pin, (pin_name, _)) in ki_gate.pins.iter().zip(&pin_names) {
            if pin_name.is_empty() {
                continue;
            }
            try_or_log(log, || {
                let pin = tc.convert_symbol_pin(pin, pin_name, ki_sym.pin_names_offset)?;
                self.symbol_pin_map
                    .insert((generated_by.to_owned(), pin_name.clone()), pin.uuid());
                symbol.pins_mut().push(pin);
                Ok(())
            });
        }

        self.symbol_map
            .insert(generated_by.to_owned(), symbol.metadata().uuid());
        Ok(symbol)
    }

    /// Converts a symbol into a component (its gates must have been
    /// converted before or been imported already).
    pub fn create_component(
        &mut self,
        lib_fp: &FilePath,
        ki_sym: &KiCadSymbol,
        ki_gates: &[KiCadSymbolGate],
        generated_by: &str,
        sym_generated_by: &[String],
        _log: &MessageLogger<'_>,
    ) -> Result<Component> {
        if ki_gates.len() != sym_generated_by.len() {
            return Err(Error::Logic("gate count mismatch"));
        }
        if self.component_map.contains_key(generated_by) {
            return Err(Error::DuplicateImport);
        }
        for sym_gen_by in sym_generated_by {
            if !self.symbol_map.contains_key(sym_gen_by) {
                self.load_already_imported_symbol(sym_gen_by)?;
            }
        }
        let mut component = Component::new(self.metadata(
            self.name(&ki_sym.name)?,
            C::convert_element_description(lib_fp.basename(), &ki_sym.name, &ki_sym.properties),
            C::convert_element_keywords(&self.settings.keywords, &ki_sym.properties),
        ))?;
        let meta = component.element_metadata_mut();
        meta.set_generated_by(generated_by.to_owned());
        meta.set_categories(self.settings.component_categories.clone());
        meta.set_resources(C::convert_resources(&ki_sym.properties));
        component.set_schematic_only(!ki_sym.on_board);
        if let Some(p) = C::find_property(&ki_sym.properties, "reference") {
            component.set_prefixes(NormDependentPrefixMap::new(ComponentPrefix::new(
                ComponentPrefix::clean(&p.value),
            )?));
        }
        component.set_default_value("{{ MPN or DEVICE }}".to_owned());
        let mut symbol_variant = ComponentSymbolVariant::new(
            self.settings.uuids.generate(),
            "",
            ElementName::new("default")?,
            "",
        );

        let add_gate_suffixes = ki_gates.len() > 1;
        const SUFFIXES: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        for (i, (gate, sym_gen_by)) in ki_gates.iter().zip(sym_generated_by).enumerate() {
            let symbol_uuid = *self
                .symbol_map
                .get(sym_gen_by)
                .ok_or(Error::Logic("symbol not found"))?;
            let suffix = if add_gate_suffixes {
                SUFFIXES.get(i..i + 1).unwrap_or_default()
            } else {
                ""
            };
            let mut item = ComponentSymbolVariantItem::new(
                self.settings.uuids.generate(),
                symbol_uuid,
                Point::ORIGIN,
                Angle::DEG0,
                true,
                ComponentSymbolVariantItemSuffix::new(suffix)?,
            );

            let pin_names = C::convert_symbol_pin_names(&gate.pins);
            for (pin, (pin_name, _)) in gate.pins.iter().zip(&pin_names) {
                if pin_name.is_empty() {
                    continue;
                }
                let signal_uuid = self.settings.uuids.generate();
                let role = match pin.pin_type {
                    KiCadPinType::Input => SignalRole::Input,
                    KiCadPinType::Output => SignalRole::Output,
                    KiCadPinType::Bidirectional => SignalRole::InOut,
                    KiCadPinType::PowerIn | KiCadPinType::PowerOut => SignalRole::Power,
                    KiCadPinType::OpenCollector => SignalRole::OpenDrain,
                    _ => SignalRole::Passive,
                };
                let is_negated = matches!(
                    pin.shape,
                    KiCadPinStyle::Inverted | KiCadPinStyle::InvertedClock
                );
                let is_clock = matches!(
                    pin.shape,
                    KiCadPinStyle::Clock
                        | KiCadPinStyle::ClockLow
                        | KiCadPinStyle::EdgeClockHigh
                        | KiCadPinStyle::InvertedClock
                );
                component.signals_mut().push(ComponentSignal::new(
                    signal_uuid,
                    CircuitIdentifier::new(pin_name.as_str())?,
                    role,
                    "",
                    false,
                    is_negated,
                    is_clock,
                ));
                let pin_uuid = *self
                    .symbol_pin_map
                    .get(&(sym_gen_by.clone(), pin_name.clone()))
                    .ok_or_else(|| Error::PinNotFound(pin_name.clone()))?;
                item.pin_signal_map_mut()
                    .push(ComponentPinSignalMapItem::new(
                        pin_uuid,
                        Some(signal_uuid),
                        CmpSigPinDisplayType::ComponentSignal,
                    ));
                self.component_signal_map
                    .insert((generated_by.to_owned(), pin_name.clone()), signal_uuid);
            }
            symbol_variant.symbol_items_mut().push(item);
        }
        component.symbol_variants_mut().push(symbol_variant);
        self.component_map
            .insert(generated_by.to_owned(), component.metadata().uuid());
        Ok(component)
    }

    /// Converts a symbol into a device (its component and package must have
    /// been converted before or been imported already).
    #[allow(clippy::too_many_arguments)]
    pub fn create_device(
        &mut self,
        lib_fp: &FilePath,
        ki_sym: &KiCadSymbol,
        ki_gates: &[KiCadSymbolGate],
        generated_by: &str,
        cmp_generated_by: &str,
        pkg_generated_by: &str,
        _log: &MessageLogger<'_>,
    ) -> Result<Device> {
        if !self.component_map.contains_key(cmp_generated_by) {
            self.load_already_imported_component(cmp_generated_by)?;
        }
        let component_uuid = *self
            .component_map
            .get(cmp_generated_by)
            .ok_or(Error::Logic("component not found"))?;
        let mut pkg_generated_by = pkg_generated_by.to_owned();
        if !self.package_map.contains_key(&pkg_generated_by) {
            // Compatibility with Ultra Librarian which stores footprints in
            // footprints.pretty but references them without that namespace.
            let alias = pkg_generated_by.replace("KiCadImport::", "KiCadImport::footprints::");
            if self.package_map.contains_key(&alias) {
                pkg_generated_by = alias;
            } else {
                pkg_generated_by =
                    self.load_already_imported_package(&[pkg_generated_by.clone(), alias])?;
            }
        }
        let package_uuid = *self
            .package_map
            .get(&pkg_generated_by)
            .ok_or(Error::Logic("package not found"))?;
        let mut device = Device::new(
            self.metadata(
                self.name(&ki_sym.name)?,
                C::convert_element_description(lib_fp.basename(), &ki_sym.name, &ki_sym.properties),
                C::convert_element_keywords(&self.settings.keywords, &ki_sym.properties),
            ),
            component_uuid,
            package_uuid,
        )?;
        let meta = device.element_metadata_mut();
        meta.set_generated_by(generated_by.to_owned());
        meta.set_categories(self.settings.device_categories.clone());
        meta.set_resources(C::convert_resources(&ki_sym.properties));

        let gate_pin_names: Vec<Vec<(String, Vec<String>)>> = ki_gates
            .iter()
            .map(|g| C::convert_symbol_pin_names(&g.pins))
            .collect();
        let mut connected_pads = BTreeSet::new();
        if let Some(pads) = self.package_pad_map.get(&pkg_generated_by) {
            for (pad_name, pad_uuid) in pads {
                let mut signal_uuid = None;
                for (pin_name, numbers) in gate_pin_names.iter().flatten() {
                    if numbers.contains(pad_name) {
                        signal_uuid = self
                            .component_signal_map
                            .get(&(cmp_generated_by.to_owned(), pin_name.clone()))
                            .copied();
                        connected_pads.insert(pad_name.clone());
                    }
                }
                device
                    .pad_signal_map_mut()
                    .push(DevicePadSignalMapItem::new(*pad_uuid, signal_uuid, false));
            }
        }

        // Fail if the symbol specifies pad numbers we didn't find in the
        // package.
        for (_, numbers) in gate_pin_names.iter().flatten() {
            for pad_number in numbers {
                if !connected_pads.contains(pad_number) {
                    return Err(Error::PadNotFound(pad_number.clone()));
                }
            }
        }
        Ok(device)
    }

    fn load_already_imported_symbol(&mut self, generated_by: &str) -> Result<()> {
        let fp = self.already_imported_fp(ElementKind::Symbol, &[generated_by.to_owned()])?;
        let symbol = Symbol::open(open_dir(&fp)?)?;
        for pin in symbol.pins() {
            self.symbol_pin_map.insert(
                (generated_by.to_owned(), pin.name().as_str().to_owned()),
                pin.uuid(),
            );
        }
        self.symbol_map
            .insert(generated_by.to_owned(), symbol.metadata().uuid());
        Ok(())
    }

    fn load_already_imported_package(&mut self, generated_by: &[String]) -> Result<String> {
        let fp = self.already_imported_fp(ElementKind::Package, generated_by)?;
        let package = Package::open(open_dir(&fp)?)?;
        let gen_by = package.element_metadata().generated_by().clone();
        let pads = self.package_pad_map.entry(gen_by.clone()).or_default();
        for pad in package.pads() {
            pads.insert(pad.name().as_str().to_owned(), pad.uuid());
        }
        self.package_map
            .insert(gen_by.clone(), package.metadata().uuid());
        Ok(gen_by)
    }

    fn load_already_imported_component(&mut self, generated_by: &str) -> Result<()> {
        let fp = self.already_imported_fp(ElementKind::Component, &[generated_by.to_owned()])?;
        let component = Component::open(open_dir(&fp)?)?;
        for signal in component.signals() {
            self.component_signal_map.insert(
                (generated_by.to_owned(), signal.name().as_str().to_owned()),
                signal.uuid(),
            );
        }
        self.component_map
            .insert(generated_by.to_owned(), component.metadata().uuid());
        Ok(())
    }

    fn already_imported_fp(&self, kind: ElementKind, generated_by: &[String]) -> Result<FilePath> {
        for gen_by in generated_by {
            for uuid in self.lookup.generated(kind, gen_by) {
                if let Some(fp) = self.lookup.latest(kind, uuid) {
                    return Ok(fp);
                }
            }
        }
        let reference = generated_by
            .first()
            .map(|g| g.replace("KiCadImport::", "").replace("::", ":"))
            .unwrap_or_default();
        Err(Error::DependencyNotFound {
            kind: match kind {
                ElementKind::Symbol => Symbol::LONG_ELEMENT_NAME,
                ElementKind::Package => Package::LONG_ELEMENT_NAME,
                ElementKind::Component => Component::LONG_ELEMENT_NAME,
                _ => Device::LONG_ELEMENT_NAME,
            },
            reference,
        })
    }
}
