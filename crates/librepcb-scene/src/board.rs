//! Scene builder of boards: port of the drawing semantics of
//! libs/librepcb/core/project/board/boardpainter.{h,cpp} and of the board
//! graphics items of libs/librepcb/editor/project/board/graphicsitems/
//! (planes, zones and air wires are drawn by the editor only).
//!
//! Canvas layers ([`BoardSceneLayer`]) are the board layers plus, per
//! copper layer, the THT pads, vias and zones on it (drawn in the pads,
//! vias and zones colors but shown and hidden with the copper layer, like
//! upstream's export does when copper layers are enabled), and the holes,
//! hole fills and air wires. The paint order is upstream's graphics export
//! order (bottom side first for the top view, reversed for the bottom view,
//! which is also mirrored). Differences to upstream:
//!
//! - Planes are drawn with their fragments if the board has computed them
//!   ([`Board::derived()`]), and always with their outline as a hairline.
//! - Pad, via and hole drills are filled with the background color instead
//!   of being cut out.
//! - Texts are stroke texts; the invisible TrueType texts upstream adds to
//!   PDF/SVG exports are not drawn.
//!
//! The scene is built per unit (a device with its pads and texts, a pad,
//! via or trace of a net segment, a plane, a zone, polygon, text or hole
//! of the board, the air wires of a net; see [`crate::units`]);
//! [`BoardScene::apply_changes()`] rebuilds only the units the changes of
//! the project's change journal affect, with the same code as the full
//! build (upstream's graphics items update themselves through signals).

use std::collections::{BTreeSet, HashMap, HashSet};

use librepcb_canvas::kurbo::{Affine, Circle as KCircle, Line, Rect};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{Item, ItemId, Layer as CanvasLayer, LayerId, Scene, Style, convert};
use librepcb_core::font::{StrokeFont, StrokeTextPathBuilder};
use librepcb_core::geometry::{Hole, PadGeometry, PadHoleList, Path, TraceAnchor, Via, ZoneLayers};
use librepcb_core::library::pkg::Footprint;
use librepcb_core::project::board::{
    Board, BoardDevice, BoardItemKind, BoardNetSegment, BoardSegmentItems, BoardStrokeTextData,
};
use librepcb_core::project::{
    BoardChange, BoardId, Change, ChangesSince, ComponentInstanceId, NetClassId, NetSegmentId,
    NetSignalId, PlaneId, Project,
};
use librepcb_core::types::{Layer, Length, Point, PositiveLength, Uuid};
use librepcb_core::utils::transform::Transform;

use crate::attributes;
use crate::colors::ColorScheme;
use crate::error::{Error, Result};
use crate::shapes::{self, Fill};
use crate::units::{Built, DepIndex, Units};

/// The side a board is viewed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BoardSide {
    /// Top view.
    #[default]
    Top,
    /// Bottom view (mirrored horizontally, bottom layers on top).
    Bottom,
}

/// A canvas layer of a [`BoardScene`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoardSceneLayer {
    /// Items on a board layer.
    Board(Layer),
    /// THT pads on a copper layer.
    ThtPads(Layer),
    /// Vias on a copper layer.
    Vias(Layer),
    /// Zones on a copper layer.
    Zones(Layer),
    /// Hole outlines (non-plated holes).
    Holes,
    /// Drills of pads, vias and holes, filled with the background color.
    HoleFills,
    /// Air wires.
    AirWires,
}

impl BoardSceneLayer {
    /// The canvas layer ID.
    pub fn id(self) -> LayerId {
        let copper = |l: Layer| l.copper_number() as u16;
        match self {
            Self::Board(l) => LayerId(
                Layer::all()
                    .iter()
                    .position(|x| *x == l)
                    .unwrap_or_default() as u16,
            ),
            Self::ThtPads(l) => LayerId(200 + copper(l)),
            Self::Vias(l) => LayerId(300 + copper(l)),
            Self::Zones(l) => LayerId(400 + copper(l)),
            Self::Holes => LayerId(500),
            Self::HoleFills => LayerId(501),
            Self::AirWires => LayerId(502),
        }
    }

    /// The color role (`None` for the hole fills, which use the
    /// background).
    fn color_role(self) -> Option<&'static str> {
        match self {
            Self::Board(l) => Some(l.color_role()),
            Self::ThtPads(_) => Some("board_pads"),
            Self::Vias(_) => Some("board_vias"),
            Self::Zones(_) => Some("board_zones"),
            Self::Holes => Some("board_holes"),
            Self::HoleFills => None,
            Self::AirWires => Some("board_airwires"),
        }
    }

    /// The board layer whose visibility the layer follows.
    fn board_layer(self) -> Option<Layer> {
        match self {
            Self::Board(l) | Self::ThtPads(l) | Self::Vias(l) | Self::Zones(l) => Some(l),
            _ => None,
        }
    }
}

/// The model object an item of a [`BoardScene`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoardObject {
    /// A device (footprint shapes, holes, zones and texts).
    Device(ComponentInstanceId),
    /// A footprint pad of a device.
    FootprintPad(ComponentInstanceId, Uuid),
    /// A standalone pad of a net segment.
    Pad(NetSegmentId, Uuid),
    /// A via of a net segment.
    Via(NetSegmentId, Uuid),
    /// A trace of a net segment.
    Trace(NetSegmentId, Uuid),
    /// A plane.
    Plane(PlaneId),
    /// A zone of the board.
    Zone(Uuid),
    /// A polygon of the board.
    Polygon(Uuid),
    /// A stroke text of the board or of a device.
    StrokeText(Uuid),
    /// A hole of the board.
    Hole(Uuid),
    /// An air wire of a net.
    AirWire(NetSignalId),
}

/// Sub-ordering of items within one canvas layer.
mod sub {
    pub const AREAS: i32 = 0;
    pub const PADS: i32 = 1;
    pub const LINES: i32 = 2;
    pub const TEXTS: i32 = 3;
}

