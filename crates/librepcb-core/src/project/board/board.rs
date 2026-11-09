//! Port of libs/librepcb/core/project/board/board.{h,cpp}.
//!
//! Differences to upstream: no project back-pointer, no `isAddedToProject`
//! state, no own `TransactionalDirectory` (the project writes
//! `boards/<directory_name>/` on save); the item `add*()`/`remove*()`
//! operations live in the project mutations, the Qt signals in the change
//! journal, and the derived data (air wires, plane fragments) with its
//! dirty sets in [`BoardDerived`], which is neither saved nor compared.
//! The board setup which upstream's `CmdBoardEdit` changes (layers,
//! thickness, colors, design rules, DRC settings, ...) plus the grid,
//! default font and fabrication output settings are grouped in
//! [`BoardSettings`]; the name stays in [`BoardProperties`].
//!
//! The reverse index "footprint pad → net segment" (upstream
//! `BI_Pad::mRegisteredNetLines` of footprint pads) is kept per board, not
//! persistent and rebuilt when the board is added to a project.
//!
//! Not ported yet: `buildScene3D()`, `calculateBoundingRect()` (need the
//! scene/painter modules), `copyFrom()` (editor), DRC approval cleanup
//! (`updateDrcMessageApprovals()`, part of the DRC).

use std::collections::{BTreeMap, BTreeSet};

use super::{
    AirWire, BoardDesignRules, BoardDevice, BoardFabricationOutputSettings, BoardHoleData,
    BoardItemKind, BoardNetSegment, BoardPlane, BoardPolygonData, BoardStrokeTextData,
    BoardZoneData,
};
use crate::application;
use crate::fileio::{self, FileSystem, TransactionalDirectory};
use crate::geometry::{Path, TraceAnchor, property};
use crate::library::org::BoardDesignRuleCheckSettings;
use crate::project::circuit::Circuit;
use crate::project::id::{
    BoardId, ComponentInstanceId, ComponentSignalRef, NetSegmentId, NetSignalId, PlaneId,
};
use crate::project::library::ProjectLibrary;
use crate::project::ref_index::{NetUse, Reference};
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, Mode, SExpression, SerializeObject,
};
use crate::types::{
    ElementName, Layer, Length, LengthUnit, PcbColor, Point, PositiveLength, Tag, Uuid, Version,
};

/// Upstream `Application::getDefaultStrokeFontName()`.
pub const DEFAULT_STROKE_FONT_NAME: &str = "newstroke.bene";

/// Tags of the footprints preferred when adding devices (upstream
/// `Board::PreferredFootprintTags`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreferredFootprintTags {
    /// For THT devices on the top side.
    pub tht_top: Vec<Tag>,
    /// For THT devices on the bottom side.
    pub tht_bot: Vec<Tag>,
    /// For SMT devices on the top side.
    pub smt_top: Vec<Tag>,
    /// For SMT devices on the bottom side.
    pub smt_bot: Vec<Tag>,
    /// For all devices.
    pub common: Vec<Tag>,
}

impl PreferredFootprintTags {
    /// Whether no tags are set.
    pub fn is_empty(&self) -> bool {
        self.tht_top.is_empty()
            && self.tht_bot.is_empty()
            && self.smt_top.is_empty()
            && self.smt_bot.is_empty()
            && self.common.is_empty()
    }

    fn lists(&self) -> [(&'static str, &Vec<Tag>); 5] {
        [
            ("tht_top", &self.tht_top),
            ("tht_bot", &self.tht_bot),
            ("smt_top", &self.smt_top),
            ("smt_bot", &self.smt_bot),
            ("common", &self.common),
        ]
    }
}

impl SerializeObject for PreferredFootprintTags {
    fn serialize(&self, root: &mut List) {
        root.ensure_line_break();
        for (name, tags) in self.lists() {
            let node = root.append_list(name);
            for tag in tags {
                node.append_value(tag);
            }
            root.ensure_line_break();
        }
    }
}

impl DeserializeObject for PreferredFootprintTags {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let load = |name: &str| -> serialization::Result<Vec<Tag>> {
            node.required_child(name)?
                .children()
                .iter()
                .filter(|c| c.is_string())
                .map(Tag::from_sexpression)
                .collect()
        };
        Ok(Self {
            tht_top: load("tht_top")?,
            tht_bot: load("tht_bot")?,
            smt_top: load("smt_top")?,
            smt_bot: load("smt_bot")?,
            common: load("common")?,
        })
    }
}

