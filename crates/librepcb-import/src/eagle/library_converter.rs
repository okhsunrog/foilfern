//! Port of libs/librepcb/eagleimport/eaglelibraryconverter.{h,cpp}.
//!
//! Converts EAGLE symbols, packages, device sets and devices into LibrePCB
//! symbols, packages, components and devices, remembering the created UUIDs
//! to resolve the references between them.
//!
//! Differences to upstream:
//! - Overlapping grab areas of symbols are detected on the union of the
//!   previous grab areas computed with Clipper (upstream `QPainterPath`
//!   union, see COMPAT.md).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use librepcb_core::attribute::AttributeList;
use librepcb_core::geometry::{Path, Polygon, ZoneLayers, ZoneRules};
use librepcb_core::library::cmp::{
    CmpSigPinDisplayType, Component, ComponentPinSignalMapItem, ComponentSignal,
    ComponentSymbolVariant, ComponentSymbolVariantItem, NormDependentPrefixMap,
};
use librepcb_core::library::dev::{Device, DevicePadSignalMapItem, Part};
use librepcb_core::library::pkg::{AssemblyType, Footprint, Package};
use librepcb_core::library::sym::Symbol;
use librepcb_core::library::{BaseMetadata, LibraryElement};
use librepcb_core::types::{
    Angle, BoundedUnsignedRatio, ElementName, Layer, Length, PositiveLength, SignalRole,
    SimpleString, UnsignedLength, Uuid, Version,
};
use librepcb_core::utils::clipper_helpers;
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_core::utils::painter_path::PainterPathPx;
use librepcb_i18n::tr;

use super::error::{Error, Result};
use super::model;
use super::type_converter::{EagleTypeConverter, Geometry};
use crate::UuidGenerator;

const CTX: &str = "librepcb::eagleimport::EagleLibraryConverter";

/// Settings of the [`EagleLibraryConverter`].
#[derive(Debug, Clone)]
pub struct EagleLibraryConverterSettings {
    /// Source of the UUIDs of created objects.
    pub uuids: UuidGenerator,
    /// Prefix added to the names of all elements (e.g. `"EAGLE_"`).
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
    /// Annular width of THT pads with "auto" size.
    pub auto_tht_annular_width: BoundedUnsignedRatio,
}

impl Default for EagleLibraryConverterSettings {
    fn default() -> Self {
        Self {
            uuids: UuidGenerator::random(),
            name_prefix: String::new(),
            version: "0.1".parse().expect("valid version"),
            author: "EAGLE Import".to_owned(),
            created: Utc::now(),
            keywords: "eagle,import".to_owned(),
            symbol_categories: BTreeSet::new(),
            package_categories: BTreeSet::new(),
            component_categories: BTreeSet::new(),
            device_categories: BTreeSet::new(),
            auto_tht_annular_width: EagleTypeConverter::default_auto_tht_annular_width(),
        }
    }
}

/// Key of an element: library name, library URN, element name.
type ElementKey = (String, String, String);

fn key(lib_name: &str, lib_urn: &str, name: &str) -> ElementKey {
    (lib_name.to_owned(), lib_urn.to_owned(), name.to_owned())
}

fn generated_by(lib_name: &str, keys: &[&str]) -> String {
    let lib_name = if lib_name.to_lowercase().ends_with(".lbr") {
        &lib_name[..lib_name.len() - 4]
    } else {
        lib_name
    };
    let mut parts = vec!["EagleImport", lib_name];
    parts.extend_from_slice(keys);
    parts.join("::")
}

/// Runs `f` and logs its error as critical message (upstream
/// `tryOrLogError()`).
fn try_or_log(log: &MessageLogger<'_>, f: impl FnOnce() -> Result<()>) {
    if let Err(e) = f() {
        log.critical(&e.to_string());
    }
}

/// Stable sort with a "less than" predicate which is not necessarily a
/// strict weak ordering (tolerances), like `std::stable_sort()` for the
/// consistent cases.
fn stable_sort_by_less<T>(items: &mut [T], less: impl Fn(&T, &T) -> bool) {
    for i in 1..items.len() {
        let mut j = i;
        while (j > 0) && less(&items[j], &items[j - 1]) {
            items.swap(j, j - 1);
            j -= 1;
        }
    }
}

