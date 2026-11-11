//! Board setup for rerouting a KiCad project from its original placement.
//! Format reference: <https://dev-docs.kicad.org/en/file-formats/sexpr-pcb/>.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::attribute::AttributeList;
use librepcb_core::fileio::{FilePath, file_utils};
use librepcb_core::geometry::{Path, Vertex, ZoneRules};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::dev::PartList;
use librepcb_core::project::board::{
    Board, BoardDevice, BoardItem, BoardPlane, BoardPolygonData, BoardSettings, BoardZoneData,
    PlaneConnectStyle,
};
use librepcb_core::project::circuit::{ComponentAssemblyOption, NetClass};
use librepcb_core::project::{
    AssemblyVariantId, BoardId, Mutation, NetClassId, NetSignalId, Project,
};
use librepcb_core::serialization::{FromSExpression, SExpression};
use librepcb_core::types::{
    Angle, ElementName, Layer, Length, Point, PositiveLength, UnsignedLength, Uuid,
};
use librepcb_core::utils::message_logger::MessageLogger;
use serde_json::Value;

use super::project_import::{ImportedSymbol, Instance, angle, invalid, point, property, read_root};
use super::types::{KiCadFootprint, KiCadLayer, KiCadPadShape};
use super::{Error, KiCadLibraryConverter, KiCadTypeConverter, Result};

pub(super) type PadNets = BTreeMap<(String, String), String>;

pub(super) struct BoardSource {
    path: FilePath,
    root: SExpression,
    codes: BTreeMap<String, String>,
    rules: Value,
}

impl BoardSource {
    pub fn open(path: FilePath) -> Result<Self> {
        let root = read_root(&path, "kicad_pcb")?;
        let rules_path = path.as_path().with_extension("kicad_pro");
        let rules = if rules_path.is_file() {
            let rules_path = FilePath::new(rules_path)
                .ok_or_else(|| invalid("Invalid KiCad project rules path."))?;
            serde_json::from_slice(&file_utils::read_file(&rules_path)?)
                .map_err(|error| invalid(format!("Invalid KiCad project settings: {error}")))?
        } else {
            Value::Null
        };
        let mut codes = BTreeMap::new();
        for net in root.children_named("net") {
            codes.insert(
                net.required_child("@0")?.value()?.to_owned(),
                net.required_child("@1")?.value()?.to_owned(),
            );
        }
        Ok(Self {
            path,
            root,
            codes,
            rules,
        })
    }

    fn net_name<'a>(&'a self, node: &'a SExpression) -> Result<Option<&'a str>> {
        let Some(net) = node.child("net/@0") else {
            return Ok(None);
        };
        let value = net.value()?;
        if !net.is_string() && value == "0" {
            return Ok(None);
        }
        let name = if net.is_string() {
            value
        } else {
            self.codes
                .get(value)
                .map(String::as_str)
                .ok_or_else(|| invalid(format!("Unknown KiCad net code {value}.")))?
        };
        Ok((!name.is_empty() && !name.starts_with("unconnected-(")).then_some(name))
    }

    pub fn pad_nets(&self) -> Result<PadNets> {
        let mut nets = BTreeMap::new();
        for footprint in self.root.children_named("footprint") {
            let reference = property(footprint, "Reference")
                .ok_or_else(|| invalid("KiCad footprint has no reference."))?;
            for pad in footprint.children_named("pad") {
                let number = pad.required_child("@0")?.value()?;
                if let Some(net) = self.net_name(pad)?
                    && !number.is_empty()
                {
                    let key = (reference.to_owned(), number.to_owned());
                    if let Some(previous) = nets.insert(key, net.to_owned())
                        && previous != net
                    {
                        return Err(invalid(format!(
                            "KiCad pad {reference}.{number} has multiple nets."
                        )));
                    }
                }
            }
        }
        Ok(nets)
    }

