//! Copy & paste of board items: port of
//! libs/librepcb/editor/project/board/boardclipboarddata.{h,cpp},
//! boardclipboarddatabuilder.{h,cpp} and
//! libs/librepcb/editor/project/cmd/cmdpasteboarditems.{h,cpp}.
//!
//! The clipboard content is upstream's format: a ZIP archive with the
//! S-expression file `board.lp` (root `librepcb_clipboard_board`) and the
//! library devices and packages of the copied devices (`dev/<uuid>/`,
//! `pkg/<uuid>/`), under the MIME type [`board_clipboard_mime_type()`]
//! on the [`Clipboard`](crate::fsm::Clipboard).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::attribute::AttributeList;
use librepcb_core::fileio::{FileSystem, TransactionalDirectory};
use librepcb_core::geometry::{Junction, NonEmptyPath, Trace, TraceAnchor, Via};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::Package;
use librepcb_core::project::board::{
    BoardDevice, BoardHoleData, BoardItem, BoardNetSegment, BoardNetSegmentSplitter, BoardPadData,
    BoardPlane, BoardPolygonData, BoardStrokeTextData, BoardZoneData,
};
use librepcb_core::project::{BoardId, BoardMutation, ComponentInstanceId, Mutation, NetSignalId};
use librepcb_core::serialization::{
    DeserializeObject, List, Mode, SExpression, SerializeObject, ToSExpression,
};
use librepcb_core::types::{Angle, CircuitIdentifier, Point, Uuid};
use librepcb_i18n::tr;

use super::selection::SelectionQuery;
use super::view::BoardItemRef;
use crate::commands::circuit::{add_net, default_net_class};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};
use crate::library_editor::commands::FootprintClipboardData;
use crate::undo_stack::LibraryElement;

/// The MIME type prefix of board clipboard data (followed by
/// `; version=<application version>`).
pub const BOARD_CLIPBOARD_MIME_PREFIX: &str = "application/x-librepcb-clipboard.board";

/// Returns the MIME type of board clipboard data (upstream
/// `BoardClipboardData::getMimeType()`).
pub fn board_clipboard_mime_type(app_version: &str) -> String {
    format!("{BOARD_CLIPBOARD_MIME_PREFIX}; version={app_version}")
}

/// A device in the clipboard.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardDevice {
    /// The component instance.
    pub component: ComponentInstanceId,
    /// The library device.
    pub lib_device: Uuid,
    /// The library footprint.
    pub lib_footprint: Uuid,
    /// The position.
    pub position: Point,
    /// The rotation.
    pub rotation: Angle,
    /// Whether the device is mirrored.
    pub mirrored: bool,
    /// Whether the device is locked.
    pub locked: bool,
    /// Whether glue is enabled.
    pub glue: bool,
    /// The attributes.
    pub attributes: AttributeList,
    /// The stroke texts.
    pub stroke_texts: Vec<BoardStrokeTextData>,
}

impl ClipboardDevice {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.component);
        root.ensure_line_break();
        root.append_child("lib_device", &self.lib_device);
        root.ensure_line_break();
        root.append_child("lib_footprint", &self.lib_footprint);
        root.ensure_line_break();
        self.position.serialize(root.append_list("position"));
        root.append_child("rotation", &self.rotation);
        root.append_child("flip", &self.mirrored);
        root.append_child("lock", &self.locked);
        root.append_child("glue", &self.glue);
        root.ensure_line_break();
        self.attributes.serialize(root);
        for text in &self.stroke_texts {
            root.ensure_line_break();
            text.serialize(root.append_list("stroke_text"));
        }
        root.ensure_line_break();
    }

    fn deserialize(node: &SExpression) -> Result<Self> {
        Ok(Self {
            component: node.child_value("@0")?,
            lib_device: node.child_value("lib_device/@0")?,
            lib_footprint: node.child_value("lib_footprint/@0")?,
            position: Point::deserialize(node.required_child("position")?)?,
            rotation: node.child_value("rotation/@0")?,
            mirrored: node.child_value("flip/@0")?,
            locked: node.child_value("lock/@0")?,
            glue: node.child_value("glue/@0")?,
            attributes: AttributeList::deserialize(node)?,
            stroke_texts: node
                .children_named("stroke_text")
                .map(BoardStrokeTextData::deserialize)
                .collect::<std::result::Result<_, _>>()?,
        })
    }
}