/// The setup of a board stored in `board.lp` (everything except UUID, name,
/// DRC approvals and items).
///
/// Serde: an object with the fields below.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BoardSettings {
    /// File name of the default stroke font.
    pub default_font: String,
    /// Grid interval.
    pub grid_interval: PositiveLength,
    /// Grid unit.
    pub grid_unit: LengthUnit,
    /// Number of inner copper layers.
    pub inner_layer_count: u32,
    /// Total PCB thickness.
    pub pcb_thickness: PositiveLength,
    /// Solder resist color (`None` = no solder resist).
    pub solder_resist: Option<PcbColor>,
    /// Silkscreen color.
    pub silkscreen_color: PcbColor,
    /// Layers printed on the top silkscreen (empty = no top silkscreen).
    pub silkscreen_layers_top: Vec<Layer>,
    /// Layers printed on the bottom silkscreen (empty = no bottom
    /// silkscreen).
    pub silkscreen_layers_bot: Vec<Layer>,
    /// Design rules.
    pub design_rules: BoardDesignRules,
    /// DRC settings.
    pub drc_settings: BoardDesignRuleCheckSettings,
    /// Fabrication output settings.
    pub fabrication_output_settings: BoardFabricationOutputSettings,
    /// Preferred footprint tags.
    pub preferred_footprint_tags: PreferredFootprintTags,
}

impl Default for BoardSettings {
    /// The settings of a new board (upstream `Board` constructor).
    fn default() -> Self {
        Self {
            default_font: DEFAULT_STROKE_FONT_NAME.to_owned(),
            grid_interval: PositiveLength::new(Length::new(635_000)).expect("positive"),
            grid_unit: LengthUnit::Millimeters,
            inner_layer_count: 0,
            pcb_thickness: PositiveLength::new(Length::new(1_600_000)).expect("positive"),
            solder_resist: Some(PcbColor::Green),
            silkscreen_color: PcbColor::White,
            silkscreen_layers_top: vec![Layer::TOP_LEGEND, Layer::TOP_NAMES],
            silkscreen_layers_bot: vec![Layer::BOT_LEGEND, Layer::BOT_NAMES],
            design_rules: BoardDesignRules::default(),
            drc_settings: BoardDesignRuleCheckSettings::default(),
            fabrication_output_settings: BoardFabricationOutputSettings::default(),
            preferred_footprint_tags: PreferredFootprintTags::default(),
        }
    }
}

impl BoardSettings {
    /// Returns the copper layers of the board (top, inner and bottom,
    /// upstream `getCopperLayers()`).
    pub fn copper_layers(&self) -> BTreeSet<Layer> {
        let inner = (1..=self.inner_layer_count as usize).filter_map(Layer::inner_copper);
        [Layer::TOP_COPPER, Layer::BOT_COPPER]
            .into_iter()
            .chain(inner)
            .collect()
    }

    /// Returns the silkscreen color of the top side, if there is a top
    /// silkscreen.
    pub fn silkscreen_color_top(&self) -> Option<PcbColor> {
        (!self.silkscreen_layers_top.is_empty()).then_some(self.silkscreen_color)
    }

    /// Returns the silkscreen color of the bottom side, if there is a bottom
    /// silkscreen.
    pub fn silkscreen_color_bot(&self) -> Option<PcbColor> {
        (!self.silkscreen_layers_bot.is_empty()).then_some(self.silkscreen_color)
    }
}

/// A zone, polygon, stroke text or hole of a board (the board items without
/// net connections), for the generic item mutations.
///
/// Serde: externally tagged, e.g. `{"Hole": {...}}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BoardItem {
    /// A keepout zone.
    Zone(BoardZoneData),
    /// A polygon.
    Polygon(BoardPolygonData),
    /// A stroke text.
    StrokeText(BoardStrokeTextData),
    /// A non-plated hole.
    Hole(BoardHoleData),
}

impl BoardItem {
    /// Returns the kind of the item.
    pub fn kind(&self) -> BoardItemKind {
        match self {
            Self::Zone(_) => BoardItemKind::Zone,
            Self::Polygon(_) => BoardItemKind::Polygon,
            Self::StrokeText(_) => BoardItemKind::StrokeText,
            Self::Hole(_) => BoardItemKind::Hole,
        }
    }