/// A unit of a board scene: a model object whose items are built together
/// (see [`crate::units`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Unit {
    /// A device with its footprint pads and stroke texts.
    Device(ComponentInstanceId),
    /// A standalone pad of a net segment.
    Pad(NetSegmentId, Uuid),
    /// A via of a net segment.
    Via(NetSegmentId, Uuid),
    /// A trace of a net segment.
    Trace(NetSegmentId, Uuid),
    /// A plane (outline and fragments).
    Plane(PlaneId),
    /// A zone of the board.
    Zone(Uuid),
    /// A polygon of the board.
    Polygon(Uuid),
    /// A stroke text of the board.
    Text(Uuid),
    /// A hole of the board.
    Hole(Uuid),
    /// The air wires of a net.
    AirWires(Option<NetSignalId>),
}

impl Unit {
    /// The net segment of a pad, via or trace.
    fn segment(self) -> Option<NetSegmentId> {
        match self {
            Self::Pad(s, _) | Self::Via(s, _) | Self::Trace(s, _) => Some(s),
            _ => None,
        }
    }
}

/// A board as a canvas [`Scene`], seen from one side.
#[derive(Debug)]
pub struct BoardScene {
    board: BoardId,
    scene: Scene,
    scheme: ColorScheme,
    side: BoardSide,
    layers: Vec<BoardSceneLayer>,
    /// Paint order rank per canvas layer.
    ranks: HashMap<LayerId, i32>,
    copper_layers: BTreeSet<Layer>,
    units: Units<Unit, BoardObject>,
    /// Pad, via and trace units per net segment.
    segments: HashMap<NetSegmentId, HashSet<Unit>>,
    /// Devices whose footprint pads the traces end at.
    trace_devices: DepIndex<Unit, ComponentInstanceId>,
    /// Library elements the devices depend on.
    library: DepIndex<Unit, Uuid>,
    /// Stroke texts of the devices.
    device_texts: DepIndex<Unit, Uuid>,
    warnings: Vec<String>,
}

static_assertions::assert_impl_all!(BoardScene: Send, Sync);

/// What to rebuild, collected from [`Change`]s.
#[derive(Debug, Default)]
struct Dirty {
    units: HashSet<Unit>,
    /// Devices whose pads may have moved (the traces at them are rebuilt).
    moved_devices: BTreeSet<ComponentInstanceId>,
    /// Traces which were added, removed or modified (the pads of the
    /// devices at them change their connected layers).
    traces: HashSet<Unit>,
    /// Segments whose elements are all rebuilt.
    segments: BTreeSet<NetSegmentId>,
    /// Anchors whose traces are rebuilt (moved junctions, vias, pads).
    anchors: Vec<(NetSegmentId, TraceAnchor)>,
    /// Nets and net classes whose vias are rebuilt (default drill).
    nets: BTreeSet<NetSignalId>,
    net_classes: BTreeSet<NetClassId>,
    /// Library elements whose dependents are rebuilt.
    library: BTreeSet<Uuid>,
    /// Whether the layer visibility changed.
    visibility: bool,
}

impl Dirty {
    fn segment_items(&mut self, segment: NetSegmentId, items: &BoardSegmentItems, moved: bool) {
        self.units
            .extend(items.pads.iter().map(|u| Unit::Pad(segment, *u)));
        self.units
            .extend(items.vias.iter().map(|u| Unit::Via(segment, *u)));
        let traces = items.traces.iter().map(|u| Unit::Trace(segment, *u));
        self.units.extend(traces.clone());
        self.traces.extend(traces);
        if moved {
            self.anchors.extend(
                items
                    .pads
                    .iter()
                    .map(|u| (segment, TraceAnchor::Pad(*u)))
                    .chain(items.vias.iter().map(|u| (segment, TraceAnchor::Via(*u))))
                    .chain(
                        items
                            .junctions
                            .iter()
                            .map(|u| (segment, TraceAnchor::Junction(*u))),
                    ),
            );
        }
    }
}

impl BoardScene {
    /// Builds the scene of a board seen from `side` with a color scheme
    /// (e.g. [`ColorScheme::BOARD_DARK`]). The layer visibility is taken
    /// from the board's user settings (all layers are visible by default).
    /// Items which cannot be resolved are skipped and reported by
    /// [`warnings()`](Self::warnings).
    pub fn build(
        project: &Project,
        board: BoardId,
        side: BoardSide,
        scheme: &ColorScheme,
    ) -> Result<Self> {
        let b = project.board(board).ok_or(Error::BoardNotFound(board))?;
        let copper_layers = b.copper_layers();
        let layers = paint_order(&copper_layers, side);
        let mut scene = Self {
            board,
            scene: Scene::new(),
            scheme: scheme.clone(),
            side,
            layers,
            ranks: HashMap::new(),
            copper_layers,
            units: Units::new(),
            segments: HashMap::new(),
            trace_devices: DepIndex::new(),
            library: DepIndex::new(),
            device_texts: DepIndex::new(),
            warnings: Vec::new(),
        };
        scene.add_layers();
        let mut units: Vec<Unit> = b.devices().keys().map(|c| Unit::Device(*c)).collect();
        for (id, segment) in b.net_segments() {
            units.extend(segment_units(*id, segment));
        }
        units.extend(b.planes().keys().map(|p| Unit::Plane(*p)));
        units.extend(b.zones().keys().map(|u| Unit::Zone(*u)));
        units.extend(b.polygons().keys().map(|u| Unit::Polygon(*u)));
        units.extend(b.stroke_texts().keys().map(|u| Unit::Text(*u)));
        units.extend(b.holes().keys().map(|u| Unit::Hole(*u)));
        units.extend(b.derived().air_wires().keys().map(|n| Unit::AirWires(*n)));
        scene.rebuild_units(project, b, units);
        scene.apply_layers_visibility(b);
        Ok(scene)
    }

    /// The board of the scene.
    pub fn board(&self) -> BoardId {
        self.board
    }

    /// The side the board is viewed from.
    pub fn side(&self) -> BoardSide {
        self.side
    }