/// A net segment in the clipboard (the net by name).
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardNetSegment {
    /// Name of the net.
    pub net_name: Option<CircuitIdentifier>,
    /// Standalone pads.
    pub pads: Vec<BoardPadData>,
    /// Vias.
    pub vias: Vec<Via>,
    /// Junctions.
    pub junctions: Vec<Junction>,
    /// Traces.
    pub traces: Vec<Trace>,
}

impl ClipboardNetSegment {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        root.append_child("net", &self.net_name);
        root.ensure_line_break();
        for pad in &self.pads {
            root.ensure_line_break();
            pad.serialize(root.append_list("pad"));
        }
        root.ensure_line_break();
        for via in &self.vias {
            root.ensure_line_break();
            via.serialize(root.append_list("via"));
        }
        root.ensure_line_break();
        for junction in &self.junctions {
            root.ensure_line_break();
            junction.serialize(root.append_list("junction"));
        }
        root.ensure_line_break();
        for trace in &self.traces {
            root.ensure_line_break();
            trace.serialize(root.append_list("trace"));
        }
        root.ensure_line_break();
    }

    fn deserialize(node: &SExpression) -> Result<Self> {
        Ok(Self {
            net_name: node.child_value("net/@0")?,
            pads: node
                .children_named("pad")
                .map(BoardPadData::deserialize)
                .collect::<std::result::Result<_, _>>()?,
            vias: node
                .children_named("via")
                .map(Via::deserialize)
                .collect::<std::result::Result<_, _>>()?,
            junctions: node
                .children_named("junction")
                .map(Junction::deserialize)
                .collect::<std::result::Result<_, _>>()?,
            traces: node
                .children_named("trace")
                .map(Trace::deserialize)
                .collect::<std::result::Result<_, _>>()?,
        })
    }
}

/// A plane in the clipboard: the plane with its net given by name.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardPlane {
    /// The plane (its net is ignored).
    pub plane: BoardPlane,
    /// Name of the net.
    pub net_name: Option<CircuitIdentifier>,
}

impl ClipboardPlane {
    fn serialize(&self, root: &mut List) {
        // Same layout as in `board.lp`, but with the net name.
        let mut plane = self.plane.clone();
        plane.set_net(None);
        plane.serialize(root);
        if let Some(net) = root
            .children_mut()
            .iter_mut()
            .find(|c| c.as_list().is_some_and(|l| l.name() == "net"))
            .and_then(SExpression::as_list_mut)
            && let Some(value) = net.children_mut().first_mut()
        {
            *value = self.net_name.to_sexpression();
        }
    }

    fn deserialize(node: &SExpression) -> Result<Self> {
        let net_name: Option<CircuitIdentifier> = node.child_value("net/@0")?;
        // Parse the plane without its net (the net is a name here).
        let mut copy = node.clone();
        if let Some(net) = copy.child_mut("net/@0") {
            *net = SExpression::token("none");
        }
        Ok(Self {
            plane: BoardPlane::deserialize(&copy)?,
            net_name,
        })
    }
}

/// Board items in the clipboard (port of upstream `BoardClipboardData`).
#[derive(Debug)]
pub struct BoardClipboardData {
    directory: TransactionalDirectory,
    /// The board the items were copied from.
    pub board_uuid: Uuid,
    /// The cursor position when copying.
    pub cursor_pos: Point,
    /// Devices.
    pub devices: Vec<ClipboardDevice>,
    /// Net segments.
    pub net_segments: Vec<ClipboardNetSegment>,
    /// Planes.
    pub planes: Vec<ClipboardPlane>,
    /// Zones.
    pub zones: Vec<BoardZoneData>,
    /// Polygons.
    pub polygons: Vec<BoardPolygonData>,
    /// Stroke texts.
    pub stroke_texts: Vec<BoardStrokeTextData>,
    /// Holes.
    pub holes: Vec<BoardHoleData>,
    /// Positions of the footprint pads of the copied devices, keyed by
    /// (component, pad).
    pub pad_positions: BTreeMap<(Uuid, Uuid), Point>,
}

