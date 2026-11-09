//! Scene builder of schematic pages: port of the drawing semantics of
//! libs/librepcb/core/project/schematic/schematicpainter.{h,cpp} (which the
//! upstream graphics export uses) and of the schematic graphics items of
//! libs/librepcb/editor/project/schematic/graphicsitems/.
//!
//! Every color role of the upstream graphics export is a canvas layer
//! ([`SchematicScene::layer_id()`]); items are ordered like the painter
//! draws them (grab areas, filled shapes, outlines and pins of symbols,
//! then polygons, texts, net lines, junctions, net labels and the bus
//! items). Differences to upstream:
//!
//! - Texts are drawn with the project's stroke font (see [`crate::text`]).
//! - Images are not drawn yet, only their borders.
//! - Pins with a forced net name conflict (`hasError()`, not ported in core)
//!   are drawn with their full length.
//!
//! The scene is built per unit (a symbol, a net or bus segment, a page
//! polygon, text or image, see [`crate::units`]);
//! [`SchematicScene::apply_changes()`] rebuilds only the units the changes
//! of the project's change journal affect, with the same code as the full
//! build (upstream's graphics items update themselves through signals).
//! Junction dots at symbol pins belong to the net segment of the pin (they
//! depend on its lines), not to the symbol.

use std::collections::{BTreeSet, HashMap, HashSet};

use librepcb_canvas::kurbo::{Affine, Circle as KCircle, Rect, Shape};
use librepcb_canvas::peniko::Color;
use librepcb_canvas::{Item, ItemId, Layer as CanvasLayer, LayerId, Scene, Style, convert};
use librepcb_core::geometry::{Image, NetLineAnchor, Polygon, Text};
use librepcb_core::library::sym::SymbolPin;
use librepcb_core::project::schematic::{Schematic, SchematicSymbol, SymbolPinView};
use librepcb_core::project::{
    BoardChange, BusId, BusSegmentId, Change, ChangesSince, ComponentInstanceId, NetSegmentId,
    NetSignalId, Project, SchematicChange, SchematicId, SymbolId,
};
use librepcb_core::types::{Length, Point, Uuid};
use librepcb_core::utils::toolbox;

use crate::attributes;
use crate::colors::ColorScheme;
use crate::error::{Error, Result};
use crate::shapes::{self, Fill};
use crate::text::{RenderedText, TextRenderer, TextStyle};
use crate::units::{Built, DepIndex, Units};

/// Color roles of the schematic in upstream's graphics export order
/// (`GraphicsExportSettings::loadColorsFromScheme()`); painted in reverse
/// order, so the first role is on top.
pub(crate) const ROLES: &[&str] = &[
    "schematic_frames",
    "schematic_outlines",
    "schematic_grab_areas",
    "schematic_pin_lines",
    "schematic_pin_names",
    "schematic_pin_numbers",
    "schematic_names",
    "schematic_values",
    "schematic_wires",
    "schematic_net_labels",
    "schematic_buses",
    "schematic_bus_labels",
    "schematic_image_borders",
    "schematic_documentation",
    "schematic_comments",
    "schematic_guide",
];

/// Draw order of the item kinds (upstream `SchematicPainter::paint()`).
mod z {
    pub const GRAB_AREAS: i32 = 0;
    pub const FILLED: i32 = 1;
    pub const SYMBOL_IMAGES: i32 = 2;
    pub const SHAPES: i32 = 3;
    pub const PINS: i32 = 4;
    pub const PIN_TEXTS: i32 = 5;
    pub const IMAGES: i32 = 6;
    pub const POLYGONS: i32 = 7;
    pub const TEXTS: i32 = 8;
    pub const NET_LINES: i32 = 9;
    pub const NET_JUNCTIONS: i32 = 10;
    pub const NET_LABELS: i32 = 11;
    pub const BUS_LINES: i32 = 12;
    pub const BUS_JUNCTIONS: i32 = 13;
    pub const BUS_LABELS: i32 = 14;
}

/// Radius of junction dots (upstream `drawNetJunction()`).
const JUNCTION_RADIUS: f64 = 0.6;
/// Line width of symbol pins (upstream `drawSymbolPin()`).
const PIN_LINE_WIDTH: Length = Length::new(158_750);