    /// The scene.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// The scene for modification (e.g. selection).
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// The model object of an item.
    pub fn object(&self, item: ItemId) -> Option<BoardObject> {
        self.units.object(item)
    }

    /// All canvas layers in paint order (first is painted first).
    pub fn layers(&self) -> &[BoardSceneLayer] {
        &self.layers
    }

    /// Shows or hides a board layer together with the THT pads, vias and
    /// zones on it.
    pub fn set_layer_visible(&mut self, layer: Layer, visible: bool) {
        let ids: Vec<LayerId> = self
            .layers
            .iter()
            .filter(|l| l.board_layer() == Some(layer))
            .map(|l| l.id())
            .collect();
        for id in ids {
            self.scene.set_layer_visible(id, visible);
        }
    }

    /// Shows or hides one canvas layer.
    pub fn set_scene_layer_visible(&mut self, layer: BoardSceneLayer, visible: bool) {
        self.scene.set_layer_visible(layer.id(), visible);
    }

    /// The background color of the color scheme.
    pub fn background(&self) -> Color {
        self.scheme.color_or_transparent("board_background")
    }

    /// Changes the color of the hole fills (should be the background).
    pub fn set_background(&mut self, color: Color) {
        self.scene
            .set_layer_color(BoardSceneLayer::HoleFills.id(), color);
    }

    /// Problems found while building (skipped items), sorted.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Bounding box of all visible items (world coordinates, mm).
    pub fn content_bounds(&self) -> Option<Rect> {
        crate::render::visible_bounds(&self.scene)
    }

    /// Updates the scene after the project changed: rebuilds exactly the
    /// items of the model objects the `changes` (from
    /// [`Project::changes_since()`]) affect, with the same code as
    /// [`build()`](Self::build), reusing the item IDs of rebuilt objects
    /// where possible. [`ChangesSince::Resync`] and changes affecting the
    /// whole board (board settings and name, project metadata and settings,
    /// boards added or removed, unknown changes) rebuild the scene from
    /// scratch (see [`rebuild()`](Self::rebuild)); returns whether that
    /// happened.
    ///
    /// Fails if the board no longer exists.
    pub fn apply_changes(&mut self, project: &Project, changes: ChangesSince<'_>) -> Result<bool> {
        let b = project
            .board(self.board)
            .ok_or(Error::BoardNotFound(self.board))?;
        let ChangesSince::Changes(changes) = changes else {
            self.rebuild(project)?;
            return Ok(true);
        };
        let mut dirty = Dirty::default();
        for change in changes {
            if !self.collect(change, &mut dirty) {
                self.rebuild(project)?;
                return Ok(true);
            }
        }
        let visibility = dirty.visibility;
        let units = self.resolve_dirty(project, b, dirty);
        self.rebuild_units(project, b, units);
        if visibility {
            self.apply_layers_visibility(b);
        }
        Ok(false)
    }

    /// Rebuilds the whole scene (the layer visibility is taken from the
    /// board again), keeping the color of the hole fills.
    pub fn rebuild(&mut self, project: &Project) -> Result<()> {
        let mut scene = Self::build(project, self.board, self.side, &self.scheme)?;
        if let Some(layer) = self.scene.layer(BoardSceneLayer::HoleFills.id()) {
            scene.set_background(layer.color);
        }
        *self = scene;
        Ok(())
    }

    fn add_layers(&mut self) {
        let background = self.scheme.color_or_transparent("board_background");
        for (rank, layer) in self.layers.iter().enumerate() {
            let rank = rank as i32;
            let color = layer
                .color_role()
                .map_or(background, |r| self.scheme.color_or_transparent(r));
            self.scene
                .set_layer(layer.id(), CanvasLayer::new(color).with_order(rank));
            self.ranks.insert(layer.id(), rank);
        }
    }

    /// Applies the layer visibility of the board's user settings (layers
    /// not listed are visible).
    fn apply_layers_visibility(&mut self, board: &Board) {
        let visibility = board.layers_visibility();
        let layers: Vec<(LayerId, bool)> = self
            .layers
            .iter()
            .filter_map(|l| {
                let layer = l.board_layer()?;
                Some((l.id(), visibility.get(layer.id()).copied().unwrap_or(true)))
            })
            .collect();
        for (id, visible) in layers {
            self.scene.set_layer_visible(id, visible);
        }
    }