    /// Returns the UUID of the item.
    pub fn uuid(&self) -> Uuid {
        match self {
            Self::Zone(z) => z.uuid(),
            Self::Polygon(p) => p.uuid(),
            Self::StrokeText(t) => t.uuid(),
            Self::Hole(h) => h.uuid(),
        }
    }
}

/// A use of a footprint pad by a net segment (the per-board reverse index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PadUse {
    /// The segment with traces connected to the pad.
    pub segment: NetSegmentId,
    /// The component signal of the pad (from the library device's
    /// pad-signal map), if connected.
    pub signal: Option<Uuid>,
}

/// Key of a footprint pad on a board: (component instance, footprint pad).
pub(crate) type FootprintPadKey = (ComponentInstanceId, Uuid);

/// The non-persistent reverse index of a board (never compared).
#[derive(Debug, Clone, Default)]
pub(crate) struct BoardIndex {
    /// Footprint pads with traces → their net segment.
    pub pad_uses: BTreeMap<FootprintPadKey, PadUse>,
}

impl PartialEq for BoardIndex {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

/// A board of a project.
///
/// Serde: an object with the fields below (item maps keyed by UUID; the
/// derived data and the reverse index are skipped).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Board {
    pub(crate) uuid: Uuid,
    /// Name of the directory below `boards/`.
    pub(crate) directory_name: String,
    pub(crate) name: ElementName,
    /// Boxed: large (DRC settings) and rarely changed.
    pub(crate) settings: Box<BoardSettings>,
    /// File format version of the DRC approvals (upstream
    /// `mDrcMessageApprovalsVersion`).
    pub(crate) drc_approvals_version: Version,
    pub(crate) drc_approvals: BTreeSet<SExpression>,
    /// Layer visibility by layer ID (user settings, `settings.user.lp`).
    pub(crate) layers_visibility: BTreeMap<String, bool>,
    /// Keyed by component instance (upstream `mDeviceInstances`).
    #[serde(with = "super::keyed_map")]
    pub(crate) devices: BTreeMap<ComponentInstanceId, BoardDevice>,
    #[serde(with = "super::keyed_map")]
    pub(crate) net_segments: BTreeMap<NetSegmentId, BoardNetSegment>,
    #[serde(with = "super::keyed_map")]
    pub(crate) planes: BTreeMap<PlaneId, BoardPlane>,
    #[serde(with = "super::keyed_map")]
    pub(crate) zones: BTreeMap<Uuid, BoardZoneData>,
    #[serde(with = "super::keyed_map")]
    pub(crate) polygons: BTreeMap<Uuid, BoardPolygonData>,
    #[serde(with = "super::keyed_map")]
    pub(crate) stroke_texts: BTreeMap<Uuid, BoardStrokeTextData>,
    #[serde(with = "super::keyed_map")]
    pub(crate) holes: BTreeMap<Uuid, BoardHoleData>,
    /// Reverse index, not persistent.
    #[serde(skip)]
    pub(crate) index: BoardIndex,
    /// Derived data, not persistent.
    #[serde(skip)]
    pub(crate) derived: Box<BoardDerived>,
}

// Top-level aggregate with caches (derived data, reverse index).
static_assertions::assert_impl_all!(Board: Send, Sync);