/// The union of grab areas (upstream `QPainterPath` with winding fill,
/// extended by `|=`), tested with the `QPainterPath::contains()` emulation.
///
/// For speed, only the grab areas whose bounding box overlaps the tested
/// path are united (the containment test only depends on them).
#[derive(Default)]
struct GrabAreaUnion {
    /// Flattened grab areas with their bounding boxes.
    areas: Vec<(clipper_helpers::ClipperPath, (i64, i64, i64, i64))>,
}

fn tolerance() -> PositiveLength {
    PositiveLength::new(Length::new(5_000)).expect("constant is positive")
}

fn bounds(path: &clipper_helpers::ClipperPath) -> (i64, i64, i64, i64) {
    path.iter().fold(
        (i64::MAX, i64::MAX, i64::MIN, i64::MIN),
        |(x0, y0, x1, y1), p| (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)),
    )
}

impl GrabAreaUnion {
    fn contains(&self, path: &Path) -> bool {
        let (x0, y0, x1, y1) = bounds(&clipper_helpers::path_to_clipper(path, tolerance()));
        let mut union: clipper_helpers::ClipperPaths = self
            .areas
            .iter()
            .filter(|(_, (ax0, ay0, ax1, ay1))| {
                (*ax0 <= x1) && (*ax1 >= x0) && (*ay0 <= y1) && (*ay1 >= y0)
            })
            .map(|(p, _)| p.clone())
            .collect();
        if union.is_empty()
            || clipper_helpers::unite(&mut union, clipper::PolyFillType::NonZero).is_err()
        {
            return false;
        }
        let polygons: Vec<Vec<(f64, f64)>> = union
            .iter()
            .map(|p| {
                let mut points: Vec<(f64, f64)> = p
                    .iter()
                    .map(|pt| clipper_helpers::point_from_clipper(*pt).to_px())
                    .collect();
                if let Some(first) = points.first().copied() {
                    points.push(first);
                }
                points
            })
            .collect();
        let mut total = PainterPathPx::from_polygons(&polygons);
        total.set_winding_fill(true);
        total.contains_path(&PainterPathPx::from_paths([path]))
    }

    fn add(&mut self, path: &Path) {
        let p = clipper_helpers::path_to_clipper(path, tolerance());
        let b = bounds(&p);
        self.areas.push((p, b));
    }
}

/// Converter of EAGLE library elements into LibrePCB library elements
/// (upstream `EagleLibraryConverter`).
#[derive(Debug)]
pub struct EagleLibraryConverter {
    settings: EagleLibraryConverterSettings,
    tc: EagleTypeConverter,
    /// Symbol UUIDs.
    symbol_map: HashMap<ElementKey, Uuid>,
    /// Pin name → (EAGLE pin, symbol pin UUID) per symbol.
    symbol_pin_map: HashMap<ElementKey, BTreeMap<String, (model::Pin, Uuid)>>,
    /// Package UUIDs.
    package_map: HashMap<ElementKey, Uuid>,
    /// Pad name → package pad UUID per package.
    package_pad_map: HashMap<ElementKey, BTreeMap<String, Uuid>>,
    /// Component UUIDs.
    component_map: HashMap<ElementKey, Uuid>,
    /// Component signal UUIDs by (component key, gate name, pin name).
    component_signal_map: HashMap<(ElementKey, String, String), Uuid>,
}

impl EagleLibraryConverter {
    /// Creates a converter.
    pub fn new(settings: EagleLibraryConverterSettings) -> Self {
        let tc = EagleTypeConverter::new(settings.uuids.clone());
        Self {
            settings,
            tc,
            symbol_map: HashMap::new(),
            symbol_pin_map: HashMap::new(),
            package_map: HashMap::new(),
            package_pad_map: HashMap::new(),
            component_map: HashMap::new(),
            component_signal_map: HashMap::new(),
        }
    }