    /// Records what a change affects; `false` if the whole scene must be
    /// rebuilt.
    fn collect(&self, change: &Change, dirty: &mut Dirty) -> bool {
        use BoardChange as B;
        match change {
            // Texts may show project and board attributes; the board
            // index may change.
            Change::ProjectMetadata
            | Change::ProjectSettings
            | Change::BoardAdded(_)
            | Change::BoardRemoved(_) => return false,
            Change::BoardChanged(id) => return *id != self.board,
            Change::OutputJobs
            | Change::ErcApprovals
            | Change::AssemblyVariantAdded(_)
            | Change::AssemblyVariantRemoved(_)
            | Change::AssemblyVariantChanged(_)
            | Change::NetClassAdded(_)
            | Change::NetClassRemoved(_)
            | Change::NetSignalAdded(_)
            | Change::NetSignalRemoved(_)
            | Change::BusAdded(_)
            | Change::BusRemoved(_)
            | Change::BusChanged(_)
            | Change::ComponentAdded(_)
            | Change::ComponentRemoved(_)
            | Change::ComponentSignalNetChanged { .. }
            | Change::SchematicAdded(_)
            | Change::SchematicRemoved(_)
            | Change::SchematicChanged(_)
            | Change::Schematic { .. } => {}
            // Default via drill of the net class.
            Change::NetSignalChanged(net) => {
                dirty.nets.insert(*net);
            }
            Change::NetClassChanged(class) => {
                dirty.net_classes.insert(*class);
            }
            // Texts (attributes).
            Change::ComponentChanged(component) => {
                dirty.units.insert(Unit::Device(*component));
            }
            Change::LibraryElementAdded { uuid, .. }
            | Change::LibraryElementRemoved { uuid, .. } => {
                dirty.library.insert(*uuid);
            }
            Change::Board { id, change } => {
                if *id != self.board {
                    return true;
                }
                match change {
                    B::Settings => return false,
                    B::DrcApprovals => {}
                    B::LayersVisibility => dirty.visibility = true,
                    B::DeviceAdded(c) | B::DeviceRemoved(c) | B::DeviceChanged(c) => {
                        dirty.units.insert(Unit::Device(*c));
                        dirty.moved_devices.insert(*c);
                    }
                    B::NetSegmentAdded(s)
                    | B::NetSegmentRemoved(s)
                    | B::NetSegmentNetChanged { segment: s, .. } => {
                        dirty.segments.insert(*s);
                    }
                    B::NetSegmentElementsAdded { segment, items }
                    | B::NetSegmentElementsRemoved { segment, items } => {
                        dirty.segment_items(*segment, items, false);
                    }
                    B::NetSegmentElementsChanged { segment, items } => {
                        dirty.segment_items(*segment, items, true);
                    }
                    B::PlaneAdded(p) | B::PlaneRemoved(p) | B::PlaneChanged(p) => {
                        dirty.units.insert(Unit::Plane(*p));
                    }
                    B::PlaneFragments(planes) => {
                        dirty.units.extend(planes.iter().map(|p| Unit::Plane(*p)));
                    }
                    B::AirWires(nets) => {
                        dirty.units.extend(nets.iter().map(|n| Unit::AirWires(*n)));
                    }
                    B::ItemAdded { kind, uuid }
                    | B::ItemRemoved { kind, uuid }
                    | B::ItemChanged { kind, uuid } => match kind {
                        BoardItemKind::Zone => {
                            dirty.units.insert(Unit::Zone(*uuid));
                        }
                        BoardItemKind::Polygon => {
                            dirty.units.insert(Unit::Polygon(*uuid));
                        }
                        BoardItemKind::Hole => {
                            dirty.units.insert(Unit::Hole(*uuid));
                        }
                        BoardItemKind::StrokeText => {
                            // A text of the board or of a device.
                            dirty.units.insert(Unit::Text(*uuid));
                            dirty.units.extend(self.device_texts.dependents(uuid));
                        }
                        _ => return false,
                    },
                    _ => return false,
                }
            }
            _ => return false,
        }
        true
    }

    /// Resolves the collected changes to the units to rebuild, including
    /// the dependents of rebuilt units.
    fn resolve_dirty(&self, project: &Project, b: &Board, mut dirty: Dirty) -> Vec<Unit> {
        let circuit = project.circuit();
        let mut units = std::mem::take(&mut dirty.units);
        // Whole segments (the old and the current elements).
        for s in &dirty.segments {
            let mut all: Vec<Unit> = self
                .segments
                .get(s)
                .into_iter()
                .flatten()
                .copied()
                .collect();
            if let Some(segment) = b.net_segment(*s) {
                all.extend(segment_units(*s, segment));
            }
            dirty
                .traces
                .extend(all.iter().filter(|u| matches!(u, Unit::Trace(..))));
            units.extend(all);
        }
        // Vias with the default drill of their net class.
        if !dirty.nets.is_empty() || !dirty.net_classes.is_empty() {
            for (id, segment) in b.net_segments() {
                let affected = segment.net().is_some_and(|n| {
                    dirty.nets.contains(&n)
                        || circuit
                            .net_signal(n)
                            .is_some_and(|n| dirty.net_classes.contains(&n.net_class()))
                });
                if affected {
                    units.extend(segment.vias().keys().map(|u| Unit::Via(*id, *u)));
                }
            }
        }
        // Library elements.
        if !dirty.library.is_empty() {
            let mut affected: Vec<Unit> = dirty
                .library
                .iter()
                .flat_map(|uuid| self.library.dependents(uuid))
                .collect();
            // Units which could not be resolved may resolve now.
            affected.extend(self.units.with_warnings());
            for unit in affected {
                if let Unit::Device(c) = unit {
                    dirty.moved_devices.insert(c);
                }
                units.insert(unit);
            }
        }
        // Traces at moved anchors.
        for (s, anchor) in &dirty.anchors {
            if let Some(segment) = b.net_segment(*s) {
                units.extend(
                    segment
                        .traces_at(*anchor)
                        .map(|t| Unit::Trace(*s, t.uuid())),
                );
            }
        }
        // Traces at the pads of moved devices.
        for c in &dirty.moved_devices {
            units.extend(self.trace_devices.dependents(c));
        }
        // Devices whose pads got or lost traces (connected layers).
        for trace in &dirty.traces {
            units.extend(
                self.trace_devices
                    .keys_of(trace)
                    .iter()
                    .map(|c| Unit::Device(*c)),
            );
            if let Unit::Trace(s, uuid) = trace
                && let Some(t) = b.net_segment(*s).and_then(|s| s.traces().get(uuid))
            {
                for anchor in [t.p1(), t.p2()] {
                    if let TraceAnchor::FootprintPad { device, .. } = anchor {
                        units.insert(Unit::Device(ComponentInstanceId(device)));
                    }
                }
            }
            // Standalone pads of the segment (a trace only ends at pads of
            // its own segment; there are few of them).
            if let Unit::Trace(s, _) = trace
                && let Some(segment) = b.net_segment(*s)
            {
                units.extend(segment.pads().keys().map(|u| Unit::Pad(*s, *u)));
            }
        }
        units.into_iter().collect()
    }

