//! Port of libs/librepcb/editor/project/schematic/schematicclipboarddata.{h,cpp},
//! schematicclipboarddatabuilder.{h,cpp} and
//! libs/librepcb/editor/project/cmd/cmdpasteschematicitems.{h,cpp}.
//!
//! The clipboard content is upstream's format: a ZIP file containing
//! `schematic.lp` (the S-expression `librepcb_clipboard_schematic`), the
//! library elements of the copied components and symbols
//! (`cmp/<uuid>/`, `sym/<uuid>/`) and the image files, under the MIME type
//! `application/x-librepcb-clipboard.schematic; version=<app version>`.
//!
//! Differences to upstream: when pasting, net lines at bus junctions of
//! a bus segment which was split while copying are attached to the right
//! part (upstream keeps only the junctions of the last part).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::attribute::AttributeList;
use librepcb_core::fileio::{FileSystem, TransactionalDirectory};
use librepcb_core::geometry::{
    Image, ImageList, Junction, JunctionList, NetLabel, NetLabelList, NetLine, NetLineAnchor,
    NetLineList, Polygon, PolygonList, Text, TextList,
};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cmp::Component;
use librepcb_core::library::sym::Symbol;
use librepcb_core::project::circuit::Bus;
use librepcb_core::project::circuit::{
    AssemblyVariantList, ComponentAssemblyOptionList, ComponentInstance,
};
use librepcb_core::project::schematic::{
    Schematic, SchematicBusSegment, SchematicNetSegment, SchematicNetSegmentSplitter,
    SchematicSymbol,
};
use librepcb_core::project::{
    AssemblyVariantId, BusId, BusSegmentId, ComponentInstanceId, Mutation, NetSegmentId,
    NetSegmentRef, Project, ProjectAttributeLookup, SchematicId, SchematicMutation, SymbolId,
};
use librepcb_core::serialization::{
    self, DeserializeObject, List, Mode, SExpression, SerializeObject,
};
use librepcb_core::types::{Angle, BusName, CircuitIdentifier, Point, UnsignedLength, Uuid};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::SchematicItem;
use super::selection::SelectionQuery;
use crate::commands::circuit::{add_net, default_net_class, set_component_signal_net};
use crate::commands::schematic::{apply_net_name, forced_net_name};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};
use crate::library_editor::commands::SymbolClipboardData;
use crate::undo_stack::LibraryElement;

/// The MIME type prefix of schematic clipboard data (followed by
/// `; version=<application version>`).
pub const SCHEMATIC_CLIPBOARD_MIME_PREFIX: &str = "application/x-librepcb-clipboard.schematic";

/// Returns the MIME type of schematic clipboard data (upstream
/// `SchematicClipboardData::getMimeType()`).
pub fn schematic_clipboard_mime_type(app_version: &str) -> String {
    format!("{SCHEMATIC_CLIPBOARD_MIME_PREFIX}; version={app_version}")
}

/// A copied component instance (upstream
/// `SchematicClipboardData::ComponentInstance`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardComponent {
    /// UUID of the original component instance.
    pub uuid: Uuid,
    /// The library component.
    pub lib_component: Uuid,
    /// The symbol variant.
    pub lib_variant: Uuid,
    /// The name.
    pub name: CircuitIdentifier,
    /// The value.
    pub value: String,
    /// The attributes.
    pub attributes: AttributeList,
    /// The assembly options.
    pub assembly_options: ComponentAssemblyOptionList,
    /// Whether the assembly options are locked.
    pub lock_assembly: bool,
}

impl SerializeObject for ClipboardComponent {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("lib_component", &self.lib_component);
        root.ensure_line_break();
        root.append_child("lib_variant", &self.lib_variant);
        root.ensure_line_break();
        root.append_child("name", &self.name);
        root.append_child("value", &self.value);
        root.ensure_line_break();
        self.attributes.serialize(root);
        root.ensure_line_break();
        self.assembly_options.serialize(root);
        root.ensure_line_break();
        root.append_child("lock_assembly", &self.lock_assembly);
        root.ensure_line_break();
    }
}

impl DeserializeObject for ClipboardComponent {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            lib_component: node.child_value("lib_component/@0")?,
            lib_variant: node.child_value("lib_variant/@0")?,
            name: node.child_value("name/@0")?,
            value: node.child_value("value/@0")?,
            attributes: AttributeList::deserialize(node)?,
            assembly_options: ComponentAssemblyOptionList::deserialize(node)?,
            lock_assembly: node.child_value("lock_assembly/@0")?,
        })
    }
}