    /// Returns the settings.
    pub fn settings(&self) -> &EagleLibraryConverterSettings {
        &self.settings
    }

    /// Returns the component signal of a converted gate pin.
    pub fn component_signal_of_symbol_pin(
        &self,
        lib_name: &str,
        lib_urn: &str,
        dev_set_name: &str,
        gate_name: &str,
        pin_name: &str,
    ) -> Result<Uuid> {
        self.component_signal_map
            .get(&(
                key(lib_name, lib_urn, dev_set_name),
                gate_name.to_owned(),
                pin_name.to_owned(),
            ))
            .copied()
            .ok_or_else(|| Error::ComponentSignalNotFound(pin_name.to_owned()))
    }

    /// Forgets all converted elements.
    pub fn reset(&mut self) {
        self.symbol_map.clear();
        self.symbol_pin_map.clear();
        self.package_map.clear();
        self.package_pad_map.clear();
        self.component_map.clear();
        self.component_signal_map.clear();
    }

    fn metadata(&self, name: ElementName, description: String) -> BaseMetadata {
        let s = &self.settings;
        BaseMetadata::new(
            s.uuids.generate(),
            s.version.clone(),
            s.author.clone(),
            s.created,
            name,
            description,
            s.keywords.clone(),
        )
    }

    /// Converts an EAGLE symbol.
    pub fn create_symbol(
        &mut self,
        lib_name: &str,
        lib_urn: &str,
        eagle_symbol: &model::Symbol,
        log: &MessageLogger<'_>,
    ) -> Result<Symbol> {
        let k = key(lib_name, lib_urn, &eagle_symbol.name);
        if self.symbol_map.contains_key(&k) {
            return Err(Error::DuplicateImport);
        }
        let tc = &self.tc;
        let mut symbol = Symbol::new(self.metadata(
            tc.convert_element_name(&format!(
                "{}{}",
                self.settings.name_prefix, eagle_symbol.name
            )),
            tc.convert_element_description(&eagle_symbol.description),
        ))?;
        let meta = symbol.element_metadata_mut();
        meta.set_generated_by(generated_by(lib_name, &[&eagle_symbol.name]));
        meta.set_categories(self.settings.symbol_categories.clone());

        // Enable grab areas on closed polygons. However, don't do this for
        // sheet frames as it would look ugly. We guess that by the absence of
        // pins.
        let grab_area = !eagle_symbol.pins.is_empty();
        let mut geometries = tc.convert_and_join_wires(&eagle_symbol.wires, grab_area, log);
        for obj in &eagle_symbol.rectangles {
            try_or_log(log, || {
                geometries.push(tc.convert_rectangle(obj, true)?);
                Ok(())
            });
        }
        for obj in &eagle_symbol.polygons {
            try_or_log(log, || {
                geometries.push(tc.convert_polygon(obj, true)?);
                Ok(())
            });
        }
        for obj in &eagle_symbol.circles {
            try_or_log(log, || {
                geometries.push(tc.convert_circle(obj, true)?);
                Ok(())
            });
        }
        for obj in &eagle_symbol.frames {
            try_or_log(log, || {
                geometries.push(tc.convert_frame(obj)?);
                Ok(())
            });
        }
        // Disable grab area on geometries located *within* another grab area
        // to avoid overlapping grab areas, but also to avoid triggering issue
        // https://github.com/LibrePCB/LibrePCB/issues/1278.
        stable_sort_by_less(&mut geometries, |a: &Geometry, b: &Geometry| {
            if a.path.is_closed() != b.path.is_closed() {
                return a.path.is_closed();
            }
            if a.path.is_closed() {
                // Use a tolerance to avoid sub-ULP floating-point differences
                // from producing platform-dependent orderings for
                // geometrically equivalent candidates.
                let a1 = a.path.calc_area_of_straight_segments();
                let a2 = b.path.calc_area_of_straight_segments();
                if (a1 - a2).abs() > 50e-9 {
                    return a1 > a2;
                }
            }
            let l1 = a.path.total_straight_length();
            let l2 = b.path.total_straight_length();
            if (*l1 - *l2).abs() < Length::new(50) {
                return l1 > l2;
            }
            false
        });
        let mut total_grab_area = GrabAreaUnion::default();
        for g in geometries.iter_mut().filter(|g| g.grab_area) {
            if total_grab_area.contains(&g.path) {
                g.grab_area = false;
            } else {
                total_grab_area.add(&g.path);
            }
        }
        for g in &geometries {
            if let Some(o) = tc.try_convert_to_schematic_circle(g) {
                symbol.circles_mut().push(o);
            } else if let Some(o) = tc.try_convert_to_schematic_polygon(g) {
                symbol.polygons_mut().push(o);
            } else {
                log.warning(&tr!(
                    CTX,
                    "Skipped graphics object on layer {0} ({1}).",
                    g.layer_id,
                    EagleTypeConverter::layer_name(g.layer_id, "unknown")
                ));
            }
        }
        for obj in &eagle_symbol.texts {
            match tc.try_convert_schematic_text(obj, true) {
                Ok(Some(t)) => {
                    symbol.texts_mut().push(t);
                }
                Ok(None) => log.warning(&tr!(
                    CTX,
                    "Skipped text on layer {0} ({1}).",
                    obj.layer,
                    EagleTypeConverter::layer_name(obj.layer, "unknown")
                )),
                Err(e) => log.critical(&e.to_string()),
            }
        }
        for obj in &eagle_symbol.pins {
            match tc.convert_symbol_pin(obj) {
                Ok(pin_obj) => {
                    self.symbol_pin_map
                        .entry(k.clone())
                        .or_default()
                        .insert(obj.name.clone(), (obj.clone(), pin_obj.pin.uuid()));
                    symbol.pins_mut().push(pin_obj.pin);
                    if let Some(c) = pin_obj.circle {
                        symbol.circles_mut().push(c);
                    }
                    if let Some(p) = pin_obj.polygon {
                        symbol.polygons_mut().push(p);
                    }
                }
                Err(e) => log.critical(&e.to_string()),
            }
        }
        self.symbol_map
            .insert(k, symbol.element_metadata().base().uuid());
        Ok(symbol)
    }

