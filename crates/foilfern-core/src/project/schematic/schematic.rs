//! Port of libs/librepcb/core/project/schematic/schematic.{h,cpp} (with the
//! trivial item wrappers `si_polygon`, `si_text` and `si_image`, whose
//! geometry objects are stored directly).
//!
//! Differences to upstream: no project back-pointer, no `isAddedToProject`
//! state, no own `TransactionalDirectory` (the project writes
//! `schematics/<directory_name>/` on save); the item `add*()`/`remove*()`
//! operations live in the project mutations and the Qt signals in the
//! change journal. The net lines registered at pins and bus junctions are
//! the non-persistent anchor index ([`SchematicDerived`]), rebuilt when the
//! schematic is added to a project. `updateAllLabelAnchors()` is replaced
//! by computing label anchors on demand
//! ([`net_label_anchor()`](Schematic::net_label_anchor)).

use std::collections::BTreeMap;

use super::anchor_index::{AnchorIndex, check_net_segment};
use super::{SchematicBusSegment, SchematicNetSegment, SchematicSymbol, uuid_map};
use crate::fileio::{self, FileSystem, TransactionalDirectory};
use crate::geometry::{Image, NetLineAnchor, Polygon, Text, property};
use crate::project::ProjectView;
use crate::project::error::Result;
use crate::project::id::{BusSegmentId, ComponentSignalRef, NetSegmentId, SchematicId, SymbolId};
use crate::project::ref_index::{NetUse, Reference};
use crate::serialization::{List, Mode, SExpression, SerializeObject};
use crate::types::{ElementName, Length, LengthUnit, Point, PositiveLength, Uuid};

/// A schematic page of a project.
///
/// Items are keyed by UUID (upstream `QMap`, file order).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Schematic {
    pub(crate) uuid: Uuid,
    /// Name of the directory below `schematics/`.
    pub(crate) directory_name: String,
    pub(crate) name: ElementName,
    pub(crate) grid_interval: PositiveLength,
    pub(crate) grid_unit: LengthUnit,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    pub(crate) symbols: BTreeMap<SymbolId, SchematicSymbol>,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    pub(crate) bus_segments: BTreeMap<BusSegmentId, SchematicBusSegment>,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    pub(crate) net_segments: BTreeMap<NetSegmentId, SchematicNetSegment>,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    pub(crate) polygons: BTreeMap<Uuid, Polygon>,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    pub(crate) texts: BTreeMap<Uuid, Text>,
    #[serde(deserialize_with = "uuid_map::deserialize")]
    pub(crate) images: BTreeMap<Uuid, Image>,
    /// Derived data, not persistent.
    #[serde(skip)]
    pub(crate) derived: SchematicDerived,
}

impl Schematic {
    /// Creates an empty schematic (default grid: 2.54 mm).
    pub fn new(uuid: Uuid, name: ElementName, directory_name: impl Into<String>) -> Self {
        Self {
            uuid,
            directory_name: directory_name.into(),
            name,
            grid_interval: PositiveLength::new(Length::new(2_540_000))
                .expect("constant is positive"),
            grid_unit: LengthUnit::Millimeters,
            symbols: BTreeMap::new(),
            bus_segments: BTreeMap::new(),
            net_segments: BTreeMap::new(),
            polygons: BTreeMap::new(),
            texts: BTreeMap::new(),
            images: BTreeMap::new(),
            derived: SchematicDerived::default(),
        }
    }

    /// Returns the UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Returns the typed identifier.
    pub fn id(&self) -> SchematicId {
        SchematicId(self.uuid)
    }

    /// Returns the name of the directory below `schematics/`.
    pub fn directory_name(&self) -> &str {
        &self.directory_name
    }

    property!(
        /// Returns the name.
        ref name: ElementName, set_name
    );
    property!(
        /// Returns the grid interval.
        copy grid_interval: PositiveLength, set_grid_interval
    );
    property!(
        /// Returns the grid unit.
        copy grid_unit: LengthUnit, set_grid_unit
    );

    /// Returns the symbols.
    pub fn symbols(&self) -> &BTreeMap<SymbolId, SchematicSymbol> {
        &self.symbols
    }

    /// Returns the bus segments.
    pub fn bus_segments(&self) -> &BTreeMap<BusSegmentId, SchematicBusSegment> {
        &self.bus_segments
    }

    /// Returns the net segments.
    pub fn net_segments(&self) -> &BTreeMap<NetSegmentId, SchematicNetSegment> {
        &self.net_segments
    }

    /// Returns the polygons.
    pub fn polygons(&self) -> &BTreeMap<Uuid, Polygon> {
        &self.polygons
    }

    /// Returns the texts.
    pub fn texts(&self) -> &BTreeMap<Uuid, Text> {
        &self.texts
    }

    /// Returns the images (files in the schematic directory).
    pub fn images(&self) -> &BTreeMap<Uuid, Image> {
        &self.images
    }

    /// Adds or replaces a symbol (for building a schematic before adding
    /// it to a project; a project's schematics change through mutations
    /// only), returns the previous one with the same UUID.
    pub fn insert_symbol(&mut self, symbol: SchematicSymbol) -> Option<SchematicSymbol> {
        self.symbols.insert(symbol.id(), symbol)
    }