/// A copied symbol (upstream `SchematicClipboardData::SymbolInstance`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardSymbol {
    /// UUID of the original symbol.
    pub uuid: Uuid,
    /// UUID of the original component instance.
    pub component: Uuid,
    /// The gate (symbol variant item).
    pub lib_gate: Uuid,
    /// The position.
    pub position: Point,
    /// The rotation.
    pub rotation: Angle,
    /// Whether the symbol is mirrored.
    pub mirrored: bool,
    /// The texts.
    pub texts: TextList,
}

impl SerializeObject for ClipboardSymbol {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("component", &self.component);
        root.ensure_line_break();
        root.append_child("lib_gate", &self.lib_gate);
        root.ensure_line_break();
        self.position.serialize(root.append_list("position"));
        root.append_child("rotation", &self.rotation);
        root.append_child("mirror", &self.mirrored);
        root.ensure_line_break();
        self.texts.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for ClipboardSymbol {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            component: node.child_value("component/@0")?,
            lib_gate: node.child_value("lib_gate/@0")?,
            position: Point::deserialize(node.required_child("position")?)?,
            rotation: node.child_value("rotation/@0")?,
            mirrored: node.child_value("mirror/@0")?,
            texts: TextList::deserialize(node)?,
        })
    }
}

/// A copied (split) net segment (upstream
/// `SchematicClipboardData::NetSegment`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardNetSegment {
    /// Name of the net.
    pub net_name: CircuitIdentifier,
    /// The junctions.
    pub junctions: JunctionList,
    /// The lines (anchors are junctions of this segment or pins of the
    /// copied symbols).
    pub lines: NetLineList,
    /// The labels.
    pub labels: NetLabelList,
}

impl SerializeObject for ClipboardNetSegment {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        root.append_child("net", &self.net_name);
        root.ensure_line_break();
        self.junctions.serialize(root);
        root.ensure_line_break();
        self.lines.serialize(root);
        root.ensure_line_break();
        self.labels.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for ClipboardNetSegment {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            net_name: node.child_value("net/@0")?,
            junctions: JunctionList::deserialize(node)?,
            lines: NetLineList::deserialize(node)?,
            labels: NetLabelList::deserialize(node)?,
        })
    }
}

/// A bus of copied bus segments (upstream `SchematicClipboardData::Bus`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardBus {
    /// UUID of the original bus.
    pub uuid: Uuid,
    /// The name.
    pub name: BusName,
    /// Whether the net names are prefixed with the bus name.
    pub prefix_net_names: bool,
    /// The maximum trace length difference.
    pub max_trace_length_difference: Option<UnsignedLength>,
}

impl SerializeObject for ClipboardBus {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("name", &self.name);
        root.append_child("prefix_nets", &self.prefix_net_names);
        let diff = match self.max_trace_length_difference {
            Some(v) => SExpression::token(v.to_string()),
            None => SExpression::token("none"),
        };
        root.append_child("max_trace_length_difference", &diff);
        root.ensure_line_break();
    }
}

impl DeserializeObject for ClipboardBus {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let diff = node.required_child("max_trace_length_difference/@0")?;
        let max_trace_length_difference = if diff.value()? == "none" {
            None
        } else {
            Some(serialization::FromSExpression::from_sexpression(diff)?)
        };
        Ok(Self {
            uuid: node.child_value("@0")?,
            name: node.child_value("name/@0")?,
            prefix_net_names: node.child_value("prefix_nets/@0")?,
            max_trace_length_difference,
        })
    }
}

/// A copied (split) bus segment (upstream
/// `SchematicClipboardData::BusSegment`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardBusSegment {
    /// UUID of the original bus segment (referenced by net lines).
    pub uuid: Uuid,
    /// UUID of the original bus.
    pub bus: Uuid,
    /// The junctions.
    pub junctions: JunctionList,
    /// The lines.
    pub lines: NetLineList,
    /// The labels.
    pub labels: NetLabelList,
}

impl SerializeObject for ClipboardBusSegment {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("bus", &self.bus);
        root.ensure_line_break();
        self.junctions.serialize(root);
        root.ensure_line_break();
        self.lines.serialize(root);
        root.ensure_line_break();
        self.labels.serialize(root);
        root.ensure_line_break();
    }
}

impl DeserializeObject for ClipboardBusSegment {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            uuid: node.child_value("@0")?,
            bus: node.child_value("bus/@0")?,
            junctions: JunctionList::deserialize(node)?,
            lines: NetLineList::deserialize(node)?,
            labels: NetLabelList::deserialize(node)?,
        })
    }
}