    /// Converts an EAGLE package.
    pub fn create_package(
        &mut self,
        lib_name: &str,
        lib_urn: &str,
        eagle_package: &model::Package,
        log: &MessageLogger<'_>,
    ) -> Result<Package> {
        let k = key(lib_name, lib_urn, &eagle_package.name);
        if self.package_map.contains_key(&k) {
            return Err(Error::DuplicateImport);
        }
        let tc = &self.tc;
        let mut package = Package::new(
            self.metadata(
                tc.convert_element_name(&format!(
                    "{}{}",
                    self.settings.name_prefix, eagle_package.name
                )),
                tc.convert_element_description(&eagle_package.description),
            ),
            AssemblyType::Auto,
        )?;
        let meta = package.element_metadata_mut();
        meta.set_generated_by(generated_by(lib_name, &[&eagle_package.name]));
        meta.set_categories(self.settings.package_categories.clone());
        let mut footprint = Footprint::new(
            self.settings.uuids.generate(),
            ElementName::new("default")?,
            String::new(),
        );

        let mut geometries = tc.convert_and_join_wires(&eagle_package.wires, false, log);
        for obj in &eagle_package.rectangles {
            try_or_log(log, || {
                geometries.push(tc.convert_rectangle(obj, false)?);
                Ok(())
            });
        }
        for obj in &eagle_package.polygons {
            try_or_log(log, || {
                geometries.push(tc.convert_polygon(obj, false)?);
                Ok(())
            });
        }
        for obj in &eagle_package.circles {
            try_or_log(log, || {
                geometries.push(tc.convert_circle(obj, false)?);
                Ok(())
            });
        }
        for g in &geometries {
            try_or_log(log, || {
                let zones = tc.try_convert_to_board_zones(g)?;
                if !zones.is_empty() {
                    for o in zones {
                        footprint.zones_mut().push(o);
                    }
                } else if let Some(o) = tc.try_convert_to_board_circle(g) {
                    footprint.circles_mut().push(o);
                } else if let Some(o) = tc.try_convert_to_board_polygon(g) {
                    footprint.polygons_mut().push(o);
                } else {
                    log.warning(&tr!(
                        CTX,
                        "Skipped graphics object on layer {0} ({1}).",
                        g.layer_id,
                        EagleTypeConverter::layer_name(g.layer_id, "unknown")
                    ));
                }
                Ok(())
            });
        }
        for obj in &eagle_package.texts {
            match tc.try_convert_board_text(obj, true) {
                Ok(Some(t)) => {
                    footprint.stroke_texts_mut().push(t);
                }
                Ok(None) => log.warning(&tr!(
                    CTX,
                    "Skipped text on layer {0} ({1}).",
                    obj.layer,
                    EagleTypeConverter::layer_name(obj.layer, "unknown")
                )),
                Err(e) => log.critical(&e.to_string()),
            }
        }
        for obj in &eagle_package.holes {
            try_or_log(log, || {
                footprint.holes_mut().push(tc.convert_hole(obj)?);
                Ok(())
            });
        }
        let mut pad_map = BTreeMap::new();
        for obj in &eagle_package.tht_pads {
            try_or_log(log, || {
                let (pkg_pad, fpt_pad) =
                    tc.convert_tht_pad(obj, &self.settings.auto_tht_annular_width)?;
                pad_map.insert(obj.name.clone(), pkg_pad.uuid());
                package.pads_mut().push(pkg_pad);
                footprint.pads_mut().push(fpt_pad);
                Ok(())
            });
        }
        for obj in &eagle_package.smt_pads {
            try_or_log(log, || {
                let (pkg_pad, fpt_pad) = tc.convert_smt_pad(obj)?;
                pad_map.insert(obj.name.clone(), pkg_pad.uuid());
                package.pads_mut().push(pkg_pad);
                footprint.pads_mut().push(fpt_pad);
                Ok(())
            });
        }
        // If there is exactly one device keepout zone on the top layer,
        // convert it to a package courtyard polygon.
        if footprint.zones().len() == 1 {
            let zone = &footprint.zones().as_slice()[0];
            if (zone.layers() == ZoneLayers::TOP) && (zone.rules() == ZoneRules::NO_DEVICES) {
                let outline = zone.outline().clone();
                footprint.polygons_mut().push(Polygon::new(
                    self.settings.uuids.generate(),
                    Layer::TOP_COURTYARD,
                    UnsignedLength::ZERO,
                    false,
                    false,
                    outline,
                ));
                footprint.zones_mut().clear();
            }
        }
        package.footprints_mut().push(footprint);
        self.package_pad_map.insert(k.clone(), pad_map);
        self.package_map
            .insert(k, package.element_metadata().base().uuid());
        Ok(package)
    }