    pub fn net_classes(
        &self,
        project: &mut Project,
        log: &MessageLogger<'_>,
    ) -> Result<BTreeMap<String, NetClassId>> {
        let settings = &self.rules["net_settings"];
        let mut classes = BTreeMap::new();
        if let Some(entries) = settings["classes"].as_array() {
            for entry in entries {
                let Some(name) = entry["name"].as_str() else {
                    continue;
                };
                if name == "Default" {
                    continue;
                }
                let mut class = NetClass::new(Uuid::new_random(), ElementName::new(name)?);
                class.set_default_trace_width(optional_length(entry, "track_width")?);
                class.set_default_via_drill(optional_length(entry, "via_drill")?);
                if let Some(clearance) = entry["clearance"].as_f64() {
                    class.set_min_copper_copper_clearance(unsigned(clearance)?);
                }
                classes.insert(name.to_owned(), project.add_net_class(class)?);
            }
        }
        let mut by_net = BTreeMap::new();
        if let Some(patterns) = settings["netclass_patterns"].as_array() {
            for entry in patterns {
                let (Some(pattern), Some(class)) =
                    (entry["pattern"].as_str(), entry["netclass"].as_str())
                else {
                    continue;
                };
                if pattern.contains(['*', '?']) {
                    log.warning(&format!("Skipped KiCad net class wildcard '{pattern}'."));
                    continue;
                }
                if let Some(id) = classes.get(class) {
                    by_net.entry(pattern.to_owned()).or_insert(*id);
                }
            }
        }
        Ok(by_net)
    }

    pub fn import(
        &self,
        project: &mut Project,
        converter: &mut KiCadLibraryConverter<'_>,
        library: &BTreeMap<String, ImportedSymbol>,
        instances: &BTreeMap<String, Instance>,
        log: &MessageLogger<'_>,
    ) -> Result<()> {
        let mut board = Board::new(Uuid::new_random(), ElementName::new("default")?, "default");
        let mut settings = board.settings().clone();
        settings.inner_layer_count = self
            .root
            .required_child("layers")?
            .children()
            .iter()
            .filter_map(|node| node.child("@0"))
            .filter_map(|node| node.value().ok())
            .filter(|name| name.starts_with("In") && name.ends_with(".Cu"))
            .count() as u32;
        if let Some(thickness) = self.root.child("general/thickness/@0") {
            settings.pcb_thickness = positive(f64::from_sexpression(thickness)?)?;
        }
        self.apply_rules(&mut settings, log)?;
        board.set_settings(settings);
        let board = project.add_board(board, None)?;
        self.add_outline(project, board)?;
        for node in self.root.children_named("footprint") {
            let reference = property(node, "Reference")
                .ok_or_else(|| invalid("KiCad footprint has no reference."))?;
            let part = instances.get(reference).ok_or_else(|| {
                invalid(format!(
                    "Footprint {reference} is missing from the schematic."
                ))
            })?;
            let symbol = library
                .get(&part.library_id)
                .ok_or(Error::Logic("component source symbol missing"))?;
            self.add_footprint(project, converter, board, node, part, symbol, log)?;
        }
        self.add_zones(project, board, log)?;
        Ok(())
    }