impl Board {
    /// Creates an empty board with the default settings.
    pub fn new(uuid: Uuid, name: ElementName, directory_name: impl Into<String>) -> Self {
        Self {
            uuid,
            directory_name: directory_name.into(),
            name,
            settings: Box::default(),
            drc_approvals_version: application::file_format_version(),
            drc_approvals: BTreeSet::new(),
            layers_visibility: BTreeMap::new(),
            devices: BTreeMap::new(),
            net_segments: BTreeMap::new(),
            planes: BTreeMap::new(),
            zones: BTreeMap::new(),
            polygons: BTreeMap::new(),
            stroke_texts: BTreeMap::new(),
            holes: BTreeMap::new(),
            index: BoardIndex::default(),
            derived: Box::default(),
        }
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Returns the typed identifier.
    pub fn id(&self) -> BoardId {
        BoardId(self.uuid)
    }

    /// Returns the name of the directory below `boards/`.
    pub fn directory_name(&self) -> &str {
        &self.directory_name
    }

    property!(
        /// Returns the name.
        ref name: ElementName, set_name
    );

    /// Returns the settings (layers, design rules, DRC settings, ...).
    pub fn settings(&self) -> &BoardSettings {
        &self.settings
    }

    /// Replaces the settings (for boards not yet added to a project; use
    /// the board mutations otherwise).
    pub fn set_settings(&mut self, settings: BoardSettings) {
        *self.settings = settings;
    }

    /// Returns the design rules.
    pub fn design_rules(&self) -> &BoardDesignRules {
        &self.settings.design_rules
    }

    /// Returns the copper layers (upstream `getCopperLayers()`).
    pub fn copper_layers(&self) -> BTreeSet<Layer> {
        self.settings.copper_layers()
    }

    /// Returns the file format version of the DRC message approvals.
    pub fn drc_approvals_version(&self) -> &Version {
        &self.drc_approvals_version
    }

    /// Returns the approved DRC messages.
    pub fn drc_approvals(&self) -> &BTreeSet<SExpression> {
        &self.drc_approvals
    }

    /// Returns the layer visibility (user settings, by layer ID).
    pub fn layers_visibility(&self) -> &BTreeMap<String, bool> {
        &self.layers_visibility
    }

    /// Returns the devices, keyed by component instance.
    pub fn devices(&self) -> &BTreeMap<ComponentInstanceId, BoardDevice> {
        &self.devices
    }

    /// Returns the device of a component instance (upstream
    /// `getDeviceInstanceByComponentUuid()`).
    pub fn device(&self, component: ComponentInstanceId) -> Option<&BoardDevice> {
        self.devices.get(&component)
    }

    /// Returns the net segments.
    pub fn net_segments(&self) -> &BTreeMap<NetSegmentId, BoardNetSegment> {
        &self.net_segments
    }

    /// Returns a net segment.
    pub fn net_segment(&self, id: NetSegmentId) -> Option<&BoardNetSegment> {
        self.net_segments.get(&id)
    }

    /// Returns the planes.
    pub fn planes(&self) -> &BTreeMap<PlaneId, BoardPlane> {
        &self.planes
    }

    /// Returns a plane.
    pub fn plane(&self, id: PlaneId) -> Option<&BoardPlane> {
        self.planes.get(&id)
    }

    /// Returns the keepout zones.
    pub fn zones(&self) -> &BTreeMap<Uuid, BoardZoneData> {
        &self.zones
    }

    /// Returns the polygons.
    pub fn polygons(&self) -> &BTreeMap<Uuid, BoardPolygonData> {
        &self.polygons
    }

    /// Returns the stroke texts (not those of devices).
    pub fn stroke_texts(&self) -> &BTreeMap<Uuid, BoardStrokeTextData> {
        &self.stroke_texts
    }

    /// Returns the non-plated holes.
    pub fn holes(&self) -> &BTreeMap<Uuid, BoardHoleData> {
        &self.holes
    }

    /// Returns a zone, polygon, stroke text or hole.
    pub fn item(&self, kind: BoardItemKind, uuid: &Uuid) -> Option<BoardItem> {
        match kind {
            BoardItemKind::Zone => self.zones.get(uuid).cloned().map(BoardItem::Zone),
            BoardItemKind::Polygon => self.polygons.get(uuid).cloned().map(BoardItem::Polygon),
            BoardItemKind::StrokeText => self
                .stroke_texts
                .get(uuid)
                .cloned()
                .map(BoardItem::StrokeText),
            BoardItemKind::Hole => self.holes.get(uuid).cloned().map(BoardItem::Hole),
            _ => None,
        }
    }

    /// Returns the derived data (air wires, plane fragments).
    pub fn derived(&self) -> &BoardDerived {
        &self.derived
    }

    /// Returns the net segment whose traces are connected to a footprint
    /// pad, if any.
    pub fn footprint_pad_segment(
        &self,
        component: ComponentInstanceId,
        pad: Uuid,
    ) -> Option<NetSegmentId> {
        self.index
            .pad_uses
            .get(&(component, pad))
            .map(|u| u.segment)
    }

    /// Whether traces are connected to a pad of a device (upstream
    /// `BI_Device::isUsed()`).
    pub fn is_device_used(&self, component: ComponentInstanceId) -> bool {
        self.index.pad_uses.keys().any(|(c, _)| *c == component)
    }

    /// Returns the layers of the traces connected to an anchor (upstream
    /// `BI_Pad::isConnectedOnLayer()` for all layers).
    pub fn anchor_trace_layers(&self, anchor: TraceAnchor) -> BTreeSet<Layer> {
        let segments: Vec<&BoardNetSegment> = match anchor {
            TraceAnchor::FootprintPad { device, pad } => self
                .footprint_pad_segment(ComponentInstanceId(device), pad)
                .and_then(|id| self.net_segments.get(&id))
                .into_iter()
                .collect(),
            _ => self
                .net_segments
                .values()
                .filter(|s| s.contains_anchor(&anchor))
                .collect(),
        };
        segments
            .into_iter()
            .flat_map(|s| s.traces_at(anchor).map(|t| t.layer()))
            .collect()
    }

    /// Returns the position of a trace anchor of a net segment (footprint
    /// pads are resolved through the library), `None` if it does not
    /// exist.
    pub fn anchor_position(
        &self,
        segment: &BoardNetSegment,
        anchor: TraceAnchor,
        library: &ProjectLibrary,
        circuit: &Circuit,
    ) -> Option<Point> {
        match anchor {
            TraceAnchor::Junction(uuid) => segment.junctions().get(&uuid).map(|j| j.position()),
            TraceAnchor::Via(uuid) => segment.vias().get(&uuid).map(|v| v.position()),
            TraceAnchor::Pad(uuid) => segment.pads().get(&uuid).map(|p| p.pad().position()),
            TraceAnchor::FootprintPad { device, pad } => self
                .devices
                .get(&ComponentInstanceId(device))?
                .pad(&pad, library, circuit)
                .ok()
                .flatten()
                .map(|p| p.position()),
        }
    }

    /// Whether the board contains no items (upstream `isEmpty()`).
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
            && self.net_segments.is_empty()
            && self.planes.is_empty()
            && self.zones.is_empty()
            && self.polygons.is_empty()
            && self.stroke_texts.is_empty()
            && self.holes.is_empty()
    }

