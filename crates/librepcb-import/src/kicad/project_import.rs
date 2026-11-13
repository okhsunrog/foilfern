//! Native import of KiCad project data from its embedded schematic library.
//! Format reference: <https://dev-docs.kicad.org/en/file-formats/sexpr-schematic/>.

use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine, engine::general_purpose::STANDARD};
use librepcb_core::fileio::{FilePath, FileSystem, TransactionalDirectory, file_utils};
use librepcb_core::geometry::{
    Image, Junction, NetLabel, NetLine, NetLineAnchor, Path, Polygon, Text,
};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::project::circuit::{ComponentInstance, NetClass, NetSignal};
use librepcb_core::project::schematic::import_appearance::{
    GlobalLabel, ImportAppearance, PinTextHeights,
};
use librepcb_core::project::schematic::{Schematic, SchematicNetSegment, SchematicSymbol};
use librepcb_core::project::{
    ComponentInstanceId, ComponentSignalRef, Mutation, NetClassId, NetSignalId, Project,
    SchematicId, SchematicMutation,
};
use librepcb_core::serialization::{FromSExpression, Mode, SExpression};
use librepcb_core::types::{
    Alignment, Angle, CircuitIdentifier, ElementName, FileProofName, Layer, Length, Point,
    PositiveLength, UnsignedLength, Uuid,
};
use librepcb_core::utils::message_logger::MessageLogger;
use librepcb_core::utils::transform::Transform;

use super::merge_symbol_gates;
use super::project_board::{BoardSource, PadNets};
use super::types::{KiCadProperty, KiCadSymbol, KiCadSymbolGate, text_alignment};
use super::{Error, KiCadLibraryConverter, KiCadTypeConverter, NoImportedElements, Result};

pub(super) fn invalid(message: impl Into<String>) -> Error {
    Error::ProjectImport(message.into())
}

pub(super) fn read_root(path: &FilePath, kind: &str) -> Result<SExpression> {
    let bytes = file_utils::read_file(path)?;
    let root = SExpression::parse(&bytes, Some(path.as_path()), Mode::Permissive)?;
    if root.name()? != kind {
        return Err(invalid(format!(
            "Expected a KiCad {kind} file at {}.",
            path.to_native()
        )));
    }
    Ok(root)
}

pub(super) fn point(node: &SExpression, path: &str) -> Result<Point> {
    let node = node.required_child(path)?;
    Ok(Point::from_mm(
        node.child_value("@0")?,
        -node.child_value::<f64>("@1")?,
    )?)
}

pub(super) fn property<'a>(node: &'a SExpression, key: &str) -> Option<&'a str> {
    node.children_named("property").find_map(|property| {
        (property.child("@0")?.value().ok()? == key)
            .then(|| property.child("@1")?.value().ok())
            .flatten()
    })
}

pub(super) fn angle(node: &SExpression, path: &str) -> Result<Angle> {
    Ok(Angle::from_deg(
        node.child(path)
            .map(|n| n.value())
            .transpose()?
            .unwrap_or("0")
            .parse::<f64>()
            .map_err(|_| invalid("Invalid KiCad angle."))?,
    )?)
}

/// Parsed KiCad schematic ready for native project creation.
pub struct KiCadProjectImport {
    path: FilePath,
    schematic: SExpression,
    board: Option<BoardSource>,
}

impl KiCadProjectImport {
    pub fn open(schematic: FilePath) -> Result<Self> {
        let root = read_root(&schematic, "kicad_sch")?;
        for item in ["sheet", "bus", "bus_entry", "hierarchical_label"] {
            if root.child(item).is_some() {
                return Err(invalid(format!(
                    "KiCad schematic '{item}' is unsupported in this import."
                )));
            }
        }
        Ok(Self {
            path: schematic,
            schematic: root,
            board: None,
        })
    }

    /// Also import the board setup for a fresh routing run.
    pub fn with_board(mut self, path: FilePath) -> Result<Self> {
        self.board = Some(BoardSource::open(path)?);
        Ok(self)
    }

    pub fn has_board(&self) -> bool {
        self.board.is_some()
    }