/// The model object an item of a [`SchematicScene`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchematicObject {
    /// A symbol (its shapes and texts).
    Symbol(SymbolId),
    /// A pin of a symbol (library pin UUID).
    SymbolPin(SymbolId, Uuid),
    /// A text of a symbol (text UUID); it belongs to the unit of its
    /// symbol.
    SymbolText(SymbolId, Uuid),
    /// A net line of a net segment.
    NetLine(NetSegmentId, Uuid),
    /// A junction dot (net segment junction or symbol pin).
    NetJunction(NetSegmentId, NetLineAnchor),
    /// A net label of a net segment.
    NetLabel(NetSegmentId, Uuid),
    /// A line of a bus segment.
    BusLine(BusSegmentId, Uuid),
    /// A junction of a bus segment.
    BusJunction(BusSegmentId, Uuid),
    /// A label of a bus segment.
    BusLabel(BusSegmentId, Uuid),
    /// A polygon of the schematic.
    Polygon(Uuid),
    /// A text of the schematic.
    Text(Uuid),
    /// An image of the schematic.
    Image(Uuid),
}

/// A unit of a schematic scene: a model object whose items are built
/// together (see [`crate::units`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Unit {
    /// A symbol with its pins and texts.
    Symbol(SymbolId),
    /// A net segment: lines, junction dots (including those at symbol
    /// pins) and labels.
    NetSegment(NetSegmentId),
    /// A bus segment: lines, junctions and labels.
    BusSegment(BusSegmentId),
    /// A polygon of the page.
    Polygon(Uuid),
    /// A text of the page.
    Text(Uuid),
    /// An image of the page.
    Image(Uuid),
}

/// A schematic page as a canvas [`Scene`].
#[derive(Debug)]
pub struct SchematicScene {
    schematic: SchematicId,
    scene: Scene,
    scheme: ColorScheme,
    units: Units<Unit, SchematicObject>,
    /// Net segments depending on symbols (lines at pins) and on bus
    /// segments (lines at bus junctions).
    anchors: DepIndex<Unit, Unit>,
    /// Library elements the symbols depend on.
    library: DepIndex<Unit, Uuid>,
    warnings: Vec<String>,
}

static_assertions::assert_impl_all!(SchematicScene: Send, Sync);

/// What to rebuild, collected from [`Change`]s.
#[derive(Debug, Default)]
struct Dirty {
    units: HashSet<Unit>,
    /// Components whose symbols are rebuilt.
    components: BTreeSet<ComponentInstanceId>,
    /// Nets whose segments and pins (names) are rebuilt.
    nets: BTreeSet<NetSignalId>,
    /// Buses whose segments are rebuilt.
    buses: BTreeSet<BusId>,
    /// Library elements whose dependents are rebuilt.
    library: BTreeSet<Uuid>,
}

impl SchematicScene {
    /// Builds the scene of a schematic page with a color scheme (e.g.
    /// [`ColorScheme::SCHEMATIC_LIGHT`]). Items which cannot be resolved
    /// (e.g. symbols missing in the project library) are skipped and
    /// reported by [`warnings()`](Self::warnings).
    pub fn build(project: &Project, schematic: SchematicId, scheme: &ColorScheme) -> Result<Self> {
        let sch = project
            .schematic(schematic)
            .ok_or(Error::SchematicNotFound(schematic))?;
        let mut scene = Self {
            schematic,
            scene: Scene::new(),
            scheme: scheme.clone(),
            units: Units::new(),
            anchors: DepIndex::new(),
            library: DepIndex::new(),
            warnings: Vec::new(),
        };
        scene.add_layers();
        let units = sch
            .symbols()
            .keys()
            .map(|id| Unit::Symbol(*id))
            .chain(sch.images().keys().map(|id| Unit::Image(*id)))
            .chain(sch.polygons().keys().map(|id| Unit::Polygon(*id)))
            .chain(sch.texts().keys().map(|id| Unit::Text(*id)))
            .chain(sch.net_segments().keys().map(|id| Unit::NetSegment(*id)))
            .chain(sch.bus_segments().keys().map(|id| Unit::BusSegment(*id)))
            .collect();
        scene.rebuild_units(project, sch, units);
        Ok(scene)
    }

    /// The schematic page of the scene.
    pub fn schematic(&self) -> SchematicId {
        self.schematic
    }

    /// The canvas layer of a color role (e.g. `"schematic_wires"`), `None`
    /// if the role is not drawn.
    pub fn layer_id(role: &str) -> Option<LayerId> {
        ROLES
            .iter()
            .position(|r| *r == role)
            .map(|i| LayerId(i as u16))
    }

    /// The scene.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// The scene for modification (e.g. layer visibility, selection).
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// The model object of an item.
    pub fn object(&self, item: ItemId) -> Option<SchematicObject> {
        self.units.object(item)
    }