/// Schematic items on the clipboard (upstream `SchematicClipboardData`).
#[derive(Debug)]
pub struct SchematicClipboardData {
    /// UUID of the schematic the items were copied from.
    pub schematic: Uuid,
    /// The cursor position when copying.
    pub cursor_pos: Point,
    /// The assembly variants of the source project.
    pub assembly_variants: AssemblyVariantList,
    /// The buses of the copied bus segments.
    pub buses: Vec<ClipboardBus>,
    /// The components of the copied symbols.
    pub components: Vec<ClipboardComponent>,
    /// The symbols.
    pub symbols: Vec<ClipboardSymbol>,
    /// The bus segments.
    pub bus_segments: Vec<ClipboardBusSegment>,
    /// The net segments.
    pub net_segments: Vec<ClipboardNetSegment>,
    /// The polygons.
    pub polygons: PolygonList,
    /// The texts.
    pub texts: TextList,
    /// The images.
    pub images: ImageList,
    /// Library elements (`cmp/<uuid>/`, `sym/<uuid>/`) and image files.
    directory: TransactionalDirectory,
}

impl SchematicClipboardData {
    /// Creates empty clipboard data.
    pub fn new(
        schematic: Uuid,
        cursor_pos: Point,
        assembly_variants: AssemblyVariantList,
    ) -> Result<Self> {
        Ok(Self {
            schematic,
            cursor_pos,
            assembly_variants,
            buses: Vec::new(),
            components: Vec::new(),
            symbols: Vec::new(),
            bus_segments: Vec::new(),
            net_segments: Vec::new(),
            polygons: PolygonList::new(),
            texts: TextList::new(),
            images: ImageList::new(),
            directory: TransactionalDirectory::new_temporary()?,
        })
    }

    /// The directory with the library elements and image files.
    pub fn directory(&self) -> &TransactionalDirectory {
        &self.directory
    }

    /// A second handle to the directory (same file system).
    fn dir(&self) -> TransactionalDirectory {
        TransactionalDirectory::new(std::sync::Arc::clone(self.directory.file_system()), "")
    }

    /// Serializes the items to `schematic.lp` (upstream `toMimeData()`).
    pub fn to_sexpression(&self) -> SExpression {
        let mut root = List::new("librepcb_clipboard_schematic");
        root.ensure_line_break();
        self.cursor_pos
            .serialize(root.append_list("cursor_position"));
        root.ensure_line_break();
        root.append_child("schematic", &self.schematic);
        root.ensure_line_break();
        self.assembly_variants.serialize(&mut root);
        root.ensure_line_break();
        for b in &self.buses {
            root.ensure_line_break();
            b.serialize(root.append_list("bus"));
        }
        root.ensure_line_break();
        for c in &self.components {
            root.ensure_line_break();
            c.serialize(root.append_list("component"));
        }
        root.ensure_line_break();
        root.ensure_line_break();
        for s in &self.symbols {
            root.ensure_line_break();
            s.serialize(root.append_list("symbol"));
        }
        root.ensure_line_break();
        root.ensure_line_break();
        for s in &self.bus_segments {
            root.ensure_line_break();
            s.serialize(root.append_list("bussegment"));
        }
        root.ensure_line_break();
        for s in &self.net_segments {
            root.ensure_line_break();
            s.serialize(root.append_list("netsegment"));
        }
        root.ensure_line_break();
        root.ensure_line_break();
        self.polygons.serialize(&mut root);
        root.ensure_line_break();
        self.texts.serialize(&mut root);
        root.ensure_line_break();
        self.images.serialize(&mut root);
        root.ensure_line_break();
        SExpression::List(root)
    }

    /// Returns the clipboard content: a ZIP file with `schematic.lp`, the
    /// library elements and the image files.
    pub fn to_zip(&self) -> Result<Vec<u8>> {
        let content = self.to_sexpression().to_byte_array(Mode::LibrePcb)?;
        let fs = self.directory.file_system();
        fs.write("schematic.lp", &content)?;
        Ok(fs.export_to_zip(None)?)
    }

    /// Loads clipboard content (upstream constructor from MIME data).
    pub fn from_zip(zip: &[u8]) -> Result<Self> {
        let directory = TransactionalDirectory::new_temporary()?;
        directory.file_system().load_from_zip_bytes(zip.to_vec())?;
        let content = directory.read("schematic.lp")?;
        let root = SExpression::parse(&content, None, Mode::LibrePcb)?;
        let mut assembly_variants = AssemblyVariantList::new();
        assembly_variants.load_from_sexpression(&root)?;
        let parse_all =
            |name: &'static str| -> Vec<&SExpression> { root.children_named(name).collect() };
        Ok(Self {
            schematic: root.child_value("schematic/@0")?,
            cursor_pos: Point::deserialize(root.required_child("cursor_position")?)?,
            assembly_variants,
            buses: parse_all("bus")
                .into_iter()
                .map(ClipboardBus::deserialize)
                .collect::<serialization::Result<_>>()?,
            bus_segments: parse_all("bussegment")
                .into_iter()
                .map(ClipboardBusSegment::deserialize)
                .collect::<serialization::Result<_>>()?,
            components: parse_all("component")
                .into_iter()
                .map(ClipboardComponent::deserialize)
                .collect::<serialization::Result<_>>()?,
            symbols: parse_all("symbol")
                .into_iter()
                .map(ClipboardSymbol::deserialize)
                .collect::<serialization::Result<_>>()?,
            net_segments: parse_all("netsegment")
                .into_iter()
                .map(ClipboardNetSegment::deserialize)
                .collect::<serialization::Result<_>>()?,
            polygons: PolygonList::deserialize(&root)?,
            texts: TextList::deserialize(&root)?,
            images: ImageList::deserialize(&root)?,
            directory,
        })
    }