    /// Imports the schematic, including its embedded symbols, as one sheet.
    pub fn import(&self, project: &mut Project, log: &MessageLogger<'_>) -> Result<()> {
        let mut converter = KiCadLibraryConverter::new(&NoImportedElements, Default::default());
        let name = ElementName::new(ElementName::clean(self.path.basename()))?;
        let sheet =
            project.add_schematic(Schematic::new(Uuid::new_random(), name, "main"), None)?;
        let mut library = BTreeMap::new();
        for node in self
            .schematic
            .required_child("lib_symbols")?
            .children_named("symbol")
        {
            let original = node.required_child("@0")?.value()?.to_owned();
            let mut symbol = KiCadSymbol::parse(node, log)?;
            if node.child("power").is_some()
                || property(node, "Reference").is_some_and(|name| name.starts_with('#'))
            {
                symbol.on_board = false;
            }
            symbol.name = original.rsplit(':').next().unwrap_or(&original).to_owned();
            let gates = merge_symbol_gates(&symbol.gates, &symbol.name);
            let keys: Vec<_> = gates
                .iter()
                .map(|gate| format!("KiCadProject::{original}::{}", gate.index))
                .collect();
            let mut lib_symbols = Vec::new();
            for (gate, key) in gates.iter().zip(&keys) {
                let lib = converter.create_symbol(&self.path, &symbol, gate, key, log)?;
                lib_symbols.push(lib.metadata().uuid());
                project.add_library_symbol(lib)?;
            }
            let key = format!("KiCadProject::{original}");
            let component =
                converter.create_component(&self.path, &symbol, &gates, &key, &keys, log)?;
            let imported = ImportedSymbol {
                uuid: component.metadata().uuid(),
                power: node.child("power").is_some() && original != "power:PWR_FLAG",
                gates: gates.into_iter().zip(lib_symbols).collect(),
                definition: symbol,
                key,
            };
            project.add_library_component(component)?;
            library.insert(original, imported);
        }
        let mut graph = WireGraph::default();
        let mut instances = BTreeMap::new();
        for node in self.schematic.children_named("symbol") {
            let id = node.required_child("lib_id/@0")?.value()?;
            let lib = library
                .get(id)
                .ok_or_else(|| invalid(format!("Missing embedded KiCad symbol {id}.")))?;
            import_instance(project, sheet, node, lib, &mut instances, &mut graph, log)?;
        }
        graph.read_wires(&self.schematic)?;
        let (pad_nets, net_classes) = match &self.board {
            Some(board) => (board.pad_nets()?, board.net_classes(project, log)?),
            None => (BTreeMap::new(), BTreeMap::new()),
        };
        graph.import(project, sheet, &pad_nets, &net_classes)?;
        let schematic = project
            .schematic(sheet)
            .ok_or(Error::Logic("imported sheet missing"))?;
        let appearance = serde_json::to_vec(&graph.appearance)
            .map_err(|err| invalid(format!("Cannot store imported sheet appearance: {err}")))?;
        project.replace_asset(&ImportAppearance::asset_path(schematic), Some(&appearance))?;
        for node in self.schematic.children_named("rectangle") {
            let width = node
                .child("stroke/width/@0")
                .map_or(Ok(0.0), f64::from_sexpression)?;
            let polygon = Polygon::new(
                Uuid::new_random(),
                Layer::SCHEMATIC_DOCUMENTATION,
                UnsignedLength::new(Length::from_mm(if width == 0.0 { 0.1524 } else { width })?)?,
                node.child("fill/type/@0")
                    .is_some_and(|value| value.value() == Ok("color")),
                false,
                Path::rect(point(node, "start")?, point(node, "end")?),
            );
            project.apply(Mutation::Schematic(SchematicMutation::AddPolygon {
                schematic: sheet,
                polygon,
            }))?;
        }
        for (index, node) in self.schematic.children_named("image").enumerate() {
            let image = import_image(project, node, index)?;
            project.apply(Mutation::Schematic(SchematicMutation::AddImage {
                schematic: sheet,
                image,
            }))?;
        }
        for node in self.schematic.children_named("text") {
            let value = node.required_child("@0")?.value()?;
            let size = node
                .child("effects/font/size/@1")
                .map_or(Ok(1.27), f64::from_sexpression)?;
            let text = Text::new(
                Uuid::new_random(),
                Layer::SCHEMATIC_DOCUMENTATION,
                value,
                point(node, "at")?,
                angle(node, "at/@2")?,
                project_text_height(size)?,
                text_alignment(node),
                false,
            );
            project.apply(Mutation::Schematic(SchematicMutation::AddText {
                schematic: sheet,
                text,
            }))?;
        }
        if let Some(board) = &self.board {
            board.import(project, &mut converter, &library, &instances, log)?;
        }
        Ok(())
    }
}