    /// The background color of the color scheme.
    pub fn background(&self) -> Color {
        self.scheme.color_or_transparent("schematic_background")
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
    /// whole page (project metadata and settings, pages and boards added,
    /// removed or renamed, unknown changes) rebuild the scene from scratch
    /// (see [`rebuild()`](Self::rebuild)); returns whether that happened.
    ///
    /// Fails if the schematic no longer exists.
    pub fn apply_changes(&mut self, project: &Project, changes: ChangesSince<'_>) -> Result<bool> {
        let sch = project
            .schematic(self.schematic)
            .ok_or(Error::SchematicNotFound(self.schematic))?;
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
        let units = self.resolve_dirty(project, sch, dirty);
        self.rebuild_units(project, sch, units);
        Ok(false)
    }

    /// Rebuilds the whole scene, keeping the layer visibility.
    pub fn rebuild(&mut self, project: &Project) -> Result<()> {
        let mut scene = Self::build(project, self.schematic, &self.scheme)?;
        for (id, layer) in self.scene.layers() {
            scene.scene.set_layer_visible(id, layer.visible);
        }
        *self = scene;
        Ok(())
    }

    fn add_layers(&mut self) {
        let count = ROLES.len() as i32;
        for (i, role) in ROLES.iter().enumerate() {
            let color = self.scheme.color_or_transparent(role);
            self.scene.set_layer(
                LayerId(i as u16),
                CanvasLayer::new(color).with_order(count - i as i32),
            );
        }
    }

    /// Records what a change affects; `false` if the whole scene must be
    /// rebuilt.
    fn collect(&self, change: &Change, dirty: &mut Dirty) -> bool {
        use SchematicChange as S;
        let units = &mut dirty.units;
        match change {
            // Texts may show project attributes, page names and numbers;
            // the primary device (first board with a device) of components
            // may change.
            Change::ProjectMetadata
            | Change::ProjectSettings
            | Change::SchematicAdded(_)
            | Change::SchematicRemoved(_)
            | Change::SchematicChanged(_)
            | Change::BoardAdded(_)
            | Change::BoardRemoved(_) => return false,
            Change::OutputJobs
            | Change::ErcApprovals
            | Change::AssemblyVariantAdded(_)
            | Change::AssemblyVariantRemoved(_)
            | Change::AssemblyVariantChanged(_)
            | Change::NetClassAdded(_)
            | Change::NetClassRemoved(_)
            | Change::NetClassChanged(_)
            | Change::NetSignalAdded(_)
            | Change::NetSignalRemoved(_)
            | Change::BusAdded(_)
            | Change::BusRemoved(_)
            | Change::ComponentAdded(_)
            | Change::ComponentRemoved(_)
            | Change::BoardChanged(_) => {}
            // Net labels and pin names showing the net name.
            Change::NetSignalChanged(net) => {
                dirty.nets.insert(*net);
            }
            Change::BusChanged(bus) => {
                dirty.buses.insert(*bus);
            }
            // Texts (attributes).
            Change::ComponentChanged(component) => {
                dirty.components.insert(*component);
            }
            // Pin names showing the net name.
            Change::ComponentSignalNetChanged { signal, .. } => {
                dirty.components.insert(signal.component);
            }
            Change::LibraryElementAdded { uuid, .. }
            | Change::LibraryElementRemoved { uuid, .. } => {
                dirty.library.insert(*uuid);
            }
            // Pad names and texts of the symbols (primary device).
            Change::Board { change, .. } => match change {
                BoardChange::DeviceAdded(c)
                | BoardChange::DeviceRemoved(c)
                | BoardChange::DeviceChanged(c) => {
                    dirty.components.insert(*c);
                }
                _ => {}
            },
            Change::Schematic { id, change } => {
                if *id != self.schematic {
                    return true;
                }
                match change {
                    S::SymbolAdded(id)
                    | S::SymbolRemoved(id)
                    | S::SymbolPlacementChanged(id)
                    | S::SymbolTextsChanged(id) => {
                        units.insert(Unit::Symbol(*id));
                    }
                    S::NetSegmentAdded(id)
                    | S::NetSegmentRemoved(id)
                    | S::NetSegmentNetChanged(id)
                    | S::NetSegmentElementsAdded { segment: id, .. }
                    | S::NetSegmentElementsRemoved { segment: id, .. }
                    | S::NetPointChanged { segment: id, .. }
                    | S::NetLineChanged { segment: id, .. }
                    | S::NetLabelAdded { segment: id, .. }
                    | S::NetLabelRemoved { segment: id, .. }
                    | S::NetLabelChanged { segment: id, .. } => {
                        units.insert(Unit::NetSegment(*id));
                    }
                    S::BusSegmentAdded(id)
                    | S::BusSegmentRemoved(id)
                    | S::BusSegmentBusChanged(id)
                    | S::BusSegmentElementsAdded { segment: id, .. }
                    | S::BusSegmentElementsRemoved { segment: id, .. }
                    | S::BusJunctionChanged { segment: id, .. }
                    | S::BusLineChanged { segment: id, .. }
                    | S::BusLabelAdded { segment: id, .. }
                    | S::BusLabelRemoved { segment: id, .. }
                    | S::BusLabelChanged { segment: id, .. } => {
                        units.insert(Unit::BusSegment(*id));
                    }
                    S::PolygonAdded(uuid) | S::PolygonRemoved(uuid) | S::PolygonChanged(uuid) => {
                        units.insert(Unit::Polygon(*uuid));
                    }
                    S::TextAdded(uuid) | S::TextRemoved(uuid) | S::TextChanged(uuid) => {
                        units.insert(Unit::Text(*uuid));
                    }
                    S::ImageAdded(uuid) | S::ImageRemoved(uuid) | S::ImageChanged(uuid) => {
                        units.insert(Unit::Image(*uuid));
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
        true
    }

    /// Resolves the collected changes to the units to rebuild, including
    /// the dependents of rebuilt units.
    fn resolve_dirty(&self, project: &Project, sch: &Schematic, dirty: Dirty) -> Vec<Unit> {
        let circuit = project.circuit();
        let mut units = dirty.units;
        if !dirty.components.is_empty() || !dirty.nets.is_empty() {
            for (id, symbol) in sch.symbols() {
                let component = symbol.component();
                let affected = dirty.components.contains(&component)
                    || (!dirty.nets.is_empty()
                        && circuit.component_instance(component).is_some_and(|c| {
                            c.signals()
                                .values()
                                .any(|s| s.net().is_some_and(|n| dirty.nets.contains(&n)))
                        }));
                if affected {
                    units.insert(Unit::Symbol(*id));
                }
            }
        }
        if !dirty.nets.is_empty() {
            units.extend(
                sch.net_segments()
                    .iter()
                    .filter(|(_, s)| dirty.nets.contains(&s.net()))
                    .map(|(id, _)| Unit::NetSegment(*id)),
            );
        }
        if !dirty.buses.is_empty() {
            units.extend(
                sch.bus_segments()
                    .iter()
                    .filter(|(_, s)| dirty.buses.contains(&s.bus()))
                    .map(|(id, _)| Unit::BusSegment(*id)),
            );
        }
        if !dirty.library.is_empty() {
            for uuid in &dirty.library {
                units.extend(self.library.dependents(uuid));
            }
            // Units which could not be resolved may resolve now.
            units.extend(self.units.with_warnings());
        }
        // Net segments at rebuilt symbols (pin positions) and bus segments
        // (junction positions).
        let dependents: Vec<Unit> = units
            .iter()
            .filter(|u| matches!(u, Unit::Symbol(_) | Unit::BusSegment(_)))
            .flat_map(|u| self.anchors.dependents(u))
            .collect();
        units.extend(dependents);
        units.into_iter().collect()
    }

    /// Builds (or removes, if gone from the model) the items of units and
    /// updates the scene.
    fn rebuild_units(&mut self, project: &Project, sch: &Schematic, mut units: Vec<Unit>) {
        if units.is_empty() {
            return;
        }
        // Symbols first: they cache the pin positions for the net lines.
        units.sort_by_key(|u| !matches!(u, Unit::Symbol(_)));
        let mut builder = Builder {
            project,
            schematic: sch,
            scheme: &self.scheme,
            text: TextRenderer::new(schematic_font(project)),
            pin_positions: HashMap::new(),
            pad_names: HashMap::new(),
            out: Built::default(),
            anchors: Vec::new(),
            library: Vec::new(),
        };
        let mut updates = Vec::with_capacity(units.len());
        for unit in units {
            let exists = builder.build(unit);
            let built = std::mem::take(&mut builder.out);
            let anchors = std::mem::take(&mut builder.anchors);
            let library = std::mem::take(&mut builder.library);
            if exists {
                self.anchors.set(unit, anchors);
                self.library.set(unit, library);
                updates.push((unit, Some(built)));
            } else {
                self.anchors.remove(unit);
                self.library.remove(unit);
                updates.push((unit, None));
            }
        }
        self.units.commit(&mut self.scene, updates);
        self.warnings = self.units.warnings();
    }
}

/// The stroke font used for the texts: the default board font (the
/// schematic has no stroke font setting), or any font of the project.
fn schematic_font(project: &Project) -> Option<&librepcb_core::font::StrokeFont> {
    let pool = project.stroke_fonts();
    pool.font(librepcb_core::project::board::DEFAULT_STROKE_FONT_NAME)
        .ok()
}

struct Builder<'a> {
    project: &'a Project,
    schematic: &'a Schematic,
    scheme: &'a ColorScheme,
    text: TextRenderer<'a>,
    /// Absolute positions of the connected pins of the built symbols.
    pin_positions: HashMap<(Uuid, Uuid), Point>,
    /// Pad names per component signal (component, signal).
    pad_names: HashMap<(Uuid, Uuid), Vec<String>>,
    /// Items of the unit being built.
    out: Built<SchematicObject>,
    /// Symbols and bus segments the unit being built depends on.
    anchors: Vec<Unit>,
    /// Library elements the unit being built depends on.
    library: Vec<Uuid>,
}

impl Builder<'_> {
    /// Builds the items of a unit; `false` if it does not exist.
    fn build(&mut self, unit: Unit) -> bool {
        let sch = self.schematic;
        match unit {
            Unit::Symbol(id) => {
                let Some(symbol) = sch.symbols().get(&id) else {
                    return false;
                };
                if let Err(e) = self.add_symbol(symbol) {
                    self.out
                        .warnings
                        .push(format!("Symbol {}: {e}", symbol.uuid()));
                }
            }
            Unit::NetSegment(id) => {
                if !sch.net_segments().contains_key(&id) {
                    return false;
                }
                self.add_net_segment(id);
            }
            Unit::BusSegment(id) => {
                if !sch.bus_segments().contains_key(&id) {
                    return false;
                }
                self.add_bus_segment(id);
            }
            Unit::Polygon(uuid) => {
                let Some(polygon) = sch.polygons().get(&uuid) else {
                    return false;
                };
                self.add_polygon(uuid, polygon);
            }
            Unit::Text(uuid) => {
                let Some(text) = sch.texts().get(&uuid) else {
                    return false;
                };
                self.add_text(uuid, text);
            }
            Unit::Image(uuid) => {
                let Some(image) = sch.images().get(&uuid) else {
                    return false;
                };
                self.add_image(uuid, image);
            }
        }
        true
    }

    fn insert(&mut self, item: Option<Item>, object: SchematicObject) {
        if let Some(item) = item
            && !item.geometry.is_empty()
        {
            self.out.items.push((item, object));
        }
    }

    fn insert_text(
        &mut self,
        layer: LayerId,
        z: i32,
        rendered: Option<RenderedText>,
        object: SchematicObject,
    ) {
        let item =
            rendered.map(|t| Item::new(layer, t.path, Style::stroke(t.stroke_width)).with_z(z));
        self.insert(item, object);
    }

    /// Canvas layer of a model layer, `None` if the role is not drawn.
    fn layer(&self, layer: librepcb_core::types::Layer) -> Option<LayerId> {
        SchematicScene::layer_id(layer.color_role())
    }

    fn grab_area_fill(&self, role_layer: librepcb_core::types::Layer) -> Fill {
        // Upstream `ColorRole::getGrabAreaRole()`.
        if role_layer.color_role() == "schematic_outlines" {
            self.scheme
                .color("schematic_grab_areas")
                .map_or(Fill::None, Fill::Solid)
        } else {
            Fill::None
        }
    }

    /// Records the library elements a symbol depends on (also those of
    /// its primary device, for the pad names).
    fn record_symbol_library(&mut self, symbol: &SchematicSymbol) {
        let project = self.project;
        let Some(component) = project.circuit().component_instance(symbol.component()) else {
            return;
        };
        self.library.push(component.lib_component());
        if let Some(gate) = project
            .library()
            .component(&component.lib_component())
            .and_then(|c| c.symbol_variants().by_uuid(&component.lib_variant()))
            .and_then(|v| v.symbol_items().by_uuid(&symbol.lib_gate()))
        {
            self.library.push(gate.symbol_uuid());
        }
        if let Some(device) = attributes::primary_device(project, component) {
            self.library.push(device.lib_device());
            if let Some(dev) = project.library().device(&device.lib_device()) {
                self.library.push(dev.package_uuid());
            }
        }
    }

    fn add_symbol(&mut self, symbol: &SchematicSymbol) -> Result<()> {
        self.record_symbol_library(symbol);
        let view = self.project.view();
        let resolved = symbol.resolve(view)?;
        let pins = symbol.pins(view)?;
        let lib = resolved.lib_symbol;
        let transform = symbol.transform();
        let xf = convert::transform(&transform);
        let object = SchematicObject::Symbol(symbol.id());

        // Shapes (upstream `drawShapes()` for grab areas, filled shapes and
        // the others).
        for polygon in lib.polygons().iter() {
            let Some(layer) = self.layer(polygon.layer()) else {
                continue;
            };
            let (z, fill) = if polygon.is_filled() {
                (z::FILLED, Fill::Layer)
            } else if polygon.is_grab_area() {
                (z::GRAB_AREAS, self.grab_area_fill(polygon.layer()))
            } else {
                (z::SHAPES, Fill::None)
            };
            let item = shapes::polygon(layer, polygon.path(), xf, *polygon.line_width(), fill)
                .map(|i| i.with_z(z));
            self.insert(item, object);
        }
        for circle in lib.circles().iter() {
            let Some(layer) = self.layer(circle.layer()) else {
                continue;
            };
            let (z, fill) = if circle.is_filled() {
                (z::FILLED, Fill::Layer)
            } else if circle.is_grab_area() {
                (z::GRAB_AREAS, self.grab_area_fill(circle.layer()))
            } else {
                (z::SHAPES, Fill::None)
            };
            let center = convert::point(transform.map(&circle.center()));
            let item = shapes::circle(
                layer,
                center,
                *circle.diameter(),
                *circle.line_width(),
                fill,
            )
            .with_z(z);
            self.insert(Some(item), object);
        }
        // Image borders (the images themselves are not drawn yet).
        if let Some(layer) = SchematicScene::layer_id("schematic_image_borders") {
            for image in lib.images().iter() {
                if let Some(border) = image.border_width() {
                    let rect = image_rect(image.width().to_mm(), image.height().to_mm());
                    let img_xf = xf
                        * Affine::translate(convert::point(image.position()).to_vec2())
                        * Affine::rotate(convert::angle(image.rotation()));
                    let item = Item::new(
                        layer,
                        img_xf * rect.to_path(1e-4),
                        Style::stroke(shapes::pen(*border)),
                    )
                    .with_z(z::SYMBOL_IMAGES);
                    self.insert(Some(item), object);
                }
            }
        }

        // Pins.
        for pin in &pins {
            self.pin_positions
                .insert((symbol.uuid(), pin.uuid()), pin.position());
            self.add_pin(symbol, &transform, pin);
        }

        // Texts (with substituted attributes).
        let texts: Vec<Text> = symbol.texts().values().cloned().collect();
        for text in texts {
            let Some(layer) = self.layer(text.layer()) else {
                continue;
            };
            let value = attributes::substitute_symbol_text(
                self.project,
                self.schematic,
                symbol,
                text.text(),
            );
            let rendered = self.text.text(
                &value,
                text.position(),
                TextStyle {
                    height: text.height().to_mm(),
                    align: text.align(),
                    rotation: text.rotation(),
                    auto_rotate: true,
                    mirror_in_place: false,
                    parse_overlines: false,
                },
            );
            let text_object = SchematicObject::SymbolText(symbol.id(), text.uuid());
            self.insert_text(layer, z::TEXTS, rendered, text_object);
        }
        Ok(())
    }

    fn add_pin(
        &mut self,
        symbol: &SchematicSymbol,
        transform: &librepcb_core::utils::transform::Transform,
        pin: &SymbolPinView<'_>,
    ) {
        let object = SchematicObject::SymbolPin(symbol.id(), pin.uuid());
        let lib_pin = pin.lib_pin();
        let rotation = pin.rotation();
        let start = pin.position();

        // Line.
        if let Some(layer) = SchematicScene::layer_id("schematic_pin_lines") {
            let end = start
                + Point::new(*lib_pin.length(), Length::ZERO).rotated(rotation, Point::ORIGIN);
            let item = shapes::line(layer, start, end, PIN_LINE_WIDTH).with_z(z::PINS);
            self.insert(Some(item), object);
        }

        // Name.
        if let Some(layer) = SchematicScene::layer_id("schematic_pin_names") {
            let mut align = lib_pin.name_alignment();
            if transform.mirrored {
                align = align.mirrored_v();
            }
            let pos = transform.map(
                &(lib_pin.position()
                    + lib_pin
                        .name_position()
                        .rotated(lib_pin.rotation(), Point::ORIGIN)),
            );
            let rendered = self.text.text(
                pin.name(),
                pos,
                TextStyle {
                    height: lib_pin.name_height().to_mm(),
                    align,
                    rotation: transform
                        .map_non_mirrorable(lib_pin.rotation() + lib_pin.name_rotation()),
                    auto_rotate: true,
                    mirror_in_place: false,
                    parse_overlines: true,
                },
            );
            self.insert_text(layer, z::PIN_TEXTS, rendered, object);
        }

        // Numbers.
        if let Some(layer) = SchematicScene::layer_id("schematic_pin_numbers") {
            let signal = pin.signal();
            let names = self.pad_names(signal.component.0, signal.signal).to_vec();
            let numbers = pin.numbers_text(&names);
            let pos = transform.map(
                &(lib_pin.position()
                    + pin
                        .numbers_position()
                        .rotated(lib_pin.rotation(), Point::ORIGIN)),
            );
            let rendered = self.text.text(
                &numbers,
                pos,
                TextStyle {
                    height: SymbolPin::numbers_height().to_mm(),
                    align: pin.numbers_alignment(),
                    rotation: transform.map_non_mirrorable(lib_pin.rotation()),
                    auto_rotate: true,
                    mirror_in_place: false,
                    parse_overlines: false,
                },
            );
            self.insert_text(layer, z::PIN_TEXTS, rendered, object);
        }
    }

    /// The sorted pad names of a component signal on the primary device
    /// (upstream `ComponentSignalInstance::getPadNames()`).
    fn pad_names(&mut self, component: Uuid, signal: Uuid) -> &[String] {
        if !self.pad_names.contains_key(&(component, signal)) {
            let project = self.project;
            let circuit = project.circuit();
            let mut per_signal: HashMap<Uuid, BTreeSet<String>> = HashMap::new();
            if let Some(cmp) = circuit.component_instance(ComponentInstanceId(component))
                && let Some(device) = attributes::primary_device(project, cmp)
                && let Ok(pads) = device.pads(project.library(), circuit)
            {
                for pad in pads {
                    if let (Some(sig), Some(pkg_pad)) = (pad.component_signal(), pad.package_pad())
                    {
                        per_signal
                            .entry(sig.signal)
                            .or_default()
                            .insert(pkg_pad.name().to_string());
                    }
                }
            }
            // Cache all signals of the component at once.
            let signals: Vec<Uuid> = circuit
                .component_instance(ComponentInstanceId(component))
                .map(|c| c.signals().keys().copied().collect())
                .unwrap_or_default();
            for s in signals.into_iter().chain([signal]) {
                let mut names: Vec<String> = per_signal
                    .get(&s)
                    .map(|n| n.iter().cloned().collect())
                    .unwrap_or_default();
                names.sort_by(|a, b| toolbox::compare_numeric(a, b));
                self.pad_names.insert((component, s), names);
            }
        }
        self.pad_names
            .get(&(component, signal))
            .map_or(&[], Vec::as_slice)
    }

    fn add_image(&mut self, uuid: Uuid, image: &Image) {
        // Image borders only.
        if let Some(layer) = SchematicScene::layer_id("schematic_image_borders")
            && let Some(border) = image.border_width()
        {
            let rect = image_rect(image.width().to_mm(), image.height().to_mm());
            let xf = Affine::translate(convert::point(image.position()).to_vec2())
                * Affine::rotate(convert::angle(image.rotation()));
            let item = Item::new(
                layer,
                xf * rect.to_path(1e-4),
                Style::stroke(shapes::pen(*border)),
            )
            .with_z(z::IMAGES);
            self.insert(Some(item), SchematicObject::Image(uuid));
        }
    }

    fn add_polygon(&mut self, uuid: Uuid, polygon: &Polygon) {
        let Some(layer) = self.layer(polygon.layer()) else {
            return;
        };
        let fill = if polygon.is_filled() {
            Fill::Layer
        } else if polygon.is_grab_area() {
            self.grab_area_fill(polygon.layer())
        } else {
            Fill::None
        };
        let item = shapes::polygon(
            layer,
            polygon.path(),
            Affine::IDENTITY,
            *polygon.line_width(),
            fill,
        )
        .map(|i| i.with_z(z::POLYGONS));
        self.insert(item, SchematicObject::Polygon(uuid));
    }

    fn add_text(&mut self, uuid: Uuid, text: &Text) {
        let Some(layer) = self.layer(text.layer()) else {
            return;
        };
        let value =
            attributes::substitute_schematic_text(self.project, self.schematic, text.text());
        let rendered = self.text.text(
            &value,
            text.position(),
            TextStyle {
                height: text.height().to_mm(),
                align: text.align(),
                rotation: text.rotation(),
                auto_rotate: true,
                mirror_in_place: false,
                parse_overlines: false,
            },
        );
        self.insert_text(layer, z::TEXTS, rendered, SchematicObject::Text(uuid));
    }

    /// The position of a net line anchor; records the symbols and bus
    /// segments the net segment being built depends on.
    fn anchor_position(&mut self, segment: NetSegmentId, anchor: NetLineAnchor) -> Option<Point> {
        match anchor {
            NetLineAnchor::Pin { symbol, pin } => {
                self.anchors.push(Unit::Symbol(SymbolId(symbol)));
                if let Some(p) = self.pin_positions.get(&(symbol, pin)) {
                    return Some(*p);
                }
            }
            NetLineAnchor::BusJunction { segment, .. } => {
                self.anchors.push(Unit::BusSegment(BusSegmentId(segment)));
            }
            NetLineAnchor::Junction(_) => {}
        }
        self.schematic
            .net_line_anchor_position(segment, anchor, self.project.view())
    }

    fn add_net_segment(&mut self, id: NetSegmentId) {
        let project = self.project;
        let Some(segment) = self.schematic.net_segments().get(&id) else {
            return;
        };
        if let Some(layer) = SchematicScene::layer_id("schematic_wires") {
            // Pins with more than one line get a junction dot.
            let mut pins = BTreeSet::new();
            for (uuid, line) in segment.lines() {
                for anchor in [line.p1(), line.p2()] {
                    if matches!(anchor, NetLineAnchor::Pin { .. }) {
                        pins.insert(anchor);
                    }
                }
                match (
                    self.anchor_position(id, line.p1()),
                    self.anchor_position(id, line.p2()),
                ) {
                    (Some(p1), Some(p2)) => {
                        let item = shapes::line(layer, p1, p2, *line.width()).with_z(z::NET_LINES);
                        self.insert(Some(item), SchematicObject::NetLine(id, *uuid));
                    }
                    _ => self
                        .out
                        .warnings
                        .push(format!("Net line {uuid}: unresolved anchor")),
                }
            }
            for anchor in pins {
                if segment.is_visible_junction(anchor)
                    && let Some(position) = self.anchor_position(id, anchor)
                {
                    let dot = junction(layer, position, z::NET_JUNCTIONS);
                    self.insert(Some(dot), SchematicObject::NetJunction(id, anchor));
                }
            }
            for (uuid, junction_obj) in segment.junctions() {
                let anchor = NetLineAnchor::Junction(*uuid);
                if segment.is_visible_junction(anchor) {
                    let dot = junction(layer, junction_obj.position(), z::NET_JUNCTIONS);
                    self.insert(Some(dot), SchematicObject::NetJunction(id, anchor));
                }
            }
        }
        if let Some(layer) = SchematicScene::layer_id("schematic_net_labels") {
            let name = project
                .circuit()
                .net_signal(segment.net())
                .map(|n| n.name().as_str().to_owned())
                .unwrap_or_default();
            for (uuid, label) in segment.labels() {
                let rendered = self.text.net_label(
                    &name,
                    label.position(),
                    label.rotation(),
                    label.mirrored(),
                );
                self.insert_text(
                    layer,
                    z::NET_LABELS,
                    rendered,
                    SchematicObject::NetLabel(id, *uuid),
                );
            }
        }
    }

    fn add_bus_segment(&mut self, id: BusSegmentId) {
        let project = self.project;
        let Some(segment) = self.schematic.bus_segments().get(&id) else {
            return;
        };
        if let Some(layer) = SchematicScene::layer_id("schematic_buses") {
            for (uuid, line) in segment.lines() {
                match (
                    segment.junction_position(line.p1()),
                    segment.junction_position(line.p2()),
                ) {
                    (Some(p1), Some(p2)) => {
                        let item = shapes::line(layer, p1, p2, *line.width()).with_z(z::BUS_LINES);
                        self.insert(Some(item), SchematicObject::BusLine(id, *uuid));
                    }
                    _ => self
                        .out
                        .warnings
                        .push(format!("Bus line {uuid}: unresolved anchor")),
                }
            }
            for (uuid, j) in segment.junctions() {
                if segment.is_visible_junction(*uuid) {
                    let dot = junction(layer, j.position(), z::BUS_JUNCTIONS);
                    self.insert(Some(dot), SchematicObject::BusJunction(id, *uuid));
                }
            }
        }
        if let Some(layer) = SchematicScene::layer_id("schematic_bus_labels") {
            let name = project
                .circuit()
                .bus(segment.bus())
                .map(|b| b.name().as_str().to_owned())
                .unwrap_or_default();
            for (uuid, label) in segment.labels() {
                let rendered = self.text.net_label(
                    &name,
                    label.position(),
                    label.rotation(),
                    label.mirrored(),
                );
                self.insert_text(
                    layer,
                    z::BUS_LABELS,
                    rendered,
                    SchematicObject::BusLabel(id, *uuid),
                );
            }
        }
    }
}

/// A filled junction dot (upstream `drawNetJunction()`).
fn junction(layer: LayerId, position: Point, z: i32) -> Item {
    Item::new(
        layer,
        KCircle::new(convert::point(position), JUNCTION_RADIUS),
        Style::fill(),
    )
    .with_z(z)
}

/// The rectangle of an image in image coordinates (bottom left at the
/// origin, like upstream `drawImage()`).
fn image_rect(width: f64, height: f64) -> Rect {
    Rect::new(0.0, 0.0, width, height)
}