    /// Builds (or removes, if gone from the model) the items of units and
    /// updates the scene.
    fn rebuild_units(&mut self, project: &Project, board: &Board, mut units: Vec<Unit>) {
        if units.is_empty() {
            return;
        }
        // Devices first: they cache the pad positions for the traces.
        units.sort_by_key(|u| !matches!(u, Unit::Device(_)));
        let mut builder = Builder {
            project,
            board,
            scheme: &self.scheme,
            ranks: &self.ranks,
            copper_layers: &self.copper_layers,
            font: project
                .stroke_fonts()
                .font(&board.settings().default_font)
                .ok(),
            pad_positions: HashMap::new(),
            out: Built::default(),
            devices: Vec::new(),
            library: Vec::new(),
            texts: Vec::new(),
        };
        let mut updates = Vec::with_capacity(units.len());
        for unit in units {
            let exists = builder.build(unit);
            let built = std::mem::take(&mut builder.out);
            let devices = std::mem::take(&mut builder.devices);
            let library = std::mem::take(&mut builder.library);
            let texts = std::mem::take(&mut builder.texts);
            if exists {
                self.trace_devices.set(unit, devices);
                self.library.set(unit, library);
                self.device_texts.set(unit, texts);
                if let Some(s) = unit.segment() {
                    self.segments.entry(s).or_default().insert(unit);
                }
                updates.push((unit, Some(built)));
            } else {
                self.trace_devices.remove(unit);
                self.library.remove(unit);
                self.device_texts.remove(unit);
                if let Some(s) = unit.segment()
                    && let Some(set) = self.segments.get_mut(&s)
                {
                    set.remove(&unit);
                    if set.is_empty() {
                        self.segments.remove(&s);
                    }
                }
                updates.push((unit, None));
            }
        }
        self.units.commit(&mut self.scene, updates);
        self.warnings = self.units.warnings();
    }
}

/// The pad, via and trace units of a net segment.
fn segment_units(id: NetSegmentId, segment: &BoardNetSegment) -> impl Iterator<Item = Unit> + '_ {
    segment
        .pads()
        .keys()
        .map(move |u| Unit::Pad(id, *u))
        .chain(segment.vias().keys().map(move |u| Unit::Via(id, *u)))
        .chain(segment.traces().keys().map(move |u| Unit::Trace(id, *u)))
}

/// The canvas layers in paint order: upstream's graphics export order
/// (`GraphicsExportSettings::loadColorsFromScheme()`, painted in reverse),
/// with the package outlines and finish layers added next to their
/// neighbors.
fn paint_order(copper_layers: &BTreeSet<Layer>, side: BoardSide) -> Vec<BoardSceneLayer> {
    use BoardSceneLayer as S;
    let copper = |l: Layer| [S::Board(l), S::Zones(l), S::ThtPads(l), S::Vias(l)];
    let mut symmetric = vec![
        S::Board(Layer::BOT_DOCUMENTATION),
        S::Board(Layer::BOT_NAMES),
        S::Board(Layer::BOT_VALUES),
        S::Board(Layer::BOT_COURTYARD),
        S::Board(Layer::BOT_PACKAGE_OUTLINES),
        S::Board(Layer::BOT_LEGEND),
        S::Board(Layer::BOT_GLUE),
        S::Board(Layer::BOT_FINISH),
        S::Board(Layer::BOT_SOLDER_PASTE),
        S::Board(Layer::BOT_STOP_MASK),
    ];
    symmetric.extend(copper(Layer::BOT_COPPER));
    for layer in copper_layers.iter().rev().filter(|l| l.is_inner()) {
        symmetric.extend(copper(*layer));
    }
    symmetric.extend(copper(Layer::TOP_COPPER));
    symmetric.extend([
        S::Board(Layer::TOP_STOP_MASK),
        S::Board(Layer::TOP_SOLDER_PASTE),
        S::Board(Layer::TOP_FINISH),
        S::Board(Layer::TOP_GLUE),
        S::Board(Layer::TOP_LEGEND),
        S::Board(Layer::TOP_PACKAGE_OUTLINES),
        S::Board(Layer::TOP_COURTYARD),
        S::Board(Layer::TOP_VALUES),
        S::Board(Layer::TOP_NAMES),
        S::Board(Layer::TOP_DOCUMENTATION),
    ]);
    if side == BoardSide::Bottom {
        // Keep each copper group's internal order.
        let mut groups: Vec<Vec<S>> = Vec::new();
        for l in symmetric {
            match (l, groups.last_mut()) {
                (S::Zones(_) | S::ThtPads(_) | S::Vias(_), Some(g)) => g.push(l),
                _ => groups.push(vec![l]),
            }
        }
        symmetric = groups.into_iter().rev().flatten().collect();
    }
    symmetric.extend([
        S::HoleFills,
        S::Board(Layer::BOARD_PLATED_CUTOUTS),
        S::Holes,
        S::Board(Layer::BOARD_CUTOUTS),
        S::Board(Layer::BOARD_OUTLINES),
        S::AirWires,
        S::Board(Layer::BOARD_SHEET_FRAMES),
        S::Board(Layer::BOARD_MEASURES),
        S::Board(Layer::BOARD_ALIGNMENT),
        S::Board(Layer::BOARD_DOCUMENTATION),
        S::Board(Layer::BOARD_COMMENTS),
        S::Board(Layer::BOARD_GUIDE),
    ]);
    symmetric
}

struct Builder<'a> {
    project: &'a Project,
    board: &'a Board,
    scheme: &'a ColorScheme,
    ranks: &'a HashMap<LayerId, i32>,
    copper_layers: &'a BTreeSet<Layer>,
    font: Option<&'a StrokeFont>,
    /// Positions of the footprint pads of the built devices (device, pad).
    pad_positions: HashMap<(Uuid, Uuid), Point>,
    /// Items of the unit being built.
    out: Built<BoardObject>,
    /// Devices whose pads the trace being built ends at.
    devices: Vec<ComponentInstanceId>,
    /// Library elements the unit being built depends on.
    library: Vec<Uuid>,
    /// Stroke texts of the device being built.
    texts: Vec<Uuid>,
}