    fn apply_rules(&self, settings: &mut BoardSettings, log: &MessageLogger<'_>) -> Result<()> {
        if self.rules.is_null() {
            log.warning(
                "No sibling KiCad project settings file. Board design rules use LibrePCB defaults.",
            );
        }
        let defaults = self.rules["net_settings"]["classes"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["name"] == "Default"));
        if let Some(defaults) = defaults {
            if let Some(width) = optional_length(defaults, "track_width")? {
                settings.design_rules.set_default_trace_width(width);
            }
            if let Some(drill) = optional_length(defaults, "via_drill")? {
                settings.design_rules.set_default_via_drill_diameter(drill);
            }
        }
        let rules = &self.rules["board"]["design_settings"]["rules"];
        let drc = &mut settings.drc_settings;
        for (key, setter) in [
            ("min_clearance", librepcb_core::library::org::BoardDesignRuleCheckSettings::set_min_copper_copper_clearance as fn(&mut _, _) -> _),
            ("min_copper_edge_clearance", librepcb_core::library::org::BoardDesignRuleCheckSettings::set_min_copper_board_clearance),
            ("min_hole_clearance", librepcb_core::library::org::BoardDesignRuleCheckSettings::set_min_copper_npth_clearance),
            ("min_hole_to_hole", librepcb_core::library::org::BoardDesignRuleCheckSettings::set_min_drill_drill_clearance),
            ("min_track_width", librepcb_core::library::org::BoardDesignRuleCheckSettings::set_min_copper_width),
            ("min_through_hole_diameter", librepcb_core::library::org::BoardDesignRuleCheckSettings::set_min_pth_drill_diameter),
            ("min_via_annular_width", librepcb_core::library::org::BoardDesignRuleCheckSettings::set_min_pth_annular_ring),
            ("min_text_height", librepcb_core::library::org::BoardDesignRuleCheckSettings::set_min_silkscreen_text_height),
            ("min_text_thickness", librepcb_core::library::org::BoardDesignRuleCheckSettings::set_min_silkscreen_width),
        ] {
            if let Some(value) = rules[key].as_f64() {
                setter(drc, unsigned(value)?);
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn add_footprint(
        &self,
        project: &mut Project,
        converter: &mut KiCadLibraryConverter<'_>,
        board: BoardId,
        node: &SExpression,
        part: &Instance,
        symbol: &ImportedSymbol,
        log: &MessageLogger<'_>,
    ) -> Result<()> {
        let reference =
            property(node, "Reference").ok_or(Error::Logic("missing footprint reference"))?;
        let log = MessageLogger::child(log, reference);
        let mut footprint = KiCadFootprint::parse(node, &log)?;
        let rotation = angle(node, "at/@2")?;
        let mirrored = footprint.layer == KiCadLayer::BackCopper;
        normalize_footprint(&mut footprint, rotation, mirrored)?;
        let key = format!("KiCadProject::Footprint::{reference}");
        let package =
            converter.create_package(&self.path, &footprint, &key, &BTreeMap::new(), &log)?;
        let gates: Vec<_> = symbol.gates.iter().map(|(gate, _)| gate.clone()).collect();
        let device = converter.create_device(
            &self.path,
            &symbol.definition,
            &gates,
            &format!("{key}::Device"),
            &symbol.key,
            &key,
            &log,
        )?;
        let device_uuid = device.metadata().uuid();
        let fp_id = package
            .footprints()
            .first()
            .ok_or(Error::Logic("converted footprint missing"))?
            .uuid();
        let locked = node
            .child("locked/@0")
            .is_some_and(|node| node.value() == Ok("yes"));
        let placed = BoardDevice::from_library(
            part.id,
            &device,
            &package,
            fp_id,
            point(node, "at")?,
            rotation,
            mirrored,
            locked,
            false,
            true,
        )?;
        project.add_library_package(package)?;
        project.add_library_device(device)?;
        let component = project
            .circuit()
            .component_instance(part.id)
            .ok_or(Error::Logic("component missing"))?;
        let mut properties = component.properties();
        let variants = if part.populate {
            project
                .circuit()
                .assembly_variants()
                .iter()
                .map(|variant| AssemblyVariantId(variant.uuid()))
                .collect()
        } else {
            BTreeSet::new()
        };
        properties
            .assembly_options
            .push(ComponentAssemblyOption::new(
                device_uuid,
                AttributeList::new(),
                variants,
                PartList::new(),
            ));
        project.apply(Mutation::UpdateComponentInstance(properties))?;
        self.check_pad_nets(project, node, &placed, reference)?;
        project.add_board_device(board, placed)?;
        Ok(())
    }

    fn check_pad_nets(
        &self,
        project: &Project,
        source: &SExpression,
        placed: &BoardDevice,
        reference: &str,
    ) -> Result<()> {
        let mut expected = BTreeMap::new();
        for pad in source.children_named("pad") {
            let number = pad.required_child("@0")?.value()?;
            let Some(net) = self.net_name(pad)? else {
                continue;
            };
            if !number.is_empty() {
                expected.insert(number, find_net(project, net)?);
            }
        }
        let pads = placed.pads(project.library(), project.circuit())?;
        let actual: BTreeMap<_, _> = pads
            .iter()
            .filter_map(|pad| Some((pad.package_pad()?.name().as_str(), pad.net()?)))
            .collect();
        if let Some(number) = expected
            .keys()
            .chain(actual.keys())
            .find(|number| expected.get(*number) != actual.get(*number))
        {
            return Err(invalid(format!(
                "Imported net assignment differs on KiCad pad {reference}.{number}."
            )));
        }
        Ok(())
    }

    fn add_outline(&self, project: &mut Project, board: BoardId) -> Result<()> {
        let mut edges = Vec::new();
        for node in self.root.children().iter().filter(|n| {
            n.child("layer/@0")
                .is_some_and(|l| l.value() == Ok("Edge.Cuts"))
        }) {
            let a = point(node, "start")?;
            let b = point(node, "end")?;
            let angle = match node.name()? {
                "gr_line" => Angle::DEG0,
                "gr_arc" => KiCadTypeConverter::convert_arc(a, point(node, "mid")?, b),
                other => {
                    return Err(invalid(format!(
                        "Unsupported KiCad outline item '{other}'."
                    )));
                }
            };
            edges.push((a, b, angle));
        }
        let Some((first, mut end, angle)) = edges.pop() else {
            return Err(invalid("KiCad board has no Edge.Cuts outline."));
        };
        let mut vertices = vec![Vertex::new(first, angle)];
        while end != first {
            let index = edges
                .iter()
                .position(|(a, b, _)| *a == end || *b == end)
                .ok_or_else(|| invalid("KiCad Edge.Cuts does not form a closed outline."))?;
            let (a, b, angle) = edges.remove(index);
            let (next, angle) = if a == end { (b, angle) } else { (a, -angle) };
            vertices.push(Vertex::new(end, angle));
            end = next;
        }
        if !edges.is_empty() {
            return Err(invalid(
                "KiCad board with multiple Edge.Cuts contours is unsupported.",
            ));
        }
        vertices.push(Vertex::at(first));
        project.add_board_item(
            board,
            BoardItem::Polygon(BoardPolygonData::new(
                Uuid::new_random(),
                Layer::BOARD_OUTLINES,
                UnsignedLength::ZERO,
                Path::new(vertices),
                false,
                false,
                false,
            )),
        )?;
        Ok(())
    }

    fn add_zones(
        &self,
        project: &mut Project,
        board: BoardId,
        log: &MessageLogger<'_>,
    ) -> Result<()> {
        let mut mixed_relief = false;
        for node in self.root.children_named("zone") {
            let positions = node
                .required_child("polygon/pts")?
                .children_named("xy")
                .map(|p| {
                    Ok(Vertex::at(Point::from_mm(
                        p.child_value("@0")?,
                        -p.child_value::<f64>("@1")?,
                    )?))
                })
                .collect::<Result<_>>()?;
            let mut outline = Path::new(positions);
            outline.close();
            let mut layers = BTreeSet::new();
            if let Some(layer) = node.child("layer/@0") {
                layers.insert(copper_layer(layer.value()?)?);
            } else if let Some(list) = node.child("layers") {
                for layer in list.children().iter().filter(|child| child.is_string()) {
                    if layer.value()? == "*.Cu" {
                        let model = project.board(board).ok_or(Error::Logic("board missing"))?;
                        layers.extend(model.copper_layers());
                    } else {
                        layers.insert(copper_layer(layer.value()?)?);
                    }
                }
            }
            if let Some(keepout) = node.child("keepout") {
                let prohibited = |path| {
                    keepout
                        .child(path)
                        .is_some_and(|node| node.value() == Ok("not_allowed"))
                };
                let mut rules = ZoneRules::empty();
                rules.set(
                    ZoneRules::NO_COPPER,
                    ["tracks/@0", "vias/@0", "pads/@0"]
                        .iter()
                        .any(|path| prohibited(path)),
                );
                rules.set(ZoneRules::NO_PLANES, prohibited("copperpour/@0"));
                rules.set(ZoneRules::NO_DEVICES, prohibited("footprints/@0"));
                let zone = BoardZoneData::new(Uuid::new_random(), layers, rules, outline, false)?;
                project.add_board_item(board, BoardItem::Zone(zone))?;
                continue;
            }
            let net = self
                .net_name(node)?
                .map(|name| find_net(project, name))
                .transpose()?;
            for layer in layers {
                let mut plane = BoardPlane::new(Uuid::new_random(), layer, net, outline.clone());
                let style = match node.child("connect_pads/@0").and_then(|n| n.value().ok()) {
                    Some("yes") => PlaneConnectStyle::Solid,
                    Some("no") => PlaneConnectStyle::None,
                    Some("thru_hole_only") => {
                        mixed_relief = true;
                        PlaneConnectStyle::Solid
                    }
                    _ => PlaneConnectStyle::ThermalRelief,
                };
                plane.set_connect_style(style);
                if let Some(value) = node.child("connect_pads/clearance/@0") {
                    plane.set_min_clearance_to_copper(unsigned(f64::from_sexpression(value)?)?);
                }
                if let Some(value) = node.child("min_thickness/@0") {
                    plane.set_min_width(unsigned(f64::from_sexpression(value)?)?);
                }
                if let Some(value) = node.child("priority/@0") {
                    plane.set_priority(i32::from_sexpression(value)?);
                }
                if let Some(value) = node.child("fill/thermal_gap/@0") {
                    plane.set_thermal_gap(positive(f64::from_sexpression(value)?)?);
                }
                if let Some(value) = node.child("fill/thermal_bridge_width/@0") {
                    plane.set_thermal_spoke_width(positive(f64::from_sexpression(value)?)?);
                }
                let drc = &project
                    .board(board)
                    .ok_or(Error::Logic("board missing"))?
                    .settings()
                    .drc_settings;
                plane.set_min_clearance_to_board(drc.min_copper_board_clearance());
                plane.set_min_clearance_to_npth(drc.min_copper_npth_clearance());
                project.add_board_plane(board, plane)?;
            }
        }
        if mixed_relief {
            log.warning("KiCad THT-only thermal relief was mapped to solid plane connections.");
        }
        Ok(())
    }
}

fn optional_length(obj: &Value, key: &str) -> Result<Option<PositiveLength>> {
    obj[key].as_f64().map(positive).transpose()
}

fn positive(mm: f64) -> Result<PositiveLength> {
    Ok(PositiveLength::new(Length::from_mm(mm)?)?)
}

fn unsigned(mm: f64) -> Result<UnsignedLength> {
    Ok(UnsignedLength::new(Length::from_mm(mm)?)?)
}

fn find_net(project: &Project, name: &str) -> Result<NetSignalId> {
    let native = KiCadTypeConverter::convert_circuit_identifier(name);
    project
        .circuit()
        .net_signal_by_name(native.as_str())
        .map(|(id, _)| id)
        .ok_or_else(|| {
            invalid(format!(
                "Board net '{name}' is missing in the imported schematic."
            ))
        })
}

fn copper_layer(name: &str) -> Result<Layer> {
    match name {
        "F.Cu" => Some(Layer::TOP_COPPER),
        "B.Cu" => Some(Layer::BOT_COPPER),
        other => other
            .strip_prefix("In")
            .and_then(|n| n.strip_suffix(".Cu"))
            .and_then(|n| n.parse().ok())
            .and_then(Layer::inner_copper),
    }
    .ok_or_else(|| invalid(format!("Unsupported KiCad board copper layer '{name}'.")))
}

fn normalize_footprint(
    footprint: &mut KiCadFootprint,
    rotation: Angle,
    mirrored: bool,
) -> Result<()> {
    let rotation = rotation.to_deg();
    let local_angle = |absolute: f64| {
        if mirrored {
            rotation - absolute
        } else {
            absolute - rotation
        }
    };
    for pad in &mut footprint.pads {
        pad.rotation = local_angle(pad.rotation);
    }
    for text in &mut footprint.texts {
        text.rotation = local_angle(text.rotation);
    }
    for property in &mut footprint.properties {
        property.rotation = local_angle(property.rotation);
    }
    if !mirrored {
        return Ok(());
    }

    // The board file stores local geometry already flipped and pad angles
    // relative to the board. Undo the flip so BoardDevice mirrors once.
    if footprint.pads.iter().any(|pad| {
        matches!(pad.shape, KiCadPadShape::Custom | KiCadPadShape::Trapezoid)
            || !pad.chamfer_edges.is_empty()
    }) {
        return Err(invalid(format!(
            "Unsupported flipped custom pad in '{}'.",
            footprint.name
        )));
    }
    footprint.layer = KiCadLayer::FrontCopper;
    for pad in &mut footprint.pads {
        pad.position.x = -pad.position.x;
        pad.offset.x = -pad.offset.x;
        for layer in &mut pad.layers {
            *layer = flip(*layer);
        }
    }
    for line in &mut footprint.lines {
        line.start.x = -line.start.x;
        line.end.x = -line.end.x;
        line.layer = flip(line.layer);
    }
    for arc in &mut footprint.arcs {
        arc.start.x = -arc.start.x;
        arc.mid.x = -arc.mid.x;
        arc.end.x = -arc.end.x;
        arc.layer = flip(arc.layer);
    }
    for circle in &mut footprint.circles {
        circle.center.x = -circle.center.x;
        circle.end.x = -circle.end.x;
        circle.layer = flip(circle.layer);
    }
    for rect in &mut footprint.rectangles {
        rect.start.x = -rect.start.x;
        rect.end.x = -rect.end.x;
        rect.layer = flip(rect.layer);
    }
    for polygon in &mut footprint.polygons {
        for position in &mut polygon.coordinates {
            position.x = -position.x;
        }
        polygon.layer = flip(polygon.layer);
    }
    for zone in &mut footprint.zones {
        for position in &mut zone.polygon {
            position.x = -position.x;
        }
        for layer in &mut zone.layers {
            *layer = flip(*layer);
        }
    }
    for text in &mut footprint.texts {
        text.position.x = -text.position.x;
        text.layer = flip(text.layer);
        text.mirror = !text.mirror;
    }
    for property in &mut footprint.properties {
        property.position.x = -property.position.x;
        let layer = match property.layer.strip_prefix("B.") {
            Some(layer) => format!("F.{layer}"),
            None => property
                .layer
                .strip_prefix("F.")
                .map(|layer| format!("B.{layer}"))
                .unwrap_or_else(|| property.layer.clone()),
        };
        property.layer = layer;
        property.mirror = !property.mirror;
    }
    Ok(())
}

fn flip(layer: KiCadLayer) -> KiCadLayer {
    use KiCadLayer::*;
    match layer {
        FrontCopper => BackCopper,
        BackCopper => FrontCopper,
        FrontAdhesion => BackAdhesion,
        BackAdhesion => FrontAdhesion,
        FrontCourtyard => BackCourtyard,
        BackCourtyard => FrontCourtyard,
        FrontFabrication => BackFabrication,
        BackFabrication => FrontFabrication,
        FrontPaste => BackPaste,
        BackPaste => FrontPaste,
        FrontSilkscreen => BackSilkscreen,
        BackSilkscreen => FrontSilkscreen,
        FrontSolderMask => BackSolderMask,
        BackSolderMask => FrontSolderMask,
        other => other,
    }
}