impl BoardClipboardData {
    /// Converts footprint clipboard data (copied in the package editor) to
    /// board clipboard data with its polygons, stroke texts and holes, to
    /// paste graphical elements from the package editor (upstream
    /// `BoardEditorState_Select::processPaste()`).
    pub fn from_footprint_data(data: &FootprintClipboardData) -> Result<Self> {
        let mut out = Self::new(data.footprint_uuid, data.cursor_pos)?;
        for polygon in data.polygons.iter() {
            out.polygons.push(BoardPolygonData::new(
                polygon.uuid(),
                polygon.layer(),
                polygon.line_width(),
                polygon.path().clone(),
                polygon.is_filled(),
                polygon.is_grab_area(),
                false,
            ));
        }
        for text in data.stroke_texts.iter() {
            out.stroke_texts.push(BoardStrokeTextData::new(
                text.uuid(),
                text.layer(),
                text.text().clone(),
                text.position(),
                text.rotation(),
                text.height(),
                text.stroke_width(),
                text.letter_spacing(),
                text.line_spacing(),
                text.align(),
                text.mirrored(),
                text.auto_rotate(),
                false,
            ));
        }
        for hole in data.holes.iter() {
            out.holes.push(BoardHoleData::new(
                hole.uuid(),
                hole.diameter(),
                hole.path().clone(),
                hole.stop_mask_config(),
                false,
            ));
        }
        Ok(out)
    }

    /// Creates empty clipboard data.
    pub fn new(board_uuid: Uuid, cursor_pos: Point) -> Result<Self> {
        Ok(Self {
            directory: TransactionalDirectory::new_temporary()?,
            board_uuid,
            cursor_pos,
            devices: Vec::new(),
            net_segments: Vec::new(),
            planes: Vec::new(),
            zones: Vec::new(),
            polygons: Vec::new(),
            stroke_texts: Vec::new(),
            holes: Vec::new(),
            pad_positions: BTreeMap::new(),
        })
    }