    /// Returns the editable properties.
    pub fn properties(&self) -> BoardProperties {
        BoardProperties {
            id: self.id(),
            name: self.name.clone(),
        }
    }

    /// Replaces the editable properties, returns the previous ones.
    pub fn set_properties(&mut self, p: BoardProperties) -> BoardProperties {
        let old = self.properties();
        self.name = p.name;
        old
    }

    /// All references of the items into the circuit, for the project's
    /// reverse index.
    pub(crate) fn references(&self, id: BoardId) -> Vec<Reference> {
        let segments = self.net_segments.values().filter_map(|s| {
            s.net().map(|net| Reference::Net {
                net,
                by: NetUse::BoardSegment(id, s.id()),
            })
        });
        let planes = self.planes.values().filter_map(|p| {
            p.net().map(|net| Reference::Net {
                net,
                by: NetUse::Plane(id, p.id()),
            })
        });
        let devices = self.devices.keys().map(|component| Reference::Device {
            component: *component,
            board: id,
        });
        segments.chain(planes).chain(devices).collect()
    }

    /// Whether traces are connected to a pad of the given component signal
    /// (upstream `ComponentSignalInstance::arePinsOrPadsUsed()`).
    pub(crate) fn is_component_signal_wired(&self, signal: ComponentSignalRef) -> bool {
        self.index
            .pad_uses
            .iter()
            .any(|((c, _), u)| (*c == signal.component) && (u.signal == Some(signal.signal)))
    }

    /// Builds the footprint pad index from the net segments; the pad
    /// signals come from the library devices (unknown devices or pads map to
    /// no signal).
    pub(crate) fn build_pad_index(
        &self,
        library: &ProjectLibrary,
    ) -> BTreeMap<FootprintPadKey, PadUse> {
        let mut index = BTreeMap::new();
        for segment in self.net_segments.values() {
            for (component, pad) in segment.footprint_pads() {
                let signal = self
                    .devices
                    .get(&component)
                    .and_then(|dev| pad_signal(dev, &pad, library));
                index.insert(
                    (component, pad),
                    PadUse {
                        segment: segment.id(),
                        signal,
                    },
                );
            }
        }
        index
    }

    /// Whether the footprint pad index equals a rebuild from scratch.
    pub(crate) fn is_pad_index_consistent(&self, library: &ProjectLibrary) -> bool {
        self.build_pad_index(library) == self.index.pad_uses
    }