fn import_image(project: &Project, node: &SExpression, index: usize) -> Result<Image> {
    let encoded: String = node
        .required_child("data")?
        .children()
        .iter()
        .filter(|child| child.is_string())
        .map(SExpression::value)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .concat();
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|error| invalid(format!("Invalid KiCad embedded image: {error}")))?;
    let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(b"\xff\xd8") {
        "jpg"
    } else {
        return Err(invalid("Unsupported KiCad embedded image format."));
    };
    let (px_width, px_height) = Image::try_load(&bytes, extension).map_err(invalid)?;
    // KiCad schematic bitmaps are centered at the insertion point at 300 ppi.
    let scale = node
        .child("scale/@0")
        .map_or(Ok(1.0), f64::from_sexpression)?;
    let mm_per_pixel = scale * 25.4 / 300.0;
    let width = PositiveLength::new(Length::from_mm(px_width * mm_per_pixel)?)?;
    let height = PositiveLength::new(Length::from_mm(px_height * mm_per_pixel)?)?;
    let rotation = angle(node, "at/@2")?;
    let offset = Point::new(*width / 2, *height / 2).rotated(rotation, Point::ORIGIN);
    let pos = point(node, "at")? - offset;
    let uuid = Uuid::new_random();
    let file = FileProofName::new(format!("image_{}.{extension}", index + 1))?;
    let root = project.directory();
    let mut directory = TransactionalDirectory::new(root.file_system().clone(), root.path());
    directory.write(&format!("schematics/main/{}", file.as_str()), &bytes)?;
    Ok(Image::new(uuid, file, pos, rotation, width, height, None))
}

pub(super) struct ImportedSymbol {
    uuid: Uuid,
    power: bool,
    pub gates: Vec<(KiCadSymbolGate, Uuid)>,
    pub definition: KiCadSymbol,
    pub key: String,
}

pub(super) struct Instance {
    pub id: ComponentInstanceId,
    pub library_id: String,
    pub populate: bool,
}