impl Builder<'_> {
    /// Builds the items of a unit; `false` if it does not exist.
    fn build(&mut self, unit: Unit) -> bool {
        let board = self.board;
        match unit {
            Unit::Device(c) => {
                let Some(device) = board.device(c) else {
                    return false;
                };
                if let Err(e) = self.add_device(device) {
                    self.out
                        .warnings
                        .push(format!("Device {}: {e}", device.component().0));
                }
            }
            Unit::Pad(s, uuid) => {
                let Some(pad) = board.net_segment(s).and_then(|s| s.pads().get(&uuid)) else {
                    return false;
                };
                let p = pad.pad();
                let xf = convert::transform(&Transform::new(p.position(), p.rotation(), false));
                let connected = board.anchor_trace_layers(TraceAnchor::Pad(uuid));
                let geometries =
                    pad.geometries(self.copper_layers, board.design_rules(), &connected);
                self.pad_geometries(&geometries, p.holes(), xf, BoardObject::Pad(s, uuid));
            }
            Unit::Via(s, uuid) => {
                let Some(segment) = board.net_segment(s) else {
                    return false;
                };
                let Some(via) = segment.vias().get(&uuid) else {
                    return false;
                };
                self.add_via(segment, uuid, via);
            }
            Unit::Trace(s, uuid) => {
                let Some(segment) = board.net_segment(s) else {
                    return false;
                };
                if !segment.traces().contains_key(&uuid) {
                    return false;
                }
                self.add_trace(segment, uuid);
            }
            Unit::Plane(id) => {
                if board.plane(id).is_none() {
                    return false;
                }
                self.add_plane(id);
            }
            Unit::Zone(uuid) => {
                let Some(zone) = board.zones().get(&uuid) else {
                    return false;
                };
                self.zone(
                    zone.layers().iter().copied(),
                    zone.outline(),
                    Affine::IDENTITY,
                    BoardObject::Zone(uuid),
                );
            }
            Unit::Polygon(uuid) => {
                let Some(polygon) = board.polygons().get(&uuid) else {
                    return false;
                };
                let fill =
                    self.polygon_fill(polygon.layer(), polygon.is_filled(), polygon.is_grab_area());
                let item = shapes::polygon(
                    LayerId::default(),
                    polygon.path(),
                    Affine::IDENTITY,
                    *polygon.line_width(),
                    fill,
                );
                self.insert(
                    BoardSceneLayer::Board(polygon.layer()),
                    sub::LINES,
                    item,
                    BoardObject::Polygon(uuid),
                );
            }
            Unit::Text(uuid) => {
                let Some(text) = board.stroke_texts().get(&uuid) else {
                    return false;
                };
                let value = attributes::substitute_board_text(self.project, board, text.text());
                self.stroke_text(text, &value, BoardObject::StrokeText(uuid));
            }
            Unit::Hole(uuid) => {
                let Some(hole) = board.holes().get(&uuid) else {
                    return false;
                };
                self.hole(
                    hole.diameter(),
                    hole.path().get(),
                    hole.stop_mask_offset(board.design_rules()),
                    BoardObject::Hole(uuid),
                );
            }
            Unit::AirWires(net) => {
                let Some(wires) = board.derived().air_wires().get(&net) else {
                    return false;
                };
                for wire in wires {
                    let item = Item::new(
                        LayerId::default(),
                        Line::new(
                            convert::point(wire.p1_position()),
                            convert::point(wire.p2_position()),
                        ),
                        Style::stroke(0.0),
                    );
                    self.insert(
                        BoardSceneLayer::AirWires,
                        sub::LINES,
                        Some(item),
                        BoardObject::AirWire(wire.net()),
                    );
                }
            }
        }
        true
    }

    /// Adds an item on `layer` (skipped if the layer is not drawn, e.g.
    /// hidden grab areas or disabled inner layers).
    fn insert(
        &mut self,
        layer: BoardSceneLayer,
        sub: i32,
        item: Option<Item>,
        object: BoardObject,
    ) {
        let id = layer.id();
        let Some(rank) = self.ranks.get(&id).copied() else {
            return;
        };
        if let Some(mut item) = item
            && !item.geometry.is_empty()
        {
            item.layer = id;
            item.z = rank * 4 + sub;
            self.out.items.push((item, object));
        }
    }

    fn grab_area_fill(&self, layer: Layer) -> Fill {
        // Upstream `ColorRole::getGrabAreaRole()`.
        let role = if layer == Layer::TOP_LEGEND {
            "board_grab_areas_top"
        } else if layer == Layer::BOT_LEGEND {
            "board_grab_areas_bottom"
        } else {
            return Fill::None;
        };
        self.scheme.color(role).map_or(Fill::None, Fill::Solid)
    }

    fn polygon_fill(&self, layer: Layer, filled: bool, grab_area: bool) -> Fill {
        if filled {
            Fill::Layer
        } else if grab_area {
            self.grab_area_fill(layer)
        } else {
            Fill::None
        }
    }

    fn stroke_text(&mut self, data: &BoardStrokeTextData, text: &str, object: BoardObject) {
        let Some(font) = self.font else {
            return;
        };
        let paths = StrokeTextPathBuilder::build(
            font,
            data.letter_spacing(),
            data.line_spacing(),
            data.height(),
            data.stroke_width(),
            data.align(),
            data.rotation(),
            data.auto_rotate(),
            text,
        );
        let xf = convert::transform(&Transform::new(
            data.position(),
            data.rotation(),
            data.mirrored(),
        ));
        let item = Item::new(
            LayerId::default(),
            xf * convert::paths(&paths),
            Style::stroke(shapes::pen(*data.stroke_width())),
        );
        self.insert(
            BoardSceneLayer::Board(data.layer()),
            sub::TEXTS,
            Some(item),
            object,
        );
    }

    /// A non-plated hole: outline, drill fill and stop mask openings.
    fn hole(
        &mut self,
        diameter: PositiveLength,
        path: &Path,
        stop_mask: Option<Length>,
        object: BoardObject,
    ) {
        let strokes = path.to_outline_strokes(diameter);
        let outline = shapes::outline(LayerId::default(), &strokes, Affine::IDENTITY);
        self.insert(BoardSceneLayer::Holes, sub::LINES, outline, object);
        let fill = shapes::area(LayerId::default(), &strokes, Affine::IDENTITY);
        self.insert(BoardSceneLayer::HoleFills, sub::AREAS, fill, object);
        if let Some(offset) = stop_mask
            && let Ok(path) = librepcb_core::geometry::NonEmptyPath::new(path.clone())
        {
            let geometry =
                PadGeometry::stroke(diameter, path, PadHoleList::new()).with_offset(offset);
            if let Ok(outlines) = geometry.to_outlines() {
                for layer in [Layer::TOP_STOP_MASK, Layer::BOT_STOP_MASK] {
                    let area = shapes::area(LayerId::default(), &outlines, Affine::IDENTITY);
                    self.insert(BoardSceneLayer::Board(layer), sub::AREAS, area, object);
                }
            }
        }
    }

    fn zone(
        &mut self,
        layers: impl IntoIterator<Item = Layer>,
        outline: &Path,
        xf: Affine,
        object: BoardObject,
    ) {
        let mut path = outline.clone();
        path.close();
        for layer in layers {
            if !self.copper_layers.contains(&layer) {
                continue;
            }
            let item = shapes::polygon(LayerId::default(), &path, xf, Length::ZERO, Fill::Layer)
                .map(|mut i| {
                    i.style.stroke = Some(librepcb_canvas::StrokeStyle::hairline());
                    i
                });
            self.insert(BoardSceneLayer::Zones(layer), sub::AREAS, item, object);
        }
    }

    fn resolve_footprint<'b>(&self, device: &BoardDevice) -> Result<&'b Footprint>
    where
        Self: 'b,
    {
        use librepcb_core::project::Error as PErr;
        let lib = self.project.library();
        let lib_device = lib
            .device(&device.lib_device())
            .ok_or(PErr::MissingLibraryDevice(device.lib_device()))?;
        let package = lib
            .package(&lib_device.package_uuid())
            .ok_or(PErr::MissingLibraryPackage(lib_device.package_uuid()))?;
        let footprint = package
            .footprints()
            .by_uuid(&device.lib_footprint())
            .ok_or(PErr::MissingLibraryPackage(device.lib_footprint()))?;
        Ok(footprint)
    }

    fn add_device(&mut self, device: &BoardDevice) -> Result<()> {
        let project = self.project;
        let board = self.board;
        // Dependencies for incremental updates.
        self.texts.extend(device.stroke_texts().keys().copied());
        self.library.push(device.lib_device());
        if let Some(dev) = project.library().device(&device.lib_device()) {
            self.library.push(dev.package_uuid());
        }
        if let Some(c) = project.circuit().component_instance(device.component()) {
            self.library.push(c.lib_component());
        }
        let footprint = self.resolve_footprint(device)?;
        let transform = device.transform();
        let xf = convert::transform(&transform);
        let object = BoardObject::Device(device.component());
        let rules = board.design_rules();

        for polygon in footprint.polygons().iter() {
            let layer = transform.map(&polygon.layer());
            let fill = self.polygon_fill(layer, polygon.is_filled(), polygon.is_grab_area());
            let item = shapes::polygon(
                LayerId::default(),
                polygon.path(),
                xf,
                *polygon.line_width(),
                fill,
            );
            self.insert(BoardSceneLayer::Board(layer), sub::LINES, item, object);
        }
        for circle in footprint.circles().iter() {
            let layer = transform.map(&circle.layer());
            let fill = self.polygon_fill(layer, circle.is_filled(), circle.is_grab_area());
            let item = shapes::circle(
                LayerId::default(),
                xf * convert::point(circle.center()),
                *circle.diameter(),
                *circle.line_width(),
                fill,
            );
            self.insert(
                BoardSceneLayer::Board(layer),
                sub::LINES,
                Some(item),
                object,
            );
        }
        let stop_masks = BoardDevice::hole_stop_mask_offsets(footprint, rules);
        for hole in footprint.holes().iter() {
            let hole: &Hole = hole;
            let path = transform.map(hole.path().get());
            let offset = stop_masks.get(&hole.uuid()).copied().flatten();
            self.hole(hole.diameter(), &path, offset, object);
        }
        for zone in footprint.zones().iter() {
            let flags = zone.layers();
            let (top, bottom) = if transform.mirrored {
                (ZoneLayers::BOTTOM, ZoneLayers::TOP)
            } else {
                (ZoneLayers::TOP, ZoneLayers::BOTTOM)
            };
            let mut layers = Vec::new();
            if flags.contains(top) {
                layers.push(Layer::TOP_COPPER);
            }
            if flags.contains(ZoneLayers::INNER) {
                layers.extend(self.copper_layers.iter().filter(|l| l.is_inner()));
            }
            if flags.contains(bottom) {
                layers.push(Layer::BOT_COPPER);
            }
            self.zone(layers, zone.outline(), xf, object);
        }

        // Pads.
        let pads = device.pads(project.library(), project.circuit())?;
        for pad in &pads {
            let object = BoardObject::FootprintPad(device.component(), pad.uuid());
            self.pad_positions
                .insert((device.component().0, pad.uuid()), pad.position());
            let pad_xf = convert::transform(&Transform::new(
                pad.position(),
                pad.rotation(),
                pad.mirrored(),
            ));
            let connected = board.anchor_trace_layers(pad.trace_anchor());
            let holes = pad.properties().holes();
            let geometries = pad.geometries(self.copper_layers, rules, &connected);
            self.pad_geometries(&geometries, holes, pad_xf, object);
        }

        // Texts.
        for text in device.stroke_texts().values() {
            let value = attributes::substitute_device_text(project, board, device, text.text());
            self.stroke_text(text, &value, BoardObject::StrokeText(text.uuid()));
        }
        Ok(())
    }

    fn pad_geometries(
        &mut self,
        geometries: &std::collections::BTreeMap<Layer, Vec<PadGeometry>>,
        holes: &PadHoleList,
        xf: Affine,
        object: BoardObject,
    ) {
        for (layer, geometries) in geometries {
            let scene_layer = if !holes.is_empty() && layer.is_copper() {
                BoardSceneLayer::ThtPads(*layer)
            } else {
                BoardSceneLayer::Board(*layer)
            };
            for geometry in geometries {
                match geometry.to_outlines() {
                    Ok(outlines) => {
                        // Copper geometries have the pad holes cut out.
                        let hole_outlines: Vec<_> = if layer.is_copper() {
                            holes
                                .iter()
                                .flat_map(|h| h.path().get().to_outline_strokes(h.diameter()))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        let item = shapes::area_with_holes(
                            LayerId::default(),
                            &outlines,
                            &hole_outlines,
                            xf,
                        );
                        self.insert(scene_layer, sub::PADS, item, object);
                    }
                    Err(e) => self.out.warnings.push(format!("Pad geometry: {e}")),
                }
            }
        }
        for hole in holes.iter() {
            let strokes = hole.path().get().to_outline_strokes(hole.diameter());
            let fill = shapes::area(LayerId::default(), &strokes, xf);
            self.insert(BoardSceneLayer::HoleFills, sub::AREAS, fill, object);
        }
    }

    /// The position of a trace anchor; records the devices the trace
    /// being built ends at.
    fn trace_anchor_position(
        &mut self,
        segment: &BoardNetSegment,
        anchor: TraceAnchor,
    ) -> Option<Point> {
        match anchor {
            TraceAnchor::Junction(uuid) => segment.junctions().get(&uuid).map(|j| j.position()),
            TraceAnchor::Via(uuid) => segment.vias().get(&uuid).map(|v| v.position()),
            TraceAnchor::Pad(uuid) => segment.pads().get(&uuid).map(|p| p.pad().position()),
            TraceAnchor::FootprintPad { device, pad } => {
                self.devices.push(ComponentInstanceId(device));
                if let Some(p) = self.pad_positions.get(&(device, pad)) {
                    return Some(*p);
                }
                self.board.anchor_position(
                    segment,
                    anchor,
                    self.project.library(),
                    self.project.circuit(),
                )
            }
        }
    }

    fn add_via(&mut self, segment: &BoardNetSegment, uuid: Uuid, via: &Via) {
        let rules = self.board.design_rules();
        let object = BoardObject::Via(segment.id(), uuid);
        let (drill, size) = self.via_drill_and_size(segment, via);
        let center = convert::point(via.position());
        for layer in self.copper_layers {
            if Via::is_on_layer_between(*layer, via.start_layer(), via.end_layer()) {
                let item = shapes::ring(LayerId::default(), center, size.to_mm(), drill.to_mm());
                self.insert(BoardSceneLayer::Vias(*layer), sub::PADS, Some(item), object);
            }
        }
        let fill = Item::new(
            LayerId::default(),
            KCircle::new(center, drill.to_mm() / 2.0),
            Style::fill(),
        );
        self.insert(BoardSceneLayer::HoleFills, sub::AREAS, Some(fill), object);
        // Stop mask openings (upstream `BI_Via::updateStopMaskDiameters()`).
        let config = via.exposure_config();
        let dia = if let Some(offset) = config.offset() {
            *size + offset * 2
        } else if config.is_enabled() {
            *size + *rules.stop_mask_clearance().calc_value(*size) * 2
        } else if rules.does_via_require_stop_mask_opening(*drill) {
            *drill + *rules.stop_mask_clearance().calc_value(*drill) * 2
        } else {
            Length::ZERO
        };
        if dia > Length::ZERO {
            let circle = KCircle::new(center, dia.to_mm() / 2.0);
            if via.start_layer().is_top() {
                let item = Item::new(LayerId::default(), circle, Style::fill());
                self.insert(
                    BoardSceneLayer::Board(Layer::TOP_STOP_MASK),
                    sub::AREAS,
                    Some(item),
                    object,
                );
            }
            if via.end_layer().is_bottom() {
                let item = Item::new(LayerId::default(), circle, Style::fill());
                self.insert(
                    BoardSceneLayer::Board(Layer::BOT_STOP_MASK),
                    sub::AREAS,
                    Some(item),
                    object,
                );
            }
        }
    }

    fn add_trace(&mut self, segment: &BoardNetSegment, uuid: Uuid) {
        let Some(trace) = segment.traces().get(&uuid) else {
            return;
        };
        match (
            self.trace_anchor_position(segment, trace.p1()),
            self.trace_anchor_position(segment, trace.p2()),
        ) {
            (Some(p1), Some(p2)) => {
                let item = shapes::line(LayerId::default(), p1, p2, *trace.width());
                self.insert(
                    BoardSceneLayer::Board(trace.layer()),
                    sub::LINES,
                    Some(item),
                    BoardObject::Trace(segment.id(), uuid),
                );
            }
            _ => self
                .out
                .warnings
                .push(format!("Trace {uuid}: unresolved anchor")),
        }
    }

    /// Upstream `BI_Via::updateActualDrillAndSize()`.
    fn via_drill_and_size(
        &self,
        segment: &BoardNetSegment,
        via: &Via,
    ) -> (PositiveLength, PositiveLength) {
        let rules = self.board.design_rules();
        let circuit = self.project.circuit();
        let drill = via.drill_diameter().unwrap_or_else(|| {
            segment
                .net()
                .and_then(|n| circuit.net_signal(n))
                .and_then(|n| circuit.net_class(n.net_class()))
                .and_then(|c| c.default_via_drill())
                .unwrap_or(rules.default_via_drill_diameter())
        });
        let size = match via.size() {
            Some(size) => size.max(drill),
            None => Via::calc_size_from_rules(drill, &rules.via_annular_ring()),
        };
        (drill, size)
    }

    fn add_plane(&mut self, id: PlaneId) {
        let board = self.board;
        let Some(plane) = board.plane(id) else {
            return;
        };
        let object = BoardObject::Plane(id);
        let layer = BoardSceneLayer::Board(plane.layer());
        let fragments = board.derived().fragments_of(id);
        if plane.visible() && !fragments.is_empty() {
            let item = shapes::area(LayerId::default(), fragments, Affine::IDENTITY);
            self.insert(layer, sub::AREAS, item, object);
        }
        let mut outline = plane.outline().clone();
        outline.close();
        let item = shapes::outline(LayerId::default(), &[outline], Affine::IDENTITY);
        self.insert(layer, sub::AREAS, item, object);
    }
}