    /// Writes `board.lp` and `settings.user.lp` into `dir` (upstream
    /// `Board::save()`).
    pub(crate) fn save(&self, dir: &mut TransactionalDirectory) -> fileio::Result<()> {
        let mut root = List::new("librepcb_board");
        self.serialize(&mut root);
        dir.write(
            "board.lp",
            &SExpression::from(root).to_byte_array(Mode::LibrePcb)?,
        )?;
        let mut user = List::new("librepcb_board_user_settings");
        self.serialize_user_settings(&mut user);
        dir.write(
            "settings.user.lp",
            &SExpression::from(user).to_byte_array(Mode::LibrePcb)?,
        )?;
        Ok(())
    }

    /// Writes the content of `settings.user.lp`.
    fn serialize_user_settings(&self, root: &mut List) {
        for (layer, visible) in &self.layers_visibility {
            root.ensure_line_break();
            let child = root.append_list("layer");
            child.push(SExpression::token(layer.as_str()));
            child.append_child("visible", visible);
        }
        root.ensure_line_break();
        for plane in self.planes.values() {
            root.ensure_line_break();
            let node = root.append_list("plane");
            node.append_value(&plane.uuid());
            node.append_child("visible", &plane.visible());
        }
        root.ensure_line_break();
    }

    /// Reads the content of `board.lp` (without validation against the
    /// circuit and library, which happens when the board is added to a
    /// project). Duplicate item UUIDs: the last one wins; the loader
    /// rejects them before, like upstream.
    pub fn deserialize(root: &SExpression, directory_name: &str) -> serialization::Result<Self> {
        let mut board = Self::new(
            root.child_value("@0")?,
            root.child_value("name/@0")?,
            directory_name,
        );
        *board.settings = deserialize_settings(root)?;
        let drc = root.required_child("design_rule_check")?;
        board.drc_approvals_version = drc.child_value("approvals_version/@0")?;
        board.drc_approvals = drc.children_named("approved").cloned().collect();
        for node in root.children_named("device") {
            let device = BoardDevice::deserialize(node)?;
            board.devices.insert(device.component(), device);
        }
        for node in root.children_named("netsegment") {
            let segment = BoardNetSegment::deserialize(node)?;
            board.net_segments.insert(segment.id(), segment);
        }
        for node in root.children_named("plane") {
            let plane = BoardPlane::deserialize(node)?;
            board.planes.insert(plane.id(), plane);
        }
        for node in root.children_named("zone") {
            let zone = BoardZoneData::deserialize(node)?;
            board.zones.insert(zone.uuid(), zone);
        }
        for node in root.children_named("polygon") {
            let polygon = BoardPolygonData::deserialize(node)?;
            board.polygons.insert(polygon.uuid(), polygon);
        }
        for node in root.children_named("stroke_text") {
            let text = BoardStrokeTextData::deserialize(node)?;
            board.stroke_texts.insert(text.uuid(), text);
        }
        for node in root.children_named("hole") {
            let hole = BoardHoleData::deserialize(node)?;
            board.holes.insert(hole.uuid(), hole);
        }
        Ok(board)
    }

    /// Applies the content of `settings.user.lp` (upstream
    /// `ProjectLoader::loadBoardUserSettings()`): layer visibility and
    /// plane visibility (unknown planes are ignored with a warning).
    pub fn load_user_settings(&mut self, root: &SExpression) -> serialization::Result<()> {
        let mut layers = BTreeMap::new();
        for node in root.children_named("layer") {
            let name: String = node.child_value("@0")?;
            layers.insert(name, node.child_value("visible/@0")?);
        }
        let mut planes = Vec::new();
        for node in root.children_named("plane") {
            let uuid: Uuid = node.child_value("@0")?;
            let visible: bool = node.child_value("visible/@0")?;
            planes.push((PlaneId(uuid), visible));
        }
        self.layers_visibility = layers;
        for (id, visible) in planes {
            match self.planes.get_mut(&id) {
                Some(plane) => {
                    plane.set_visible(visible);
                }
                None => log::warn!("Plane {id} doesn't exist, could not restore its visibility."),
            }
        }
        Ok(())
    }
}