    /// Whether nothing is contained.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
            && self.net_segments.is_empty()
            && self.planes.is_empty()
            && self.zones.is_empty()
            && self.polygons.is_empty()
            && self.stroke_texts.is_empty()
            && self.holes.is_empty()
    }

    /// The directory holding the library elements (`dev/`, `pkg/`).
    pub fn directory(&mut self, path: &str) -> TransactionalDirectory {
        self.directory.subdir(path)
    }

    /// The `board.lp` content.
    pub fn to_sexpression(&self) -> SExpression {
        let mut root = List::new("librepcb_clipboard_board");
        root.ensure_line_break();
        self.cursor_pos
            .serialize(root.append_list("cursor_position"));
        root.ensure_line_break();
        root.append_child("board", &self.board_uuid);
        root.ensure_line_break();
        for d in &self.devices {
            root.ensure_line_break();
            d.serialize(root.append_list("device"));
        }
        root.ensure_line_break();
        for s in &self.net_segments {
            root.ensure_line_break();
            s.serialize(root.append_list("netsegment"));
        }
        root.ensure_line_break();
        for p in &self.planes {
            root.ensure_line_break();
            p.serialize(root.append_list("plane"));
        }
        for z in &self.zones {
            root.ensure_line_break();
            z.serialize(root.append_list("zone"));
        }
        root.ensure_line_break();
        for p in &self.polygons {
            root.ensure_line_break();
            p.serialize(root.append_list("polygon"));
        }
        root.ensure_line_break();
        for t in &self.stroke_texts {
            root.ensure_line_break();
            t.serialize(root.append_list("stroke_text"));
        }
        root.ensure_line_break();
        for h in &self.holes {
            root.ensure_line_break();
            h.serialize(root.append_list("hole"));
        }
        root.ensure_line_break();
        for ((device, pad), pos) in &self.pad_positions {
            root.ensure_line_break();
            let child = root.append_list("pad_position");
            child.append_child("device", device);
            child.append_child("pad", pad);
            pos.serialize(child.append_list("position"));
        }
        root.ensure_line_break();
        SExpression::List(root)
    }

    /// Serializes the data for the clipboard (upstream `toMimeData()`):
    /// a ZIP archive with `board.lp` and the library elements.
    pub fn to_zip(&self) -> Result<Vec<u8>> {
        let sexpr = self.to_sexpression().to_byte_array(Mode::LibrePcb)?;
        let fs = self.directory.file_system();
        fs.write(
            &format!("{}board.lp", prefix(self.directory.path())),
            &sexpr,
        )?;
        Ok(fs.export_to_zip(None)?)
    }

    /// Loads the data from clipboard content (upstream constructor from
    /// MIME data).
    pub fn from_zip(zip: &[u8]) -> Result<Self> {
        let directory = TransactionalDirectory::new_temporary()?;
        directory.file_system().load_from_zip_bytes(zip.to_vec())?;
        let root = SExpression::parse(&directory.read("board.lp")?, None, Mode::LibrePcb)?;
        let mut data = Self::new(root.child_value("board/@0")?, Point::ORIGIN)?;
        data.directory = directory;
        data.cursor_pos = Point::deserialize(root.required_child("cursor_position")?)?;
        for child in root.children_named("device") {
            data.devices.push(ClipboardDevice::deserialize(child)?);
        }
        for child in root.children_named("netsegment") {
            data.net_segments
                .push(ClipboardNetSegment::deserialize(child)?);
        }
        for child in root.children_named("plane") {
            data.planes.push(ClipboardPlane::deserialize(child)?);
        }
        for child in root.children_named("zone") {
            data.zones.push(BoardZoneData::deserialize(child)?);
        }
        for child in root.children_named("polygon") {
            data.polygons.push(BoardPolygonData::deserialize(child)?);
        }
        for child in root.children_named("stroke_text") {
            data.stroke_texts
                .push(BoardStrokeTextData::deserialize(child)?);
        }
        for child in root.children_named("hole") {
            data.holes.push(BoardHoleData::deserialize(child)?);
        }
        for child in root.children_named("pad_position") {
            data.pad_positions.insert(
                (
                    child.child_value("device/@0")?,
                    child.child_value("pad/@0")?,
                ),
                Point::deserialize(child.required_child("position")?)?,
            );
        }
        Ok(data)
    }

    /// Builds the clipboard data of the selected items (port of upstream
    /// `BoardClipboardDataBuilder::generate()`).
    pub fn from_selection(query: &mut SelectionQuery<'_>, cursor_pos: Point) -> Result<Self> {
        query
            .add_devices()
            .add_board_pads()
            .add_vias()
            .add_traces()
            .add_planes()
            .add_zones()
            .add_polygons()
            .add_board_texts()
            .add_holes()
            .add_junctions_of_traces(false);
        let p = query.project();
        let board = query.board();
        let mut data = Self::new(board.uuid(), cursor_pos)?;

        // Devices with their library elements.
        for c in &query.devices {
            let Some(dev) = board.device(*c) else {
                continue;
            };
            let lib_dev =
                p.library()
                    .device(&dev.lib_device())
                    .ok_or(Error::NotInProjectLibrary {
                        kind: librepcb_core::project::LibraryElementKind::Device,
                        uuid: dev.lib_device(),
                    })?;
            let mut dev_dir = data.directory(&format!("dev/{}", dev.lib_device()));
            if dev_dir.files("").is_empty() {
                lib_dev.directory().copy_to(&mut dev_dir)?;
            }
            let pkg_uuid = lib_dev.package_uuid();
            if let Some(pkg) = p.library().package(&pkg_uuid) {
                let mut pkg_dir = data.directory(&format!("pkg/{pkg_uuid}"));
                if pkg_dir.files("").is_empty() {
                    pkg.directory().copy_to(&mut pkg_dir)?;
                }
            }
            data.devices.push(ClipboardDevice {
                component: *c,
                lib_device: dev.lib_device(),
                lib_footprint: dev.lib_footprint(),
                position: dev.position(),
                rotation: dev.rotation(),
                mirrored: dev.mirrored(),
                locked: dev.locked(),
                glue: dev.glue(),
                attributes: dev.attributes().clone(),
                stroke_texts: dev.stroke_texts().values().cloned().collect(),
            });
            for pad in dev.pads(p.library(), p.circuit())? {
                data.pad_positions.insert((c.0, pad.uuid()), pad.position());
            }
        }

        // Net segments, split into the copied parts.
        for (id, items) in query.segment_items() {
            let Some(seg) = board.net_segment(id) else {
                continue;
            };
            let mut splitter = BoardNetSegmentSplitter::new();
            for (c, dev) in board.devices() {
                if query.devices.contains(c) {
                    continue;
                }
                for pad in dev.pads(p.library(), p.circuit())? {
                    if board.footprint_pad_segment(*c, pad.uuid()) == Some(id) {
                        splitter
                            .replace_footprint_pad_by_junctions(pad.trace_anchor(), pad.position());
                    }
                }
            }
            for pad in seg.pads().values() {
                splitter.add_pad(pad.clone(), !items.pads.contains(&pad.uuid()));
            }
            for via in seg.vias().values() {
                splitter.add_via(via.clone(), !items.vias.contains(&via.uuid()));
            }
            for j in &items.junctions {
                if let Some(junction) = seg.junctions().get(j) {
                    splitter.add_junction(junction.clone());
                }
            }
            for t in &items.traces {
                if let Some(trace) = seg.traces().get(t) {
                    splitter.add_trace(trace, Uuid::new_random);
                }
            }
            let net_name = seg
                .net()
                .and_then(|n| p.circuit().net_signal(n))
                .map(|n| n.name().clone());
            for part in splitter.split() {
                data.net_segments.push(ClipboardNetSegment {
                    net_name: net_name.clone(),
                    pads: part.pads,
                    vias: part.vias,
                    junctions: part.junctions,
                    traces: part.traces,
                });
            }
        }

        for id in &query.planes {
            if let Some(plane) = board.plane(*id) {
                data.planes.push(ClipboardPlane {
                    plane: plane.clone(),
                    net_name: plane
                        .net()
                        .and_then(|n| p.circuit().net_signal(n))
                        .map(|n| n.name().clone()),
                });
            }
        }
        data.zones.extend(
            query
                .zones
                .iter()
                .filter_map(|u| board.zones().get(u).cloned()),
        );
        data.polygons.extend(
            query
                .polygons
                .iter()
                .filter_map(|u| board.polygons().get(u).cloned()),
        );
        data.stroke_texts.extend(
            query
                .stroke_texts
                .iter()
                .filter_map(|u| board.stroke_texts().get(u).cloned()),
        );
        data.holes.extend(
            query
                .holes
                .iter()
                .filter_map(|u| board.holes().get(u).cloned()),
        );
        Ok(data)
    }
}