    /// Adds or replaces a bus segment, see [`insert_symbol()`](Self::insert_symbol).
    pub fn insert_bus_segment(
        &mut self,
        segment: SchematicBusSegment,
    ) -> Option<SchematicBusSegment> {
        self.bus_segments.insert(segment.id(), segment)
    }

    /// Adds or replaces a net segment, see [`insert_symbol()`](Self::insert_symbol).
    pub fn insert_net_segment(
        &mut self,
        segment: SchematicNetSegment,
    ) -> Option<SchematicNetSegment> {
        self.net_segments.insert(segment.id(), segment)
    }

    /// Adds or replaces a polygon, see [`insert_symbol()`](Self::insert_symbol).
    pub fn insert_polygon(&mut self, polygon: Polygon) -> Option<Polygon> {
        self.polygons.insert(polygon.uuid(), polygon)
    }

    /// Adds or replaces a text, see [`insert_symbol()`](Self::insert_symbol).
    pub fn insert_text(&mut self, text: Text) -> Option<Text> {
        self.texts.insert(text.uuid(), text)
    }

    /// Adds or replaces an image, see [`insert_symbol()`](Self::insert_symbol).
    pub fn insert_image(&mut self, image: Image) -> Option<Image> {
        self.images.insert(image.uuid(), image)
    }

    /// Whether the schematic contains no items (upstream `isEmpty()`).
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
            && self.bus_segments.is_empty()
            && self.net_segments.is_empty()
            && self.polygons.is_empty()
            && self.texts.is_empty()
            && self.images.is_empty()
    }

    /// Returns the editable properties.
    pub fn properties(&self) -> SchematicProperties {
        SchematicProperties {
            id: self.id(),
            name: self.name.clone(),
            grid_interval: self.grid_interval,
            grid_unit: self.grid_unit,
        }
    }

    /// Replaces the editable properties, returns the previous ones.
    pub fn set_properties(&mut self, p: SchematicProperties) -> SchematicProperties {
        let old = self.properties();
        self.name = p.name;
        self.grid_interval = p.grid_interval;
        self.grid_unit = p.grid_unit;
        old
    }

    // --- Connectivity queries ---

    /// Returns the net segment whose lines end at a symbol pin (upstream
    /// `SI_SymbolPin::getNetSegmentOfLines()`).
    pub fn pin_net_segment(&self, symbol: SymbolId, pin: Uuid) -> Option<NetSegmentId> {
        self.derived.anchors.pin(symbol, pin).map(|u| u.segment)
    }

    /// Returns the net segments whose lines end at a bus junction (upstream
    /// `SI_BusJunction::getNetLines()`).
    pub fn bus_junction_net_segments(
        &self,
        segment: BusSegmentId,
        junction: Uuid,
    ) -> impl Iterator<Item = NetSegmentId> + '_ {
        self.derived
            .anchors
            .bus_junction_segments(segment, junction)
    }

    /// Returns the net segments attached to a bus segment (upstream
    /// `SI_BusSegment::getAttachedNetSegments()`).
    pub fn attached_net_segments(&self, segment: BusSegmentId) -> Vec<NetSegmentId> {
        let mut result: Vec<_> = self
            .bus_segments
            .get(&segment)
            .into_iter()
            .flat_map(|s| s.junctions().keys())
            .flat_map(|j| self.bus_junction_net_segments(segment, *j))
            .collect();
        result.sort();
        result.dedup();
        result
    }

    /// Returns the position of a net line anchor of the net segment
    /// `segment` (junctions are relative to their segment), `None` if it
    /// cannot be resolved.
    pub fn net_line_anchor_position(
        &self,
        segment: NetSegmentId,
        anchor: NetLineAnchor,
        ctx: ProjectView<'_>,
    ) -> Option<Point> {
        match anchor {
            NetLineAnchor::Junction(junction) => self
                .net_segments
                .get(&segment)?
                .junctions()
                .get(&junction)
                .map(|j| j.position()),
            NetLineAnchor::BusJunction { segment, junction } => self
                .bus_segments
                .get(&BusSegmentId(segment))?
                .junctions()
                .get(&junction)
                .map(|j| j.position()),
            NetLineAnchor::Pin { symbol, pin } => self
                .symbols
                .get(&SymbolId(symbol))?
                .pin(ctx, pin)
                .ok()
                .flatten()
                .map(|p| p.position()),
        }
    }

    /// Returns the anchor point of a net label: the nearest point on the
    /// lines of its segment (upstream `SI_NetLabel::updateAnchor()`).
    pub fn net_label_anchor(
        &self,
        segment: NetSegmentId,
        label: Uuid,
        ctx: ProjectView<'_>,
    ) -> Option<Point> {
        let s = self.net_segments.get(&segment)?;
        let label = s.labels().get(&label)?;
        Some(s.nearest_point(label.position(), |a| {
            self.net_line_anchor_position(segment, a, ctx)
        }))
    }

    /// Returns the anchor point of a bus label (upstream
    /// `SI_BusLabel::updateAnchor()`).
    pub fn bus_label_anchor(&self, segment: BusSegmentId, label: Uuid) -> Option<Point> {
        let s = self.bus_segments.get(&segment)?;
        Some(s.nearest_point(s.labels().get(&label)?.position()))
    }

    // --- Project integration ---

    /// All references of the items into the circuit, for the project's
    /// reverse index.
    pub(crate) fn references(&self, id: SchematicId) -> Vec<Reference> {
        let symbols = self.symbols.values().map(|s| symbol_reference(id, s));
        let buses = self
            .bus_segments
            .values()
            .map(|s| bus_segment_reference(id, s));
        let nets = self
            .net_segments
            .values()
            .map(|s| net_segment_reference(id, s));
        symbols.chain(buses).chain(nets).collect()
    }

    /// Whether net lines are connected to a pin of the given component
    /// signal (upstream `ComponentSignalInstance::arePinsOrPadsUsed()`).
    pub(crate) fn is_component_signal_wired(&self, signal: ComponentSignalRef) -> bool {
        self.derived.anchors.is_signal_wired(signal)
    }

    /// Builds the anchor index from scratch, validating all net segments
    /// (see [`check_net_segment()`]).
    pub(crate) fn build_anchor_index(&self, ctx: ProjectView<'_>) -> Result<AnchorIndex> {
        let mut index = AnchorIndex::default();
        for (id, segment) in &self.net_segments {
            let anchors = check_net_segment(self, &index, segment, ctx, false)?;
            index.add_segment(*id, &anchors);
        }
        Ok(index)
    }

    /// Whether the maintained anchor index equals a rebuild from scratch.
    pub(crate) fn is_anchor_index_consistent(&self, ctx: ProjectView<'_>) -> bool {
        self.build_anchor_index(ctx)
            .is_ok_and(|index| index == self.derived.anchors)
    }

    /// Writes `schematic.lp` and `settings.user.lp` into `dir` (upstream
    /// `Schematic::save()`).
    pub(crate) fn save(&self, dir: &mut TransactionalDirectory) -> fileio::Result<()> {
        let mut root = List::new("librepcb_schematic");
        self.serialize(&mut root);
        dir.write(
            "schematic.lp",
            &SExpression::from(root).to_byte_array(Mode::LibrePcb)?,
        )?;
        let mut user = List::new("librepcb_schematic_user_settings");
        user.ensure_line_break();
        dir.write(
            "settings.user.lp",
            &SExpression::from(user).to_byte_array(Mode::LibrePcb)?,
        )?;
        Ok(())
    }
}