/// Returns the component signal of a footprint pad of a device.
pub(crate) fn pad_signal(
    device: &BoardDevice,
    pad: &Uuid,
    library: &ProjectLibrary,
) -> Option<Uuid> {
    let lib_device = library.device(&device.lib_device())?;
    let package = library.package(&lib_device.package_uuid())?;
    let footprint = package.footprints().by_uuid(&device.lib_footprint())?;
    let package_pad = footprint.pads().by_uuid(pad)?.package_pad_uuid()?;
    lib_device
        .pad_signal_map()
        .by_uuid(&package_pad)?
        .signal_uuid()
}

fn deserialize_settings(root: &SExpression) -> serialization::Result<BoardSettings> {
    let layers = |name: &str| -> serialization::Result<Vec<Layer>> {
        root.required_child(name)?
            .children()
            .iter()
            .filter(|c| c.is_token())
            .map(Layer::from_sexpression)
            .collect()
    };
    Ok(BoardSettings {
        default_font: root.child_value("default_font/@0")?,
        grid_interval: root.child_value("grid/interval/@0")?,
        grid_unit: root.child_value("grid/unit/@0")?,
        inner_layer_count: root.child_value("layers/inner/@0")?,
        pcb_thickness: root.child_value("thickness/@0")?,
        solder_resist: root.child_value("solder_resist/@0")?,
        silkscreen_color: root.child_value("silkscreen/@0")?,
        silkscreen_layers_top: layers("silkscreen_layers_top")?,
        silkscreen_layers_bot: layers("silkscreen_layers_bot")?,
        design_rules: BoardDesignRules::deserialize(root.required_child("design_rules")?)?,
        drc_settings: BoardDesignRuleCheckSettings::deserialize(
            root.required_child("design_rule_check")?,
        )?,
        fabrication_output_settings: BoardFabricationOutputSettings::deserialize(
            root.required_child("fabrication_output_settings")?,
        )?,
        preferred_footprint_tags: PreferredFootprintTags::deserialize(
            root.required_child("preferred_footprint_tags")?,
        )?,
    })
}

impl SerializeObject for Board {
    /// Writes the content of `board.lp`.
    fn serialize(&self, root: &mut List) {
        let s = &self.settings;
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("name", &self.name);
        root.ensure_line_break();
        root.append_child("default_font", &s.default_font);
        root.ensure_line_break();
        let grid = root.append_list("grid");
        grid.append_child("interval", &s.grid_interval);
        grid.append_child("unit", &s.grid_unit);
        root.ensure_line_break();
        root.append_list("layers")
            .append_child("inner", &s.inner_layer_count);
        root.ensure_line_break();
        root.append_child("thickness", &s.pcb_thickness);
        root.ensure_line_break();
        root.append_child("solder_resist", &s.solder_resist);
        root.ensure_line_break();
        root.append_child("silkscreen", &s.silkscreen_color);
        root.ensure_line_break();
        for (name, layers) in [
            ("silkscreen_layers_top", &s.silkscreen_layers_top),
            ("silkscreen_layers_bot", &s.silkscreen_layers_bot),
        ] {
            let node = root.append_list(name);
            for layer in layers {
                node.append_value(layer);
            }
            root.ensure_line_break();
        }
        s.design_rules.serialize(root.append_list("design_rules"));
        root.ensure_line_break();
        {
            let node = root.append_list("design_rule_check");
            s.drc_settings.serialize(node);
            node.append_child("approvals_version", &self.drc_approvals_version);
            node.ensure_line_break();
            for approval in &self.drc_approvals {
                node.push(approval.clone());
                node.ensure_line_break();
            }
        }
        root.ensure_line_break();
        s.fabrication_output_settings
            .serialize(root.append_list("fabrication_output_settings"));
        root.ensure_line_break();
        s.preferred_footprint_tags
            .serialize(root.append_list("preferred_footprint_tags"));
        root.ensure_line_break();
        for device in self.devices.values() {
            root.ensure_line_break();
            device.serialize(root.append_list("device"));
        }
        root.ensure_line_break();
        for segment in self.net_segments.values() {
            root.ensure_line_break();
            segment.serialize(root.append_list("netsegment"));
        }
        root.ensure_line_break();
        for plane in self.planes.values() {
            root.ensure_line_break();
            plane.serialize(root.append_list("plane"));
        }
        for zone in self.zones.values() {
            root.ensure_line_break();
            zone.serialize(root.append_list("zone"));
        }
        root.ensure_line_break();
        for polygon in self.polygons.values() {
            root.ensure_line_break();
            polygon.serialize(root.append_list("polygon"));
        }
        root.ensure_line_break();
        for text in self.stroke_texts.values() {
            root.ensure_line_break();
            text.serialize(root.append_list("stroke_text"));
        }
        root.ensure_line_break();
        for hole in self.holes.values() {
            root.ensure_line_break();
            hole.serialize(root.append_list("hole"));
        }
        root.ensure_line_break();
    }
}