    /// Converts symbol clipboard data (copied in the symbol editor) to
    /// schematic clipboard data with its polygons, texts and images, to
    /// paste graphical elements from the symbol editor (upstream
    /// `pasteFromClipboard()`). Texts which evaluate to an empty string in
    /// the schematic (e.g. `{{NAME}}`) lose their braces to keep them
    /// visible.
    pub fn from_symbol_data(
        data: &SymbolClipboardData,
        project: &Project,
        schematic: &Schematic,
    ) -> Result<Self> {
        let mut out = Self::new(
            data.symbol_uuid,
            data.cursor_pos,
            AssemblyVariantList::new(),
        )?;
        for polygon in data.polygons.iter() {
            out.polygons.push(polygon.clone());
        }
        let lookup = ProjectAttributeLookup::for_schematic(project, schematic, None);
        for text in data.texts.iter() {
            let mut copy = text.clone();
            if lookup.substitute(text.text()).trim().is_empty() {
                copy.set_text(text.text().replace(['{', '}'], ""));
            }
            out.texts.push(copy);
        }
        let mut dir = out.dir();
        for image in data.images.iter() {
            if let Some(content) = data.files.get(image.file_name().as_str()) {
                dir.write(image.file_name().as_str(), content)?;
            }
            out.images.push(image.clone());
        }
        Ok(out)
    }