fn import_instance(
    project: &mut Project,
    sheet: SchematicId,
    node: &SExpression,
    lib: &ImportedSymbol,
    instances: &mut BTreeMap<String, Instance>,
    graph: &mut WireGraph,
    log: &MessageLogger<'_>,
) -> Result<()> {
    let reference =
        property(node, "Reference").ok_or_else(|| invalid("Symbol has no reference."))?;
    let value = property(node, "Value").unwrap_or("");
    let unit = node.child("unit/@0").map_or(Ok(1), i32::from_sexpression)?;
    let style = node
        .child("body_style/@0")
        .or_else(|| node.child("convert/@0"));
    if style.map_or(Ok(1), i32::from_sexpression)? > 1 {
        return Err(invalid(format!(
            "KiCad alternate body style is unsupported on {reference}."
        )));
    }
    let index = lib
        .gates
        .iter()
        .position(|(gate, _)| gate.index == unit)
        .or_else(|| (lib.gates.len() == 1).then_some(0))
        .ok_or_else(|| invalid(format!("Missing unit {unit} of {reference}.")))?;
    let (gate, library_symbol) = &lib.gates[index];
    let component = project
        .library()
        .component(&lib.uuid)
        .ok_or(Error::Logic("imported component missing"))?;
    let variant = component
        .symbol_variants()
        .first()
        .ok_or(Error::Logic("imported component has no symbol variant"))?;
    let item = variant
        .symbol_items()
        .get(index)
        .ok_or(Error::Logic("imported symbol unit missing"))?;
    let item_uuid = item.uuid();
    let component_id = if let Some(instance) = instances.get(reference) {
        instance.id
    } else {
        let name = CircuitIdentifier::new(CircuitIdentifier::clean(reference))?;
        let mut instance =
            ComponentInstance::new(Uuid::new_random(), component, variant.uuid(), name, false)?;
        instance.set_value(value.to_owned());
        let id = instance.id();
        project.apply(Mutation::AddComponentInstance(instance))?;
        instances.insert(
            reference.to_owned(),
            Instance {
                id,
                library_id: node.required_child("lib_id/@0")?.value()?.to_owned(),
                populate: !node
                    .child("dnp/@0")
                    .is_some_and(|value| value.value() == Ok("yes")),
            },
        );
        id
    };
    let pos = point(node, "at")?;
    let mut rot = angle(node, "at/@2")?;
    let mirrored = match node.child("mirror/@0").map(|v| v.value()).transpose()? {
        None => false,
        Some("y") => {
            rot = -rot;
            true
        }
        Some("x") => {
            rot = Angle::DEG180 - rot;
            true
        }
        Some(value) => {
            return Err(invalid(format!(
                "Unsupported mirror axis '{value}' on {reference}."
            )));
        }
    };
    let uuid = Uuid::new_random();
    if node.child("dnp/@0").and_then(|value| value.value().ok()) == Some("yes") {
        graph.appearance.unpopulated.insert(uuid);
    }
    let mut symbol = SchematicSymbol::new(uuid, component_id, item_uuid, pos, rot, mirrored);
    let transform = Transform::from_obj(&symbol);
    for field in node.children_named("property") {
        let field = KiCadProperty::parse(field, log)?;
        let (layer, text) = match field.key.as_str() {
            "Reference" if !field.hide => (Layer::SYMBOL_NAMES, "{{NAME}}"),
            "Value" if !field.hide => (Layer::SYMBOL_VALUES, "{{VALUE}}"),
            _ => continue,
        };
        let (rotation, alignment) = field_orientation(
            &transform,
            Angle::from_deg(field.rotation)?,
            field.alignment,
        );
        symbol.insert_text(Text::new(
            Uuid::new_random(),
            layer,
            text,
            Point::from_mm(field.position.x, -field.position.y)?,
            rotation,
            project_text_height(field.font_size.height)?,
            alignment,
            false,
        ));
    }
    let component = project
        .library()
        .component(&lib.uuid)
        .ok_or(Error::Logic("imported component missing"))?;
    let item = component
        .symbol_variants()
        .first()
        .and_then(|v| v.symbol_items().by_uuid(&item_uuid))
        .ok_or(Error::Logic("imported gate missing"))?;
    let lib_symbol = project
        .library()
        .symbol(library_symbol)
        .ok_or(Error::Logic("imported symbol missing"))?;
    let mut power_pin = None;
    for (source_pin, (pin_name, numbers)) in gate
        .pins
        .iter()
        .zip(KiCadTypeConverter::convert_symbol_pin_names(&gate.pins))
    {
        if pin_name.is_empty() {
            continue;
        }
        let pin = lib_symbol
            .pins()
            .iter()
            .find(|pin| pin.name().as_str() == pin_name)
            .ok_or_else(|| Error::PinNotFound(pin_name.clone()))?;
        let signal = item
            .pin_signal_map()
            .by_uuid(&pin.uuid())
            .and_then(|m| m.signal_uuid())
            .ok_or_else(|| Error::PinNotFound(pin_name.clone()))?;
        let position = transform.map(&pin.position());
        let visible_pin = gate
            .pins
            .iter()
            .find(|p| !p.hidden && p.position == source_pin.position && p.name == source_pin.name);
        if let Some(visible_pin) = visible_pin {
            let heights = PinTextHeights {
                name: pin_text_height(visible_pin.name_height)?,
                number: pin_text_height(visible_pin.number_height)?,
            };
            if heights != PinTextHeights::default() {
                graph
                    .appearance
                    .pin_text_heights
                    .entry(uuid)
                    .or_default()
                    .insert(pin.uuid(), heights);
            }
        }
        let display_number = if lib.definition.hide_pin_numbers {
            String::new()
        } else {
            visible_pin.map(|p| p.number.clone()).unwrap_or_default()
        };
        graph
            .appearance
            .pin_numbers
            .entry(uuid)
            .or_default()
            .insert(pin.uuid(), display_number);
        let display_name = if lib.definition.hide_pin_names {
            String::new()
        } else {
            visible_pin
                .map(|p| KiCadTypeConverter::convert_circuit_identifier(&p.name).to_string())
                .filter(|_| !source_pin.name.is_empty())
                .unwrap_or_default()
        };
        graph
            .appearance
            .pin_names
            .entry(uuid)
            .or_default()
            .insert(pin.uuid(), display_name);
        graph.pins.entry(position).or_default().push(BoundPin {
            anchor: NetLineAnchor::Pin {
                symbol: uuid,
                pin: pin.uuid(),
            },
            signal: ComponentSignalRef {
                component: component_id,
                signal,
            },
            reference: reference.to_owned(),
            numbers,
        });
        power_pin = Some(position);
    }
    if lib.power
        && !reference.starts_with("#FLG")
        && !value.is_empty()
        && let Some(position) = power_pin
    {
        graph
            .names
            .entry(position)
            .or_default()
            .insert(value.to_owned());
    }
    project.add_symbol(sheet, symbol)?;
    Ok(())
}