/// Derived (computed, not persistent) data of a board with the dirty sets
/// that schedule its recomputation (upstream `mAirWires`,
/// `mScheduledNetSignalsForAirWireRebuild`, `BI_Plane::mFragments`,
/// `mScheduledLayersForPlanesRebuild`).
///
/// The mutations only mark nets and layers dirty; the air wires are
/// rebuilt by [`Project::rebuild_air_wires()`](crate::project::Project::rebuild_air_wires)
/// (the editor calls it after each command, upstream
/// `triggerAirWiresRebuild()`), the plane fragments by
/// [`Project::rebuild_planes()`](crate::project::Project::rebuild_planes)
/// or a [`PlaneJob`](super::PlaneJob).
///
/// Never participates in equality: two boards with the same persistent
/// content are equal regardless of their derived data.
#[derive(Debug, Clone, Default)]
pub struct BoardDerived {
    pub(crate) air_wires: BTreeMap<Option<NetSignalId>, Vec<AirWire>>,
    pub(crate) dirty_air_wire_nets: BTreeSet<Option<NetSignalId>>,
    pub(crate) plane_fragments: BTreeMap<PlaneId, Vec<Path>>,
    pub(crate) dirty_plane_layers: BTreeSet<Layer>,
    /// Approvals of all DRC messages which occurred in a full check during
    /// this session (upstream `mSupportedDrcMessageApprovals`), see
    /// [`Project::drc_approvals_update()`](crate::project::Project::drc_approvals_update).
    pub(crate) supported_drc_approvals: BTreeSet<SExpression>,
}

impl PartialEq for BoardDerived {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl BoardDerived {
    /// Returns the air wires per net.
    pub fn air_wires(&self) -> &BTreeMap<Option<NetSignalId>, Vec<AirWire>> {
        &self.air_wires
    }

    /// Returns all air wires.
    pub fn all_air_wires(&self) -> impl Iterator<Item = &AirWire> {
        self.air_wires.values().flatten()
    }

    /// Returns the nets whose air wires need to be rebuilt (upstream
    /// `mScheduledNetSignalsForAirWireRebuild`).
    pub fn dirty_air_wire_nets(&self) -> &BTreeSet<Option<NetSignalId>> {
        &self.dirty_air_wire_nets
    }

    /// Returns the calculated fragments of the planes.
    pub fn plane_fragments(&self) -> &BTreeMap<PlaneId, Vec<Path>> {
        &self.plane_fragments
    }

    /// Returns the fragments of a plane (empty if not calculated).
    pub fn fragments_of(&self, plane: PlaneId) -> &[Path] {
        self.plane_fragments.get(&plane).map_or(&[], Vec::as_slice)
    }

    /// Returns the copper layers whose planes need to be rebuilt (upstream
    /// `mScheduledLayersForPlanesRebuild`).
    pub fn dirty_plane_layers(&self) -> &BTreeSet<Layer> {
        &self.dirty_plane_layers
    }

    /// Returns the approvals of all DRC messages which occurred during this
    /// session (upstream `mSupportedDrcMessageApprovals`).
    pub fn supported_drc_approvals(&self) -> &BTreeSet<SExpression> {
        &self.supported_drc_approvals
    }

    /// Schedules the air wires of a net for rebuild (upstream
    /// `scheduleAirWiresRebuild()`).
    pub(crate) fn schedule_air_wires(&mut self, net: Option<NetSignalId>) {
        self.dirty_air_wire_nets.insert(net);
    }

    /// Schedules the planes on the given layers for rebuild (upstream
    /// `invalidatePlanes()`).
    pub(crate) fn invalidate_planes(&mut self, layers: impl IntoIterator<Item = Layer>) {
        self.dirty_plane_layers.extend(layers);
    }
}

/// The editable properties of a [`Board`] (upstream `Board::setName()`;
/// the board setup is [`BoardSettings`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BoardProperties {
    /// The board.
    pub id: BoardId,
    /// The name.
    pub name: ElementName,
}