    /// Converts an EAGLE device set (its symbols must have been converted
    /// before).
    pub fn create_component(
        &mut self,
        lib_name: &str,
        lib_urn: &str,
        eagle_device_set: &model::DeviceSet,
        _log: &MessageLogger<'_>,
    ) -> Result<Component> {
        use model::{PinDirection as D, PinFunction as F, PinVisibility as V};
        let k = key(lib_name, lib_urn, &eagle_device_set.name);
        if self.component_map.contains_key(&k) {
            return Err(Error::DuplicateImport);
        }
        let tc = &self.tc;
        let mut component = Component::new(self.metadata(
            tc.convert_component_name(&format!(
                "{}{}",
                self.settings.name_prefix, eagle_device_set.name
            )),
            tc.convert_element_description(&eagle_device_set.description),
        ))?;
        let meta = component.element_metadata_mut();
        meta.set_generated_by(generated_by(lib_name, &[&eagle_device_set.name]));
        meta.set_categories(self.settings.component_categories.clone());
        component.set_prefixes(NormDependentPrefixMap::new(
            tc.convert_component_prefix(&eagle_device_set.prefix),
        ));
        component.set_default_value(if eagle_device_set.user_value {
            "{{ MPN }}".to_owned()
        } else {
            "{{ MPN or DEVICE or COMPONENT }}".to_owned()
        });
        let mut symbol_variant = ComponentSymbolVariant::new(
            self.settings.uuids.generate(),
            "",
            ElementName::new("default")?,
            "",
        );
        let empty = BTreeMap::new();
        let symbol_pins = |symbol: &str| {
            self.symbol_pin_map
                .get(&key(lib_name, lib_urn, symbol))
                .unwrap_or(&empty)
        };
        let mut pin_count: HashMap<&str, usize> = HashMap::new();
        for gate in &eagle_device_set.gates {
            for pin_name in symbol_pins(&gate.symbol).keys() {
                *pin_count.entry(pin_name).or_default() += 1;
            }
        }
        let add_gate_suffixes = eagle_device_set.gates.len() > 1;
        let mut new_signals = Vec::new();
        for gate in &eagle_device_set.gates {
            let symbol_uuid = self
                .symbol_map
                .get(&key(lib_name, lib_urn, &gate.symbol))
                .copied()
                .ok_or_else(|| Error::DependentSymbolNotImported(gate.symbol.clone()))?;
            let mut item = ComponentSymbolVariantItem::new(
                self.settings.uuids.generate(),
                symbol_uuid,
                tc.convert_point(gate.position)?,
                Angle::DEG0,
                true,
                tc.convert_gate_name(if add_gate_suffixes { &gate.name } else { "" }),
            );
            for (pin_name, (pin, pin_uuid)) in symbol_pins(&gate.symbol) {
                let signal_uuid = self.settings.uuids.generate();
                let mut signal_name = pin_name.clone();
                if (pin_count.get(pin_name.as_str()).copied().unwrap_or(0) > 1)
                    || component.signals().contains_name(&signal_name)
                {
                    // Name conflict -> add prefix to ensure unique signal
                    // names.
                    signal_name = format!("{}_{}", item.suffix().as_str(), signal_name);
                }
                let signal_role = match pin.direction {
                    D::Input => SignalRole::Input,
                    D::Output => SignalRole::Output,
                    D::Io => SignalRole::InOut,
                    D::OpenCollector => SignalRole::OpenDrain,
                    D::Power => SignalRole::Power,
                    _ => SignalRole::Passive,
                };
                let display_type = match pin.visibility {
                    V::Off | V::Pad => CmpSigPinDisplayType::None,
                    _ => CmpSigPinDisplayType::ComponentSignal,
                };
                let forced_net_name = if pin.direction == D::Supply {
                    pin_name.clone()
                } else {
                    String::new()
                };
                let is_negated = matches!(pin.function, F::Dot | F::DotClock);
                let is_clock = matches!(pin.function, F::Clock | F::DotClock);
                component.signals_mut().push(ComponentSignal::new(
                    signal_uuid,
                    tc.convert_pin_or_pad_name(&signal_name),
                    signal_role,
                    forced_net_name,
                    false,
                    is_negated,
                    is_clock,
                ));
                item.pin_signal_map_mut()
                    .push(ComponentPinSignalMapItem::new(
                        *pin_uuid,
                        Some(signal_uuid),
                        display_type,
                    ));
                new_signals.push((
                    (k.clone(), gate.name.clone(), pin_name.clone()),
                    signal_uuid,
                ));
            }
            symbol_variant.symbol_items_mut().push(item);
        }
        component.symbol_variants_mut().push(symbol_variant);
        // If the device set has no package at all, we consider it as a
        // schematic-only component to avoid the "unplaced devices" warning
        // in the board editor.
        let has_package = eagle_device_set
            .devices
            .iter()
            .any(|d| !d.package.is_empty());
        component.set_schematic_only(!has_package);
        self.component_signal_map.extend(new_signals);
        self.component_map
            .insert(k, component.element_metadata().base().uuid());
        Ok(component)
    }