fn project_text_height(mm: f64) -> Result<PositiveLength> {
    Ok(PositiveLength::new(project_line_height(mm)?)?)
}

fn pin_text_height(mm: Option<f64>) -> Result<Option<UnsignedLength>> {
    mm.map(|mm| Ok(UnsignedLength::new(project_line_height(mm)?)?))
        .transpose()
}

fn project_line_height(mm: f64) -> Result<Length> {
    // KiCad specifies the glyph height. Native schematic text uses line height.
    Ok(Length::from_mm(mm * 1.5)?)
}

fn field_orientation(
    transform: &Transform,
    rotation: Angle,
    alignment: Alignment,
) -> (Angle, Alignment) {
    // KiCad stores placed field coordinates in sheet space, but its text
    // axes and justification are relative to the symbol (SCH_FIELD).
    let direction = transform.map_non_mirrorable(rotation).mapped_to_0_360deg();
    let upright = match direction {
        Angle::DEG180 => Angle::DEG0,
        Angle::DEG270 => Angle::DEG90,
        _ => direction,
    };
    let alignment = if transform.mirrored {
        alignment.mirrored_v()
    } else {
        alignment
    };
    let alignment = if direction == upright {
        alignment
    } else {
        alignment.mirrored()
    };
    (upright, alignment)
}

#[derive(Clone)]
struct BoundPin {
    anchor: NetLineAnchor,
    signal: ComponentSignalRef,
    reference: String,
    numbers: Vec<String>,
}

#[derive(Default)]
struct WireGraph {
    edges: BTreeMap<Point, BTreeSet<Point>>,
    pins: BTreeMap<Point, Vec<BoundPin>>,
    names: BTreeMap<Point, BTreeSet<String>>,
    labels: BTreeMap<Point, Vec<NetLabel>>,
    no_connects: BTreeSet<Point>,
    appearance: ImportAppearance,
}