    /// Whether there is nothing to paste.
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
            && self.bus_segments.is_empty()
            && self.net_segments.is_empty()
            && self.polygons.is_empty()
            && self.texts.is_empty()
            && self.images.is_empty()
    }

    /// Builds the clipboard data of the selected items (upstream
    /// `SchematicClipboardDataBuilder::generate()`).
    pub(crate) fn build(
        p: &Project,
        s: &Schematic,
        query: &SelectionQuery,
        cursor_pos: Point,
    ) -> Result<Self> {
        let mut data = Self::new(
            s.uuid(),
            cursor_pos,
            p.circuit().assembly_variants().clone(),
        )?;
        let mut query = query.clone();
        query.add_junctions_of_bus_lines(s, false);
        query.add_net_points_of_net_lines(s, false);
        let ctx = p.view();
        let mut dir = data.dir();

        // Components and symbols.
        let mut components = BTreeSet::new();
        for id in &query.symbols {
            let symbol = &s.symbols()[id];
            let resolved = symbol.resolve(ctx)?;
            let cmp = resolved.component;
            if components.insert(cmp.id()) {
                let mut cmp_dir = dir.subdir(&format!("cmp/{}", cmp.lib_component()));
                if cmp_dir.files("").is_empty() {
                    resolved.lib_component.directory().copy_to(&mut cmp_dir)?;
                }
                data.components.push(ClipboardComponent {
                    uuid: cmp.id().0,
                    lib_component: cmp.lib_component(),
                    lib_variant: cmp.lib_variant(),
                    name: cmp.name().clone(),
                    value: cmp.value().clone(),
                    attributes: cmp.attributes().clone(),
                    assembly_options: cmp.assembly_options().clone(),
                    lock_assembly: cmp.lock_assembly(),
                });
            }
        }
        for id in &query.symbols {
            let symbol = &s.symbols()[id];
            let resolved = symbol.resolve(ctx)?;
            let mut sym_dir = dir.subdir(&format!("sym/{}", resolved.lib_symbol.metadata().uuid()));
            if sym_dir.files("").is_empty() {
                resolved.lib_symbol.directory().copy_to(&mut sym_dir)?;
            }
            data.symbols.push(ClipboardSymbol {
                uuid: symbol.uuid(),
                component: symbol.component().0,
                lib_gate: symbol.lib_gate(),
                position: symbol.position(),
                rotation: symbol.rotation(),
                mirrored: symbol.mirrored(),
                texts: symbol.texts().values().cloned().collect::<Vec<_>>().into(),
            });
        }

        // Bus segments (split into cohesive parts).
        // Selected junctions, lines and labels per bus segment.
        type BusItems = (BTreeSet<Uuid>, BTreeSet<Uuid>, BTreeSet<Uuid>);
        let mut bus_items: BTreeMap<BusSegmentId, BusItems> = BTreeMap::new();
        for (seg, j) in &query.bus_junctions {
            bus_items.entry(*seg).or_default().0.insert(*j);
        }
        for (seg, l) in &query.bus_lines {
            bus_items.entry(*seg).or_default().1.insert(*l);
        }
        for (seg, l) in &query.bus_labels {
            bus_items.entry(*seg).or_default().2.insert(*l);
        }
        for (seg_id, (junctions, lines, labels)) in &bus_items {
            let Some(seg) = s.bus_segments().get(seg_id) else {
                continue;
            };
            let bus = p
                .circuit()
                .bus(seg.bus())
                .ok_or_else(|| Error::not_found("Bus", seg.bus()))?;
            if !data.buses.iter().any(|b| b.uuid == bus.uuid()) {
                data.buses.push(ClipboardBus {
                    uuid: bus.uuid(),
                    name: bus.name().clone(),
                    prefix_net_names: bus.prefix_net_names(),
                    max_trace_length_difference: bus.max_trace_length_difference(),
                });
            }
            let mut splitter = SchematicNetSegmentSplitter::new();
            for j in junctions {
                if let Some(junction) = seg.junctions().get(j) {
                    splitter.add_junction(junction.clone());
                }
            }
            for l in lines {
                if let Some(line) = seg.lines().get(l) {
                    splitter.add_net_line(line.clone());
                }
            }
            for l in labels {
                if let Some(label) = seg.labels().get(l) {
                    splitter.add_net_label(label.clone());
                }
            }
            for part in splitter.split() {
                data.bus_segments.push(ClipboardBusSegment {
                    uuid: seg.uuid(),
                    bus: bus.uuid(),
                    junctions: part.junctions.into(),
                    lines: part.lines.into(),
                    labels: part.labels.into(),
                });
            }
        }

        // Net segments (split into cohesive parts).
        for (seg_id, items) in query.net_segment_items() {
            let Some(seg) = s.net_segments().get(&seg_id) else {
                continue;
            };
            let mut splitter = SchematicNetSegmentSplitter::new();
            for (symbol, pin) in seg.connected_pins() {
                let anchor = NetLineAnchor::Pin {
                    symbol: symbol.0,
                    pin,
                };
                if let Some(pos) = s.net_line_anchor_position(seg_id, anchor, ctx) {
                    splitter.add_fixed_anchor(anchor, pos, !query.symbols.contains(&symbol));
                }
            }
            for (bus_segment, junction) in seg.connected_bus_junctions() {
                let anchor = NetLineAnchor::BusJunction {
                    segment: bus_segment.0,
                    junction,
                };
                if let Some(pos) = s.net_line_anchor_position(seg_id, anchor, ctx) {
                    let copied = query.bus_junctions.contains(&(bus_segment, junction));
                    splitter.add_fixed_anchor(anchor, pos, !copied);
                }
            }
            for j in &items.net_points {
                if let Some(junction) = seg.junctions().get(j) {
                    splitter.add_junction(junction.clone());
                }
            }
            for l in &items.net_lines {
                if let Some(line) = seg.lines().get(l) {
                    splitter.add_net_line(line.clone());
                }
            }
            for l in &items.net_labels {
                if let Some(label) = seg.labels().get(l) {
                    splitter.add_net_label(label.clone());
                }
            }
            let net_name = p
                .circuit()
                .net_signal(seg.net())
                .map(|n| n.name().clone())
                .ok_or_else(|| Error::not_found("Net", seg.net()))?;
            for part in splitter.split() {
                data.net_segments.push(ClipboardNetSegment {
                    net_name: net_name.clone(),
                    junctions: part.junctions.into(),
                    lines: part.lines.into(),
                    labels: part.labels.into(),
                });
            }
        }

        // Polygons, texts, images.
        for id in &query.polygons {
            data.polygons.push(s.polygons()[id].clone());
        }
        for id in &query.texts {
            data.texts.push(s.texts()[id].clone());
        }
        let sch_dir = format!("schematics/{}", s.directory_name());
        for id in &query.images {
            let image = &s.images()[id];
            let file = format!("{sch_dir}/{}", image.file_name());
            if let Ok(Some(content)) = p.directory().read_if_exists(&file) {
                dir.write(image.file_name().as_str(), &content)?;
            }
            data.images.push(image.clone());
        }
        Ok(data)
    }
}