fn prefix(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    }
}

/// Pastes clipboard data (port of upstream `CmdPasteBoardItems`): devices
/// only if their component exists and has no device on the board yet,
/// traces at pads of devices which are not pasted end in new junctions,
/// all other items get new UUIDs and are moved by `offset`. Returns the
/// pasted items (upstream selects them).
#[derive(Debug)]
pub struct PasteBoardItems {
    /// The board.
    pub board: BoardId,
    /// The data.
    pub data: BoardClipboardData,
    /// The offset added to all positions.
    pub offset: Point,
}

impl Command for PasteBoardItems {
    type Output = Vec<BoardItemRef>;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdPasteBoardItems",
            "Paste Board Elements"
        )
    }

    fn execute(mut self, tx: &mut Transaction<'_>) -> Result<Vec<BoardItemRef>> {
        let board = self.board;
        let offset = self.offset;
        let mut pasted_items = Vec::new();

        // Devices which do not exist on the board yet.
        let mut pasted_devices: BTreeSet<Uuid> = BTreeSet::new();
        let devices = std::mem::take(&mut self.data.devices);
        for dev in &devices {
            let p = tx.project();
            if p.circuit().component_instance(dev.component).is_none() {
                continue; // The component does not exist (anymore).
            }
            let b = p
                .board(board)
                .ok_or_else(|| Error::not_found("Board", board))?;
            if b.device(dev.component).is_some() {
                continue; // The device exists already.
            }
            // Copy the library device and package if missing.
            let pkg_uuid = match p.library().device(&dev.lib_device) {
                Some(d) => d.package_uuid(),
                None => {
                    let lib_dev =
                        Device::open(self.data.directory(&format!("dev/{}", dev.lib_device)))?;
                    let pkg = lib_dev.package_uuid();
                    tx.add_library_element(LibraryElement::Device(lib_dev))?;
                    pkg
                }
            };
            if tx.project().library().package(&pkg_uuid).is_none() {
                let pkg = Package::open(self.data.directory(&format!("pkg/{pkg_uuid}")))?;
                tx.add_library_element(LibraryElement::Package(pkg))?;
            }
            let mut device = BoardDevice::new(
                dev.component,
                dev.lib_device,
                dev.lib_footprint,
                dev.position + offset,
                dev.rotation,
                dev.mirrored,
                dev.locked,
                dev.glue,
            );
            device.set_attributes(dev.attributes.clone());
            // upstream `BI_Device` constructor: the default 3D model.
            let lib = tx.project().library();
            if let Some(pkg) = lib.package(&pkg_uuid) {
                let model = device.default_lib_model(pkg);
                device.set_lib_model(model);
            }
            for text in &dev.stroke_texts {
                // Keep the UUID: it references the library footprint text.
                let mut copy = text.clone();
                copy.set_position(copy.position() + offset);
                device.insert_stroke_text(copy);
            }
            tx.apply(Mutation::Board(BoardMutation::AddDevice { board, device }))?;
            pasted_devices.insert(dev.component.0);
            pasted_items.push(BoardItemRef::Device(dev.component));
        }

        // Net segments.
        for seg in &self.data.net_segments {
            let mut splitter = BoardNetSegmentSplitter::new();
            for ((device, pad), pos) in &self.data.pad_positions {
                if !pasted_devices.contains(device) {
                    splitter.replace_footprint_pad_by_junctions(
                        TraceAnchor::FootprintPad {
                            device: *device,
                            pad: *pad,
                        },
                        *pos,
                    );
                }
            }
            for pad in &seg.pads {
                splitter.add_pad(pad.clone(), false);
            }
            for via in &seg.vias {
                splitter.add_via(via.clone(), false);
            }
            for j in &seg.junctions {
                splitter.add_junction(j.clone());
            }
            for t in &seg.traces {
                splitter.add_trace(t, Uuid::new_random);
            }
            for part in splitter.split() {
                let net = match &seg.net_name {
                    Some(name) => Some(net_by_name(tx, name)?),
                    None => None,
                };
                let mut map: BTreeMap<TraceAnchor, TraceAnchor> = BTreeMap::new();
                let pads: Vec<BoardPadData> = part
                    .pads
                    .iter()
                    .map(|pad| {
                        let mut copy = pad.with_uuid(Uuid::new_random());
                        let pos = copy.pad().position() + offset;
                        copy.pad_mut().set_position(pos);
                        map.insert(TraceAnchor::Pad(pad.uuid()), TraceAnchor::Pad(copy.uuid()));
                        copy
                    })
                    .collect();
                let vias: Vec<Via> = part
                    .vias
                    .iter()
                    .map(|via| {
                        let mut copy = via.with_uuid(Uuid::new_random());
                        copy.set_position(copy.position() + offset);
                        map.insert(TraceAnchor::Via(via.uuid()), TraceAnchor::Via(copy.uuid()));
                        copy
                    })
                    .collect();
                let junctions: Vec<Junction> = part
                    .junctions
                    .iter()
                    .map(|j| {
                        let copy = Junction::new(Uuid::new_random(), j.position() + offset);
                        map.insert(
                            TraceAnchor::Junction(j.uuid()),
                            TraceAnchor::Junction(copy.uuid()),
                        );
                        copy
                    })
                    .collect();
                let resolve = |a: TraceAnchor| -> Result<TraceAnchor> {
                    match a {
                        TraceAnchor::FootprintPad { device, .. }
                            if pasted_devices.contains(&device) =>
                        {
                            Ok(a)
                        }
                        _ => map.get(&a).copied().ok_or_else(|| {
                            Error::InvalidArgument("Invalid trace anchor in clipboard.".into())
                        }),
                    }
                };
                let traces: Vec<Trace> = part
                    .traces
                    .iter()
                    .map(|t| {
                        Ok(Trace::new(
                            Uuid::new_random(),
                            t.layer(),
                            t.width(),
                            resolve(t.p1())?,
                            resolve(t.p2())?,
                        ))
                    })
                    .collect::<Result<_>>()?;
                let segment = BoardNetSegment::with_elements(
                    Uuid::new_random(),
                    net,
                    pads,
                    vias,
                    junctions,
                    traces,
                );
                let id = segment.id();
                pasted_items.extend(super::selection::segment_items(id, &segment, true));
                tx.apply(Mutation::Board(BoardMutation::AddNetSegment {
                    board,
                    segment,
                }))?;
            }
        }

        // Planes.
        for plane in &self.data.planes {
            let net = match &plane.net_name {
                Some(name) => Some(net_by_name(tx, name)?),
                None => None,
            };
            let mut copy = plane.plane.with_uuid(Uuid::new_random());
            copy.set_net(net);
            copy.set_outline(copy.outline().translated(offset));
            pasted_items.push(BoardItemRef::Plane(copy.id()));
            tx.apply(Mutation::Board(BoardMutation::AddPlane {
                board,
                plane: copy,
            }))?;
        }

        // Zones, polygons, texts and holes.
        let add = |tx: &mut Transaction<'_>, item: BoardItem| -> Result<()> {
            tx.apply(Mutation::Board(BoardMutation::AddItem { board, item }))
        };
        for zone in &self.data.zones {
            let mut copy = zone.with_uuid(Uuid::new_random());
            copy.set_outline(copy.outline().translated(offset));
            pasted_items.push(BoardItemRef::Zone(copy.uuid()));
            add(tx, BoardItem::Zone(copy))?;
        }
        for polygon in &self.data.polygons {
            let mut copy = polygon.with_uuid(Uuid::new_random());
            copy.set_path(copy.path().translated(offset));
            pasted_items.push(BoardItemRef::Polygon(copy.uuid()));
            add(tx, BoardItem::Polygon(copy))?;
        }
        for text in &self.data.stroke_texts {
            let mut copy = text.with_uuid(Uuid::new_random());
            copy.set_position(copy.position() + offset);
            pasted_items.push(BoardItemRef::StrokeText(copy.uuid()));
            add(tx, BoardItem::StrokeText(copy))?;
        }
        for hole in &self.data.holes {
            let mut copy = hole.with_uuid(Uuid::new_random());
            copy.set_path(
                NonEmptyPath::new(copy.path().get().translated(offset))
                    .map_err(|e| Error::InvalidArgument(e.to_string()))?,
            );
            pasted_items.push(BoardItemRef::Hole(copy.uuid()));
            add(tx, BoardItem::Hole(copy))?;
        }
        self.data.devices = devices;
        Ok(pasted_items)
    }
}

/// Returns the net with the given name, or a new net in the net class
/// "default" (upstream `CmdPasteBoardItems::getOrCreateNetSignal()`: the
/// new net gets an automatic name, like upstream).
fn net_by_name(tx: &mut Transaction<'_>, name: &CircuitIdentifier) -> Result<NetSignalId> {
    if let Some((id, _)) = tx.project().circuit().net_signal_by_name(name.as_str()) {
        return Ok(id);
    }
    let class = default_net_class(tx)?;
    add_net(tx, None, class)
}