impl WireGraph {
    fn read_wires(&mut self, sheet: &SExpression) -> Result<()> {
        let mut points: BTreeSet<_> = self.pins.keys().chain(self.names.keys()).copied().collect();
        for tag in ["label", "global_label"] {
            for label in sheet.children_named(tag) {
                let pos = point(label, "at")?;
                self.names
                    .entry(pos)
                    .or_default()
                    .insert(label.required_child("@0")?.value()?.to_owned());
                let rotation = angle(label, "at/@2")?;
                let style = if tag == "global_label" {
                    let shape = label.child("shape/@0").and_then(|n| n.value().ok());
                    let size = label
                        .child("effects/font/size/@1")
                        .map_or(Ok(1.27), f64::from_sexpression)?;
                    Some(GlobalLabel {
                        input: matches!(shape, Some("input" | "bidirectional" | "tri_state")),
                        output: matches!(shape, Some("output" | "bidirectional" | "tri_state")),
                        height: *project_text_height(size)?,
                    })
                } else {
                    None
                };
                let native = NetLabel::new(Uuid::new_random(), pos, rotation, false);
                if let Some(style) = style {
                    self.appearance.global_labels.insert(native.uuid(), style);
                }
                self.labels.entry(pos).or_default().push(native);
                points.insert(pos);
            }
        }
        for junction in sheet.children_named("junction") {
            points.insert(point(junction, "at")?);
        }
        for marker in sheet.children_named("no_connect") {
            let pos = point(marker, "at")?;
            self.no_connects.insert(pos);
            for pin in self.pins.get(&pos).into_iter().flatten() {
                if let NetLineAnchor::Pin { symbol, pin } = pin.anchor {
                    self.appearance.set_no_connect(symbol, pin, true);
                }
            }
        }
        let mut wires = Vec::new();
        for wire in sheet.children_named("wire") {
            let start = point(wire, "pts/xy")?;
            let mut positions = wire.required_child("pts")?.children_named("xy");
            positions.next();
            let end = positions
                .next()
                .ok_or_else(|| invalid("Wire has no end point."))?;
            let end = Point::from_mm(end.child_value("@0")?, -end.child_value::<f64>("@1")?)?;
            if start != end {
                wires.push((start, end));
                points.extend([start, end]);
            }
        }
        for (start, end) in wires {
            let mut previous = None;
            for &pos in points.iter().filter(|pos| on_segment(**pos, start, end)) {
                if let Some(previous) = previous {
                    self.edges.entry(pos).or_default().insert(previous);
                    self.edges.entry(previous).or_default().insert(pos);
                }
                previous = Some(pos);
            }
        }
        Ok(())
    }

    fn import(
        &self,
        project: &mut Project,
        sheet: SchematicId,
        pad_nets: &PadNets,
        net_classes: &BTreeMap<String, NetClassId>,
    ) -> Result<()> {
        let existing_default = project
            .circuit()
            .net_class_by_name("default")
            .or_else(|| project.circuit().net_class_by_name("Default"))
            .map(|(id, _)| id);
        let default = match existing_default {
            Some(id) => id,
            None => project.add_net_class(NetClass::new(
                Uuid::new_random(),
                ElementName::new("Default")?,
            ))?,
        };
        let mut visited = BTreeSet::new();
        let mut names = BTreeMap::new();
        let mut unnamed_board_nets = BTreeSet::new();
        for start in self
            .edges
            .keys()
            .chain(self.pins.keys())
            .chain(self.names.keys())
        {
            if visited.contains(start) {
                continue;
            }
            let mut positions = BTreeSet::new();
            let mut pending = vec![*start];
            while let Some(pos) = pending.pop() {
                if visited.insert(pos) {
                    positions.insert(pos);
                    pending.extend(self.edges.get(&pos).into_iter().flatten().copied());
                }
            }
            let pins: Vec<_> = positions
                .iter()
                .flat_map(|pos| self.pins.get(pos).into_iter().flatten())
                .collect();
            let labels: BTreeSet<_> = positions
                .iter()
                .flat_map(|pos| self.names.get(pos).into_iter().flatten())
                .collect();
            let has_wire = self.edges.contains_key(start);
            if !has_wire
                && pins.len() <= 1
                && (labels.is_empty() || self.no_connects.contains(start))
            {
                continue;
            }
            if labels.len() > 1 {
                return Err(invalid(format!(
                    "KiCad wire group contains conflicting names {labels:?} at {start:?}."
                )));
            }
            let board_names: BTreeSet<_> = pins
                .iter()
                .flat_map(|pin| {
                    pin.numbers
                        .iter()
                        .filter_map(|number| pad_nets.get(&(pin.reference.clone(), number.clone())))
                })
                .collect();
            if board_names.len() > 1 {
                return Err(invalid(format!(
                    "KiCad schematic and board disagree on nets {board_names:?} at {start:?}."
                )));
            }
            let name = if let Some(name) = board_names.first() {
                if labels.is_empty() && names.contains_key(*name)
                    || unnamed_board_nets.contains(*name)
                {
                    return Err(invalid(format!(
                        "KiCad board net '{name}' joins disconnected unnamed schematic wires."
                    )));
                }
                if labels.is_empty() {
                    unnamed_board_nets.insert((*name).clone());
                } else if !labels.contains(name) {
                    return Err(invalid(format!(
                        "KiCad board net '{name}' differs from schematic label {labels:?}."
                    )));
                }
                (*name).clone()
            } else if let Some(name) = labels.first() {
                (*name).clone()
            } else if let Some(pin) = pins.first() {
                format!("Net-({}-{})", pin.reference, pin.numbers.join(","))
            } else {
                format!("N${}", names.len() + 1)
            };
            let net = if let Some(net) = names.get(&name) {
                *net
            } else {
                let native_name = KiCadTypeConverter::convert_circuit_identifier(&name);
                if project
                    .circuit()
                    .net_signal_by_name(native_name.as_str())
                    .is_some()
                {
                    return Err(invalid(format!(
                        "KiCad net '{name}' collides with '{native_name}'."
                    )));
                }
                let class = net_classes.get(&name).copied().unwrap_or(default);
                let net = project.add_net_signal(NetSignal::new(
                    Uuid::new_random(),
                    class,
                    native_name,
                    labels.is_empty(),
                ))?;
                names.insert(name, net);
                net
            };
            self.add_segment(project, sheet, net, &positions)?;
        }
        Ok(())
    }