/// Pastes clipboard data into a schematic (upstream
/// `CmdPasteSchematicItems`): library elements missing in the project
/// are added, components get new UUIDs (and new names if theirs exist),
/// net segments get new nets unless a net label or a forced net name
/// determines the net. Returns the pasted items.
#[derive(Debug)]
pub struct PasteSchematicItems {
    /// The schematic.
    pub schematic: SchematicId,
    /// The data.
    pub data: SchematicClipboardData,
    /// Offset added to all positions.
    pub offset: Point,
}

impl Command for PasteSchematicItems {
    type Output = Vec<SchematicItem>;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdPasteSchematicItems",
            "Paste Schematic Elements"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Vec<SchematicItem>> {
        paste(tx, self.schematic, self.data, self.offset)
    }
}

fn paste(
    tx: &mut Transaction<'_>,
    schematic: SchematicId,
    data: SchematicClipboardData,
    offset: Point,
) -> Result<Vec<SchematicItem>> {
    let sch = Mutation::Schematic;
    let mut pasted = Vec::new();
    let mut dir = data.dir();

    // Copy new components and symbols to the project library.
    for dirname in dir.dirs("cmp") {
        let Ok(uuid) = dirname.as_str().parse::<Uuid>() else {
            continue;
        };
        if tx.project().library().component(&uuid).is_none() {
            let cmp = Component::open(dir.subdir(&format!("cmp/{dirname}")))?;
            tx.add_library_element(LibraryElement::Component(cmp))?;
        }
    }
    for dirname in dir.dirs("sym") {
        let Ok(uuid) = dirname.as_str().parse::<Uuid>() else {
            continue;
        };
        if tx.project().library().symbol(&uuid).is_none() {
            let sym = Symbol::open(dir.subdir(&format!("sym/{dirname}")))?;
            tx.add_library_element(LibraryElement::Symbol(sym))?;
        }
    }

    // Assembly variants conversion (blacklisting, like upstream).
    let convert =
        |p: &Project, uuids: &BTreeSet<AssemblyVariantId>| -> BTreeSet<AssemblyVariantId> {
            if uuids.is_empty() {
                return uuids.clone(); // Keep do-not-mount status!
            }
            let variants = p.circuit().assembly_variants();
            let mut result: BTreeSet<AssemblyVariantId> = variants
                .uuids()
                .into_iter()
                .map(AssemblyVariantId)
                .collect();
            for old in data.assembly_variants.iter() {
                let id = AssemblyVariantId(old.uuid());
                if !uuids.contains(&id)
                    && !result.remove(&id)
                    && let Some(new) = variants.iter().find(|v| v.name() == old.name())
                {
                    result.remove(&AssemblyVariantId(new.uuid()));
                }
            }
            result
        };

    // Components, sorted by name.
    let mut components = data.components.clone();
    components.sort_by(|a, b| toolbox::compare_numeric(a.name.as_str(), b.name.as_str()));
    let mut component_map: BTreeMap<Uuid, ComponentInstanceId> = BTreeMap::new();
    for cmp in &components {
        let p = tx.project();
        let lib_cmp = p
            .library()
            .component(&cmp.lib_component)
            .ok_or_else(|| Error::not_found("Component", cmp.lib_component))?;
        let mut name = cmp.name.clone();
        if p.circuit()
            .component_instance_by_name(name.as_str())
            .is_some()
        {
            let prefix = lib_cmp.prefixes().value(&p.settings().norm_order);
            name = CircuitIdentifier::new(
                p.circuit()
                    .generate_auto_component_instance_name(prefix.as_str()),
            )?;
        }
        let mut copy = ComponentInstance::new(
            Uuid::new_random(),
            lib_cmp,
            cmp.lib_variant,
            name,
            cmp.lock_assembly,
        )?;
        copy.set_value(cmp.value.clone());
        copy.set_attributes(cmp.attributes.clone());
        let mut options = cmp.assembly_options.clone();
        for option in options.iter_mut() {
            let converted = convert(p, option.assembly_variants());
            option.set_assembly_variants(converted);
        }
        copy.set_assembly_options(options);
        copy.set_lock_assembly(cmp.lock_assembly);
        component_map.insert(cmp.uuid, copy.id());
        tx.apply(Mutation::AddComponentInstance(copy))?;
    }

    // Symbols.
    let mut symbol_map: BTreeMap<Uuid, SymbolId> = BTreeMap::new();
    for sym in &data.symbols {
        let component = component_map
            .get(&sym.component)
            .copied()
            .ok_or_else(|| Error::not_found("Component", sym.component))?;
        let mut symbol = SchematicSymbol::new(
            Uuid::new_random(),
            component,
            sym.lib_gate,
            sym.position + offset,
            sym.rotation,
            sym.mirrored,
        );
        for text in sym.texts.iter() {
            // Keep the UUID (reference to the library symbol text).
            let mut copy = text.clone();
            copy.set_position(copy.position() + offset);
            symbol.insert_text(copy);
        }
        symbol_map.insert(sym.uuid, symbol.id());
        pasted.push(SchematicItem::Symbol(symbol.id()));
        tx.apply(sch(SchematicMutation::AddSymbol { schematic, symbol }))?;
    }

    // Bus segments: parts with labels keep their bus (by name), others get
    // a new bus with an automatic name.
    let mut bus_junction_map: BTreeMap<(Uuid, Uuid), (Uuid, Uuid)> = BTreeMap::new();
    for seg in &data.bus_segments {
        let bus_obj = data
            .buses
            .iter()
            .find(|b| b.uuid == seg.bus)
            .ok_or_else(|| Error::not_found("Bus", seg.bus))?;
        let existing = (!seg.labels.is_empty())
            .then(|| tx.project().circuit().bus_by_name(bus_obj.name.as_str()))
            .flatten()
            .map(|(id, _)| id);
        let bus = match existing {
            Some(bus) => bus,
            None if !seg.labels.is_empty() => {
                let bus = Bus::new(
                    Uuid::new_random(),
                    bus_obj.name.clone(),
                    false,
                    bus_obj.prefix_net_names,
                    bus_obj.max_trace_length_difference,
                );
                let id = BusId(bus.uuid());
                tx.apply(Mutation::AddBus(bus))?;
                id
            }
            None => crate::commands::bus::add_bus(tx, None)?,
        };
        let mut segment = SchematicBusSegment::new(Uuid::new_random(), bus);
        let mut junction_map: BTreeMap<Uuid, Uuid> = BTreeMap::new();
        for junction in seg.junctions.iter() {
            let copy = Junction::new(Uuid::new_random(), junction.position() + offset);
            junction_map.insert(junction.uuid(), copy.uuid());
            bus_junction_map.insert((seg.uuid, junction.uuid()), (segment.uuid(), copy.uuid()));
            segment.insert_junction(copy);
        }
        for line in seg.lines.iter() {
            let map = |a: NetLineAnchor| match a {
                NetLineAnchor::Junction(j) => {
                    junction_map.get(&j).map(|n| NetLineAnchor::Junction(*n))
                }
                _ => None,
            };
            let (Some(p1), Some(p2)) = (map(line.p1()), map(line.p2())) else {
                return Err(Error::InvalidArgument(
                    "Invalid bus line in the clipboard data.".to_owned(),
                ));
            };
            segment.insert_line(NetLine::new(Uuid::new_random(), line.width(), p1, p2));
        }
        for label in seg.labels.iter() {
            segment.insert_label(NetLabel::new(
                Uuid::new_random(),
                label.position() + offset,
                label.rotation(),
                label.mirrored(),
            ));
        }
        let id = segment.id();
        for j in segment.junctions().keys() {
            pasted.push(SchematicItem::BusJunction(id, *j));
        }
        for l in segment.lines().keys() {
            pasted.push(SchematicItem::BusLine(id, *l));
        }
        for l in segment.labels().keys() {
            pasted.push(SchematicItem::BusLabel(id, *l));
        }
        tx.apply(sch(SchematicMutation::AddBusSegment { schematic, segment }))?;
    }

    // Net segments.
    for seg in &data.net_segments {
        let class = default_net_class(tx)?;
        let net = add_net(tx, None, class)?;
        let mut forced: Option<CircuitIdentifier> = None;
        let mut segment = SchematicNetSegment::new(Uuid::new_random(), net);
        let mut junction_map: BTreeMap<Uuid, Uuid> = BTreeMap::new();
        for junction in seg.junctions.iter() {
            let copy = Junction::new(Uuid::new_random(), junction.position() + offset);
            junction_map.insert(junction.uuid(), copy.uuid());
            segment.insert_junction(copy);
        }
        let mut lines = Vec::new();
        for line in seg.lines.iter() {
            let mut anchors = [line.p1(), line.p2()];
            let mut valid = true;
            for anchor in &mut anchors {
                match *anchor {
                    NetLineAnchor::Junction(j) => match junction_map.get(&j) {
                        Some(new) => *anchor = NetLineAnchor::Junction(*new),
                        None => valid = false,
                    },
                    NetLineAnchor::Pin { symbol, pin } => {
                        let Some(new_symbol) = symbol_map.get(&symbol).copied() else {
                            valid = false;
                            continue;
                        };
                        let p = tx.project();
                        let symbol = p
                            .schematic(schematic)
                            .and_then(|s| s.symbols().get(&new_symbol))
                            .ok_or_else(|| Error::not_found("Symbol", new_symbol))?;
                        let view = symbol
                            .pin(p.view(), pin)?
                            .ok_or_else(|| Error::not_found("Pin", pin))?;
                        let signal = view.signal();
                        if view.net() != Some(net) {
                            set_component_signal_net(tx, signal, Some(net))?;
                        }
                        if forced.is_none() {
                            forced = forced_net_name(tx.project(), signal);
                        }
                        *anchor = NetLineAnchor::Pin {
                            symbol: new_symbol.0,
                            pin,
                        };
                    }
                    NetLineAnchor::BusJunction { segment, junction } => {
                        match bus_junction_map.get(&(segment, junction)) {
                            Some((new_segment, new_junction)) => {
                                *anchor = NetLineAnchor::BusJunction {
                                    segment: *new_segment,
                                    junction: *new_junction,
                                }
                            }
                            None => valid = false,
                        }
                    }
                }
            }
            if valid {
                lines.push(NetLine::new(
                    Uuid::new_random(),
                    line.width(),
                    anchors[0],
                    anchors[1],
                ));
            }
        }
        if lines.is_empty() {
            continue;
        }
        for line in lines {
            segment.insert_line(line);
        }
        for label in seg.labels.iter() {
            segment.insert_label(NetLabel::new(
                Uuid::new_random(),
                label.position() + offset,
                label.rotation(),
                label.mirrored(),
            ));
            if forced.is_none() {
                forced = Some(seg.net_name.clone());
            }
        }
        // Junctions without lines are dropped (the model rejects them).
        let used: BTreeSet<Uuid> = segment
            .lines()
            .values()
            .flat_map(|l| [l.p1(), l.p2()])
            .filter_map(|a| match a {
                NetLineAnchor::Junction(j) => Some(j),
                _ => None,
            })
            .collect();
        let mut cleaned = SchematicNetSegment::new(segment.uuid(), net);
        for j in segment
            .junctions()
            .values()
            .filter(|j| used.contains(&j.uuid()))
        {
            cleaned.insert_junction(j.clone());
        }
        for l in segment.lines().values() {
            cleaned.insert_line(l.clone());
        }
        for l in segment.labels().values() {
            cleaned.insert_label(l.clone());
        }
        let id: NetSegmentId = cleaned.id();
        for j in cleaned.junctions().keys() {
            pasted.push(SchematicItem::NetPoint(id, *j));
        }
        for l in cleaned.lines().keys() {
            pasted.push(SchematicItem::NetLine(id, *l));
        }
        for l in cleaned.labels().keys() {
            pasted.push(SchematicItem::NetLabel(id, *l));
        }
        tx.apply(sch(SchematicMutation::AddNetSegment {
            schematic,
            segment: cleaned,
        }))?;
        if let Some(name) = forced {
            apply_net_name(
                tx,
                NetSegmentRef {
                    schematic,
                    segment: id,
                },
                &name,
            )?;
        }
    }

    // Polygons and texts.
    for polygon in data.polygons.iter() {
        let copy = Polygon::new(
            Uuid::new_random(),
            polygon.layer(),
            polygon.line_width(),
            polygon.is_filled(),
            polygon.is_grab_area(),
            polygon.path().translated(offset),
        );
        pasted.push(SchematicItem::Polygon(copy.uuid()));
        tx.apply(sch(SchematicMutation::AddPolygon {
            schematic,
            polygon: copy,
        }))?;
    }
    // Images: an existing file with the same content is reused, otherwise
    // a new file is added (upstream `CmdPasteSchematicItems`).
    for image in data.images.iter() {
        let Some(content) = dir.read_if_exists(image.file_name().as_str())? else {
            continue; // Skip images with missing file.
        };
        if content.is_empty() {
            continue;
        }
        let existing =
            crate::commands::image::find_existing_image_file(tx.project(), schematic, &content)?;
        let file_name = match &existing {
            Some(name) => name.clone(),
            None => crate::commands::image::unused_image_file_name(
                tx.project(),
                schematic,
                image.file_basename(),
                image.file_extension(),
            )?,
        };
        let copy = Image::new(
            Uuid::new_random(),
            file_name,
            image.position() + offset,
            image.rotation(),
            image.width(),
            image.height(),
            image.border_width(),
        );
        pasted.push(SchematicItem::Image(copy.uuid()));
        tx.run(crate::commands::AddSchematicImage {
            schematic,
            image: copy,
            data: existing.is_none().then_some(content),
        })?;
    }
    for text in data.texts.iter() {
        let copy = Text::new(
            Uuid::new_random(),
            text.layer(),
            text.text().clone(),
            text.position() + offset,
            text.rotation(),
            text.height(),
            text.align(),
            text.locked(),
        );
        pasted.push(SchematicItem::Text(copy.uuid()));
        tx.apply(sch(SchematicMutation::AddText {
            schematic,
            text: copy,
        }))?;
    }
    Ok(pasted)
}