impl SerializeObject for Schematic {
    /// Writes the content of `schematic.lp`.
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.uuid);
        root.ensure_line_break();
        root.append_child("name", &self.name);
        root.ensure_line_break();
        let grid = root.append_list("grid");
        grid.append_child("interval", &self.grid_interval);
        grid.append_child("unit", &self.grid_unit);
        root.ensure_line_break();
        serialize_items(root, "symbol", self.symbols.values());
        serialize_items(root, "bussegment", self.bus_segments.values());
        serialize_items(root, "netsegment", self.net_segments.values());
        serialize_items(root, "polygon", self.polygons.values());
        serialize_items(root, "text", self.texts.values());
        serialize_items(root, "image", self.images.values());
    }
}

fn serialize_items<'a, T: SerializeObject + 'a>(
    root: &mut List,
    name: &str,
    items: impl Iterator<Item = &'a T>,
) {
    for item in items {
        root.ensure_line_break();
        item.serialize(root.append_list(name));
    }
    root.ensure_line_break();
}

pub(crate) fn symbol_reference(schematic: SchematicId, symbol: &SchematicSymbol) -> Reference {
    Reference::Symbol {
        component: symbol.component(),
        gate: symbol.lib_gate(),
        at: (schematic, symbol.id()),
    }
}

pub(crate) fn net_segment_reference(
    schematic: SchematicId,
    segment: &SchematicNetSegment,
) -> Reference {
    Reference::Net {
        net: segment.net(),
        by: NetUse::SchematicSegment(schematic, segment.id()),
    }
}

pub(crate) fn bus_segment_reference(
    schematic: SchematicId,
    segment: &SchematicBusSegment,
) -> Reference {
    Reference::Bus {
        bus: segment.bus(),
        by: (schematic, segment.id()),
    }
}

/// Derived (non-persistent) data of a schematic: the anchor index (which
/// net segment's lines end at each pin and bus junction).
///
/// Never participates in equality: two schematics with the same persistent
/// content are equal regardless of their derived data.
#[derive(Debug, Clone, Default)]
pub struct SchematicDerived {
    pub(crate) anchors: AnchorIndex,
}

impl PartialEq for SchematicDerived {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

/// The editable properties of a [`Schematic`] (upstream `CmdSchematicEdit`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SchematicProperties {
    /// The schematic.
    pub id: SchematicId,
    /// The name.
    pub name: ElementName,
    /// The grid interval.
    pub grid_interval: PositiveLength,
    /// The grid unit.
    pub grid_unit: LengthUnit,
}