    fn add_segment(
        &self,
        project: &mut Project,
        sheet: SchematicId,
        net: NetSignalId,
        positions: &BTreeSet<Point>,
    ) -> Result<()> {
        let mut segment = SchematicNetSegment::new(Uuid::new_random(), net);
        let mut anchors = BTreeMap::new();
        for pos in positions {
            let pins = self.pins.get(pos).map_or(&[][..], Vec::as_slice);
            for pin in pins {
                let previous = project
                    .circuit()
                    .component_instance(pin.signal.component)
                    .and_then(|cmp| cmp.signal(&pin.signal.signal))
                    .and_then(|s| s.net());
                if previous.is_some_and(|id| id != net) {
                    return Err(invalid(format!(
                        "{} pin(s) {:?} would connect two different nets.",
                        pin.reference, pin.numbers
                    )));
                }
                if previous.is_none() {
                    project.apply(Mutation::SetComponentSignalNet {
                        signal: pin.signal,
                        net: Some(net),
                    })?;
                }
            }
            if let Some((first, remaining)) = pins.split_first() {
                anchors.insert(*pos, first.anchor);
                for pin in remaining {
                    segment.insert_line(wire(first.anchor, pin.anchor));
                }
                if remaining.is_empty()
                    && !self.edges.contains_key(pos)
                    && self.labels.contains_key(pos)
                {
                    // KiCad can place a label directly on a pin. In the native
                    // model the label belongs to a wired net segment.
                    let junction = Junction::new(Uuid::new_random(), *pos);
                    segment
                        .insert_line(wire(first.anchor, NetLineAnchor::Junction(junction.uuid())));
                    segment.insert_junction(junction);
                }
            } else if self.edges.contains_key(pos) {
                let junction = Junction::new(Uuid::new_random(), *pos);
                anchors.insert(*pos, NetLineAnchor::Junction(junction.uuid()));
                segment.insert_junction(junction);
            }
            for label in self.labels.get(pos).into_iter().flatten() {
                segment.insert_label(label.clone());
            }
        }
        for pos in positions {
            for target in self
                .edges
                .get(pos)
                .into_iter()
                .flatten()
                .filter(|target| *target > pos)
            {
                segment.insert_line(wire(anchors[pos], anchors[target]));
            }
        }
        if !segment.lines().is_empty() || !segment.labels().is_empty() {
            project.apply(Mutation::Schematic(SchematicMutation::AddNetSegment {
                schematic: sheet,
                segment,
            }))?;
        }
        Ok(())
    }
}

fn wire(from: NetLineAnchor, to: NetLineAnchor) -> NetLine {
    NetLine::new(
        Uuid::new_random(),
        SchematicNetSegment::default_line_width(),
        from,
        to,
    )
}

fn on_segment(p: Point, a: Point, b: Point) -> bool {
    let x = |p: Point| i128::from(p.x.to_nm());
    let y = |p: Point| i128::from(p.y.to_nm());
    (x(a).min(x(b))..=x(a).max(x(b))).contains(&x(p))
        && (y(a).min(y(b))..=y(a).max(y(b))).contains(&y(p))
        && (x(p) - x(a)) * (y(b) - y(a)) == (y(p) - y(a)) * (x(b) - x(a))
}