    /// Converts an EAGLE device (its component and package must have been
    /// converted before).
    #[allow(clippy::too_many_arguments)]
    pub fn create_device(
        &mut self,
        dev_lib_name: &str,
        dev_lib_urn: &str,
        eagle_device_set: &model::DeviceSet,
        eagle_device: &model::Device,
        pkg_lib_name: &str,
        pkg_lib_urn: &str,
        log: &MessageLogger<'_>,
    ) -> Result<Device> {
        let component_key = key(dev_lib_name, dev_lib_urn, &eagle_device_set.name);
        let component_uuid = self
            .component_map
            .get(&component_key)
            .copied()
            .ok_or_else(|| Error::DependentComponentNotImported(eagle_device_set.name.clone()))?;
        let package_key = key(pkg_lib_name, pkg_lib_urn, &eagle_device.package);
        let package_uuid = self
            .package_map
            .get(&package_key)
            .copied()
            .ok_or_else(|| Error::DependentPackageNotImported(eagle_device.package.clone()))?;
        let tc = &self.tc;
        let mut device = Device::new(
            self.metadata(
                tc.convert_device_name(
                    &format!("{}{}", self.settings.name_prefix, eagle_device_set.name),
                    &eagle_device.name,
                ),
                tc.convert_element_description(&eagle_device_set.description),
            ),
            component_uuid,
            package_uuid,
        )?;
        let meta = device.element_metadata_mut();
        meta.set_generated_by(generated_by(
            dev_lib_name,
            &[&eagle_device_set.name, &eagle_device.name],
        ));
        meta.set_categories(self.settings.device_categories.clone());
        if let Some(pads) = self.package_pad_map.get(&package_key) {
            for (pad_name, pad_uuid) in pads {
                let mut signal_uuid = None;
                for connection in &eagle_device.connections {
                    if connection.pads.contains(pad_name) {
                        signal_uuid = self
                            .component_signal_map
                            .get(&(
                                component_key.clone(),
                                connection.gate.clone(),
                                connection.pin.clone(),
                            ))
                            .copied();
                    }
                }
                device
                    .pad_signal_map_mut()
                    .push(DevicePadSignalMapItem::new(*pad_uuid, signal_uuid, false));
            }
        }
        for technology in &eagle_device.technologies {
            let mut attributes = AttributeList::new();
            tc.try_convert_attributes(&technology.attributes, &mut attributes, log);
            let mut mpn = SimpleString::clean("");
            let mut manufacturer = SimpleString::clean("");
            tc.try_extract_mpn_and_manufacturer(&mut attributes, &mut mpn, &mut manufacturer);
            if mpn.is_empty() {
                mpn = SimpleString::clean(&technology.name); // Good idea or not?
            }
            if !mpn.is_empty() {
                device
                    .parts_mut()
                    .push(Part::new(mpn, manufacturer, attributes));
            }
        }
        Ok(device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use librepcb_core::types::Point;

    #[test]
    fn test_generated_by() {
        assert_eq!(
            generated_by("foo.LBR", &["A", "B"]),
            "EagleImport::foo::A::B"
        );
        assert_eq!(generated_by("", &["A"]), "EagleImport::::A");
    }

    #[test]
    fn test_stable_sort() {
        let mut v = vec![3, 1, 2, 1];
        stable_sort_by_less(&mut v, |a, b| a < b);
        assert_eq!(v, vec![1, 1, 2, 3]);
    }

    #[test]
    fn test_grab_area_union() {
        let rect = |x1, y1, x2, y2| {
            Path::rect(
                Point::new(Length::new(x1), Length::new(y1)),
                Point::new(Length::new(x2), Length::new(y2)),
            )
        };
        let mut union = GrabAreaUnion::default();
        assert!(!union.contains(&rect(0, 0, 10, 10)));
        union.add(&rect(0, 0, 10_000_000, 10_000_000));
        assert!(union.contains(&rect(1_000_000, 1_000_000, 2_000_000, 2_000_000)));
        assert!(!union.contains(&rect(9_000_000, 1_000_000, 12_000_000, 2_000_000)));
    }
}
