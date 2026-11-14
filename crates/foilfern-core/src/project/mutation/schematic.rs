//! Schematic mutations: pages (upstream `Project::addSchematic()`,
//! `removeSchematic()`, `Schematic::setName()`) and, in
//! [`SchematicMutation`], the items (upstream `Schematic::addSymbol()`,
//! `SI_NetSegment::addNetPointsAndNetLines()`, the item setters and the
//! editor's `CmdSchematic*` commands built on them).
//!
//! Every function validates first (check-then-commit), then modifies the
//! schematic, its anchor index, the project's reverse index and the
//! journal, and returns the inverse mutation. Removals are strict like
//! upstream: a symbol with connected pins, a junction with lines, a bus
//! segment with attached net lines cannot be removed, and the net (bus) of
//! a segment can only change while the segment is empty. Since only empty
//! pages can be removed, the inverse of adding a page with items is a
//! [`Mutation::Batch`] removing the items first.

use std::collections::{BTreeMap, BTreeSet};

use super::Mutation;
use crate::fileio::FileSystem;
use crate::geometry::{Image, Junction, NetLabel, NetLine, NetLineAnchor, Polygon, Text};
use crate::project::Project;
use crate::project::change::Change;
use crate::project::error::{EntityKind, Error, Result};
use crate::project::id::{
    BusId, BusSegmentRef, ComponentInstanceId, NetSegmentRef, NetSignalId, SchematicId, SymbolRef,
};
use crate::project::schematic::{
    Schematic, SchematicBusSegment, SchematicChange, SchematicNetSegment, SchematicProperties,
    SchematicSymbol, SegmentAnchors, bus_segment_reference, check_bus_segment, check_net_segment,
    net_segment_reference, symbol_reference,
};
use crate::serialization::HasUuid;
use crate::types::{Angle, Point, UnsignedLength, Uuid};

/// A change of the items of a schematic, see
/// [`Mutation::Schematic`](super::Mutation::Schematic).
///
/// Granularity follows upstream's undo commands: add/remove of items,
/// property changes, adding/removing junctions and lines of a segment
/// (the result must be cohesive), and re-targeting empty segments.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum SchematicMutation {
    // --- Symbols ---
    /// Adds a symbol (a gate of a component instance, not placed yet).
    AddSymbol {
        /// The schematic.
        schematic: SchematicId,
        /// The symbol (with its texts).
        symbol: SchematicSymbol,
    },
    /// Removes a symbol whose pins are not connected.
    RemoveSymbol(SymbolRef),
    /// Moves, rotates or mirrors a symbol.
    SetSymbolPlacement {
        /// The symbol.
        symbol: SymbolRef,
        /// The new position.
        position: Point,
        /// The new rotation.
        rotation: Angle,
        /// The new mirror state.
        mirrored: bool,
    },
    /// Adds a text to a symbol.
    AddSymbolText {
        /// The symbol.
        symbol: SymbolRef,
        /// The text.
        text: Text,
    },
    /// Removes a text of a symbol.
    RemoveSymbolText {
        /// The symbol.
        symbol: SymbolRef,
        /// UUID of the text.
        text: Uuid,
    },
    /// Replaces a text of a symbol (same UUID).
    UpdateSymbolText {
        /// The symbol.
        symbol: SymbolRef,
        /// The new text.
        text: Text,
    },

    // --- Net segments ---
    /// Adds a net segment (with its elements).
    AddNetSegment {
        /// The schematic.
        schematic: SchematicId,
        /// The segment.
        segment: SchematicNetSegment,
    },
    /// Removes a net segment with all its elements.
    RemoveNetSegment(NetSegmentRef),
    /// Changes the net of an empty net segment.
    SetNetSegmentNet {
        /// The segment.
        segment: NetSegmentRef,
        /// The new net.
        net: NetSignalId,
    },
    /// Adds junctions and lines to a net segment.
    AddNetSegmentElements {
        /// The segment.
        segment: NetSegmentRef,
        /// The junctions.
        junctions: Vec<Junction>,
        /// The lines.
        lines: Vec<NetLine>,
    },
    /// Removes junctions and lines from a net segment.
    RemoveNetSegmentElements {
        /// The segment.
        segment: NetSegmentRef,
        /// UUIDs of the junctions.
        junctions: Vec<Uuid>,
        /// UUIDs of the lines.
        lines: Vec<Uuid>,
    },
    /// Moves a junction of a net segment.
    SetNetPointPosition {
        /// The segment.
        segment: NetSegmentRef,
        /// The junction.
        junction: Uuid,
        /// The new position.
        position: Point,
    },
    /// Changes the width of a net line.
    SetNetLineWidth {
        /// The segment.
        segment: NetSegmentRef,
        /// The line.
        line: Uuid,
        /// The new width.
        width: UnsignedLength,
    },
    /// Adds a net label.
    AddNetLabel {
        /// The segment.
        segment: NetSegmentRef,
        /// The label.
        label: NetLabel,
    },
    /// Removes a net label.
    RemoveNetLabel {
        /// The segment.
        segment: NetSegmentRef,
        /// UUID of the label.
        label: Uuid,
    },
    /// Replaces a net label (same UUID).
    UpdateNetLabel {
        /// The segment.
        segment: NetSegmentRef,
        /// The new label.
        label: NetLabel,
    },

    // --- Bus segments ---
    /// Adds a bus segment (with its elements).
    AddBusSegment {
        /// The schematic.
        schematic: SchematicId,
        /// The segment.
        segment: SchematicBusSegment,
    },
    /// Removes a bus segment without attached net lines.
    RemoveBusSegment(BusSegmentRef),
    /// Changes the bus of an empty bus segment.
    SetBusSegmentBus {
        /// The segment.
        segment: BusSegmentRef,
        /// The new bus.
        bus: BusId,
    },
    /// Adds junctions and lines to a bus segment.
    AddBusSegmentElements {
        /// The segment.
        segment: BusSegmentRef,
        /// The junctions.
        junctions: Vec<Junction>,
        /// The lines.
        lines: Vec<NetLine>,
    },
    /// Removes junctions and lines from a bus segment.
    RemoveBusSegmentElements {
        /// The segment.
        segment: BusSegmentRef,
        /// UUIDs of the junctions.
        junctions: Vec<Uuid>,
        /// UUIDs of the lines.
        lines: Vec<Uuid>,
    },
    /// Moves a junction of a bus segment.
    SetBusJunctionPosition {
        /// The segment.
        segment: BusSegmentRef,
        /// The junction.
        junction: Uuid,
        /// The new position.
        position: Point,
    },
    /// Changes the width of a bus line.
    SetBusLineWidth {
        /// The segment.
        segment: BusSegmentRef,
        /// The line.
        line: Uuid,
        /// The new width.
        width: UnsignedLength,
    },
    /// Adds a bus label.
    AddBusLabel {
        /// The segment.
        segment: BusSegmentRef,
        /// The label.
        label: NetLabel,
    },
    /// Removes a bus label.
    RemoveBusLabel {
        /// The segment.
        segment: BusSegmentRef,
        /// UUID of the label.
        label: Uuid,
    },
    /// Replaces a bus label (same UUID).
    UpdateBusLabel {
        /// The segment.
        segment: BusSegmentRef,
        /// The new label.
        label: NetLabel,
    },

    // --- Polygons, texts, images ---
    /// Adds a polygon.
    AddPolygon {
        /// The schematic.
        schematic: SchematicId,
        /// The polygon.
        polygon: Polygon,
    },
    /// Removes a polygon.
    RemovePolygon {
        /// The schematic.
        schematic: SchematicId,
        /// UUID of the polygon.
        polygon: Uuid,
    },
    /// Replaces a polygon (same UUID).
    UpdatePolygon {
        /// The schematic.
        schematic: SchematicId,
        /// The new polygon.
        polygon: Polygon,
    },
    /// Adds a text.
    AddText {
        /// The schematic.
        schematic: SchematicId,
        /// The text.
        text: Text,
    },
    /// Removes a text.
    RemoveText {
        /// The schematic.
        schematic: SchematicId,
        /// UUID of the text.
        text: Uuid,
    },
    /// Replaces a text (same UUID).
    UpdateText {
        /// The schematic.
        schematic: SchematicId,
        /// The new text.
        text: Text,
    },
    /// Adds an image (the file must be written to the schematic directory
    /// separately; upstream does not check it either).
    AddImage {
        /// The schematic.
        schematic: SchematicId,
        /// The image.
        image: Image,
    },
    /// Removes an image (its file stays).
    RemoveImage {
        /// The schematic.
        schematic: SchematicId,
        /// UUID of the image.
        image: Uuid,
    },
    /// Replaces an image (same UUID).
    UpdateImage {
        /// The schematic.
        schematic: SchematicId,
        /// The new image.
        image: Image,
    },
}

use SchematicMutation as M;

pub(super) fn apply(p: &mut Project, m: SchematicMutation) -> Result<Mutation> {
    let inverse = match m {
        M::AddSymbol { schematic, symbol } => add_symbol(p, schematic, symbol)?,
        M::RemoveSymbol(r) => remove_symbol(p, r)?,
        M::SetSymbolPlacement {
            symbol,
            position,
            rotation,
            mirrored,
        } => set_symbol_placement(p, symbol, position, rotation, mirrored)?,
        M::AddSymbolText { symbol, text } => add_symbol_text(p, symbol, text)?,
        M::RemoveSymbolText { symbol, text } => remove_symbol_text(p, symbol, text)?,
        M::UpdateSymbolText { symbol, text } => update_symbol_text(p, symbol, text)?,
        M::AddNetSegment { schematic, segment } => add_net_segment(p, schematic, segment)?,
        M::RemoveNetSegment(r) => remove_net_segment(p, r)?,
        M::SetNetSegmentNet { segment, net } => set_net_segment_net(p, segment, net)?,
        M::AddNetSegmentElements {
            segment,
            junctions,
            lines,
        } => add_net_segment_elements(p, segment, junctions, lines)?,
        M::RemoveNetSegmentElements {
            segment,
            junctions,
            lines,
        } => remove_net_segment_elements(p, segment, junctions, lines)?,
        M::SetNetPointPosition {
            segment,
            junction,
            position,
        } => set_net_point_position(p, segment, junction, position)?,
        M::SetNetLineWidth {
            segment,
            line,
            width,
        } => set_net_line_width(p, segment, line, width)?,
        M::AddNetLabel { segment, label } => add_net_label(p, segment, label)?,
        M::RemoveNetLabel { segment, label } => remove_net_label(p, segment, label)?,
        M::UpdateNetLabel { segment, label } => update_net_label(p, segment, label)?,
        M::AddBusSegment { schematic, segment } => add_bus_segment(p, schematic, segment)?,
        M::RemoveBusSegment(r) => remove_bus_segment(p, r)?,
        M::SetBusSegmentBus { segment, bus } => set_bus_segment_bus(p, segment, bus)?,
        M::AddBusSegmentElements {
            segment,
            junctions,
            lines,
        } => add_bus_segment_elements(p, segment, junctions, lines)?,
        M::RemoveBusSegmentElements {
            segment,
            junctions,
            lines,
        } => remove_bus_segment_elements(p, segment, junctions, lines)?,
        M::SetBusJunctionPosition {
            segment,
            junction,
            position,
        } => set_bus_junction_position(p, segment, junction, position)?,
        M::SetBusLineWidth {
            segment,
            line,
            width,
        } => set_bus_line_width(p, segment, line, width)?,
        M::AddBusLabel { segment, label } => add_bus_label(p, segment, label)?,
        M::RemoveBusLabel { segment, label } => remove_bus_label(p, segment, label)?,
        M::UpdateBusLabel { segment, label } => update_bus_label(p, segment, label)?,
        M::AddPolygon { schematic, polygon } => {
            let uuid = polygon.uuid();
            let s = schematic_mut(p, schematic)?;
            insert_new(&mut s.polygons, polygon, EntityKind::SchematicPolygon)?;
            record(p, schematic, SchematicChange::PolygonAdded(uuid));
            M::RemovePolygon {
                schematic,
                polygon: uuid,
            }
        }
        M::RemovePolygon { schematic, polygon } => {
            let s = schematic_mut(p, schematic)?;
            let polygon_value = take(&mut s.polygons, polygon, EntityKind::SchematicPolygon)?;
            record(p, schematic, SchematicChange::PolygonRemoved(polygon));
            M::AddPolygon {
                schematic,
                polygon: polygon_value,
            }
        }
        M::UpdatePolygon { schematic, polygon } => {
            let uuid = polygon.uuid();
            let s = schematic_mut(p, schematic)?;
            let old = replace(&mut s.polygons, polygon, EntityKind::SchematicPolygon)?;
            record(p, schematic, SchematicChange::PolygonChanged(uuid));
            M::UpdatePolygon {
                schematic,
                polygon: old,
            }
        }
        M::AddText { schematic, text } => {
            let uuid = text.uuid();
            let s = schematic_mut(p, schematic)?;
            insert_new(&mut s.texts, text, EntityKind::SchematicText)?;
            record(p, schematic, SchematicChange::TextAdded(uuid));
            M::RemoveText {
                schematic,
                text: uuid,
            }
        }
        M::RemoveText { schematic, text } => {
            let s = schematic_mut(p, schematic)?;
            let text_value = take(&mut s.texts, text, EntityKind::SchematicText)?;
            record(p, schematic, SchematicChange::TextRemoved(text));
            M::AddText {
                schematic,
                text: text_value,
            }
        }
        M::UpdateText { schematic, text } => {
            let uuid = text.uuid();
            let s = schematic_mut(p, schematic)?;
            let old = replace(&mut s.texts, text, EntityKind::SchematicText)?;
            record(p, schematic, SchematicChange::TextChanged(uuid));
            M::UpdateText {
                schematic,
                text: old,
            }
        }
        M::AddImage { schematic, image } => {
            let uuid = image.uuid();
            let s = schematic_mut(p, schematic)?;
            insert_new(&mut s.images, image, EntityKind::SchematicImage)?;
            record(p, schematic, SchematicChange::ImageAdded(uuid));
            M::RemoveImage {
                schematic,
                image: uuid,
            }
        }
        M::RemoveImage { schematic, image } => {
            let s = schematic_mut(p, schematic)?;
            let image_value = take(&mut s.images, image, EntityKind::SchematicImage)?;
            record(p, schematic, SchematicChange::ImageRemoved(image));
            M::AddImage {
                schematic,
                image: image_value,
            }
        }
        M::UpdateImage { schematic, image } => {
            let uuid = image.uuid();
            let s = schematic_mut(p, schematic)?;
            let old = replace(&mut s.images, image, EntityKind::SchematicImage)?;
            record(p, schematic, SchematicChange::ImageChanged(uuid));
            M::UpdateImage {
                schematic,
                image: old,
            }
        }
    };
    Ok(Mutation::Schematic(inverse))
}

// --- Pages ---

pub(super) fn add_schematic(
    p: &mut Project,
    mut schematic: Schematic,
    index: Option<usize>,
) -> Result<Mutation> {
    let id = schematic.id();
    if p.schematic(id).is_some() {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::Schematic,
            uuid: id.0,
        });
    }
    check_schematic_name(p, schematic.name(), None)?;
    if p.schematics
        .iter()
        .any(|s| s.directory_name() == schematic.directory_name())
    {
        return Err(Error::DuplicateDirectoryName {
            kind: EntityKind::Schematic,
            name: schematic.directory_name().to_owned(),
        });
    }
    // Validate all items like upstream `Schematic::addToProject()` (every
    // item's `addToSchematic()`), building the anchor index on the way.
    let ctx = p.view();
    let mut placed: BTreeMap<ComponentInstanceId, BTreeSet<Uuid>> = BTreeMap::new();
    for symbol in schematic.symbols.values() {
        symbol.pins(ctx)?;
        check_symbol_gate(p, id, symbol)?;
        if !placed
            .entry(symbol.component())
            .or_default()
            .insert(symbol.lib_gate())
        {
            return Err(Error::SymbolItemAlreadyPlaced(symbol.lib_gate()));
        }
    }
    for segment in schematic.bus_segments.values() {
        check_bus_segment(segment, ctx)?;
    }
    schematic.derived.anchors = schematic.build_anchor_index(ctx)?;
    for r in schematic.references(id) {
        p.refs.insert(r);
    }
    let inverse = remove_schematic_with_items(&schematic);
    let index = index.unwrap_or(p.schematics.len()).min(p.schematics.len());
    p.schematics.insert(index, schematic);
    p.record(Change::SchematicAdded(id));
    p.record(Change::ProjectMetadata);
    Ok(inverse)
}

/// The inverse of adding `schematic`: `RemoveSchematic`, preceded by the
/// removal of all items if it is not empty (removing a page requires an
/// empty page, like upstream). Net segments go first since they lock the
/// pins and bus junctions they are connected to.
fn remove_schematic_with_items(schematic: &Schematic) -> Mutation {
    let id = schematic.id();
    let mut steps: Vec<Mutation> = Vec::new();
    steps.extend(schematic.net_segments.keys().map(|segment| {
        sch(M::RemoveNetSegment(NetSegmentRef {
            schematic: id,
            segment: *segment,
        }))
    }));
    steps.extend(schematic.bus_segments.keys().map(|segment| {
        sch(M::RemoveBusSegment(BusSegmentRef {
            schematic: id,
            segment: *segment,
        }))
    }));
    steps.extend(schematic.symbols.keys().map(|symbol| {
        sch(M::RemoveSymbol(SymbolRef {
            schematic: id,
            symbol: *symbol,
        }))
    }));
    steps.extend(schematic.polygons.keys().map(|polygon| {
        sch(M::RemovePolygon {
            schematic: id,
            polygon: *polygon,
        })
    }));
    steps.extend(schematic.texts.keys().map(|text| {
        sch(M::RemoveText {
            schematic: id,
            text: *text,
        })
    }));
    steps.extend(schematic.images.keys().map(|image| {
        sch(M::RemoveImage {
            schematic: id,
            image: *image,
        })
    }));
    if steps.is_empty() {
        Mutation::RemoveSchematic(id)
    } else {
        steps.push(Mutation::RemoveSchematic(id));
        Mutation::Batch(steps)
    }
}

fn sch(m: SchematicMutation) -> Mutation {
    Mutation::Schematic(m)
}

pub(super) fn remove_schematic(p: &mut Project, id: SchematicId) -> Result<Mutation> {
    let index = schematic_index(p, id)?;
    let schematic = &p.schematics[index];
    if !schematic.is_empty() {
        return Err(Error::SchematicNotEmpty(schematic.name().to_string()));
    }
    // upstream: moves the schematic directory into a temporary file system
    p.directory
        .remove_dir_recursively(&format!("schematics/{}", schematic.directory_name()))?;
    let schematic = p.schematics.remove(index);
    p.refs.remove_schematic(id);
    p.record(Change::SchematicRemoved(id));
    p.record(Change::ProjectMetadata);
    Ok(Mutation::AddSchematic {
        schematic,
        index: Some(index),
    })
}

pub(super) fn update_schematic(
    p: &mut Project,
    properties: SchematicProperties,
) -> Result<Mutation> {
    let id = properties.id;
    let index = schematic_index(p, id)?;
    check_schematic_name(p, &properties.name, Some(id))?;
    let old = p.schematics[index].set_properties(properties);
    p.record(Change::SchematicChanged(id));
    p.record(Change::ProjectMetadata);
    Ok(Mutation::UpdateSchematic(old))
}

fn check_schematic_name(p: &Project, name: &str, except: Option<SchematicId>) -> Result<()> {
    if p.schematic_by_name(name)
        .is_some_and(|s| Some(s.id()) != except)
    {
        return Err(Error::DuplicateName {
            kind: EntityKind::Schematic,
            name: name.to_owned(),
        });
    }
    Ok(())
}

// --- Symbols ---

/// Upstream `ComponentInstance::registerSymbol()`: a gate is placed at most
/// once, and all symbols of a component are in the same schematic.
fn check_symbol_gate(p: &Project, schematic: SchematicId, symbol: &SchematicSymbol) -> Result<()> {
    if let Some(uses) = p.refs.component_uses(symbol.component()) {
        if uses.symbols.contains_key(&symbol.lib_gate()) {
            return Err(Error::SymbolItemAlreadyPlaced(symbol.lib_gate()));
        }
        if uses.symbols.values().any(|(s, _)| *s != schematic) {
            return Err(Error::SymbolsInDifferentSchematics);
        }
    }
    Ok(())
}

fn add_symbol(
    p: &mut Project,
    schematic: SchematicId,
    symbol: SchematicSymbol,
) -> Result<SchematicMutation> {
    let s = schematic_ref(p, schematic)?;
    let id = symbol.id();
    if s.symbols.contains_key(&id) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::Symbol,
            uuid: id.0,
        });
    }
    symbol.pins(p.view())?;
    check_symbol_gate(p, schematic, &symbol)?;
    p.refs.insert(symbol_reference(schematic, &symbol));
    schematic_mut(p, schematic)?.symbols.insert(id, symbol);
    record(p, schematic, SchematicChange::SymbolAdded(id));
    Ok(M::RemoveSymbol(SymbolRef {
        schematic,
        symbol: id,
    }))
}

fn remove_symbol(p: &mut Project, r: SymbolRef) -> Result<SchematicMutation> {
    let s = schematic_ref(p, r.schematic)?;
    symbol_ref(s, r)?;
    if s.derived.anchors.is_symbol_wired(r.symbol) {
        return Err(Error::ItemInUse {
            kind: EntityKind::Symbol,
            uuid: r.symbol.0,
        });
    }
    let symbol = schematic_mut(p, r.schematic)?
        .symbols
        .remove(&r.symbol)
        .expect("checked above");
    p.refs.remove(&symbol_reference(r.schematic, &symbol));
    record(p, r.schematic, SchematicChange::SymbolRemoved(r.symbol));
    Ok(M::AddSymbol {
        schematic: r.schematic,
        symbol,
    })
}

fn set_symbol_placement(
    p: &mut Project,
    r: SymbolRef,
    position: Point,
    rotation: Angle,
    mirrored: bool,
) -> Result<SchematicMutation> {
    let symbol = symbol_mut(p, r)?;
    let inverse = M::SetSymbolPlacement {
        symbol: r,
        position: symbol.position(),
        rotation: symbol.rotation(),
        mirrored: symbol.mirrored(),
    };
    symbol.set_position(position);
    symbol.set_rotation(rotation);
    symbol.set_mirrored(mirrored);
    record(
        p,
        r.schematic,
        SchematicChange::SymbolPlacementChanged(r.symbol),
    );
    Ok(inverse)
}

fn add_symbol_text(p: &mut Project, r: SymbolRef, text: Text) -> Result<SchematicMutation> {
    let symbol = symbol_mut(p, r)?;
    let uuid = text.uuid();
    if symbol.texts().contains_key(&uuid) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::SchematicText,
            uuid,
        });
    }
    symbol.insert_text(text);
    record(
        p,
        r.schematic,
        SchematicChange::SymbolTextsChanged(r.symbol),
    );
    Ok(M::RemoveSymbolText {
        symbol: r,
        text: uuid,
    })
}

fn remove_symbol_text(p: &mut Project, r: SymbolRef, uuid: Uuid) -> Result<SchematicMutation> {
    let text = symbol_mut(p, r)?
        .remove_text(&uuid)
        .ok_or(Error::NotFound {
            kind: EntityKind::SchematicText,
            uuid,
        })?;
    record(
        p,
        r.schematic,
        SchematicChange::SymbolTextsChanged(r.symbol),
    );
    Ok(M::AddSymbolText { symbol: r, text })
}

fn update_symbol_text(p: &mut Project, r: SymbolRef, text: Text) -> Result<SchematicMutation> {
    let symbol = symbol_mut(p, r)?;
    let uuid = text.uuid();
    if !symbol.texts().contains_key(&uuid) {
        return Err(Error::NotFound {
            kind: EntityKind::SchematicText,
            uuid,
        });
    }
    let old = symbol.insert_text(text).expect("checked above");
    record(
        p,
        r.schematic,
        SchematicChange::SymbolTextsChanged(r.symbol),
    );
    Ok(M::UpdateSymbolText {
        symbol: r,
        text: old,
    })
}

// --- Net segments ---

fn add_net_segment(
    p: &mut Project,
    schematic: SchematicId,
    segment: SchematicNetSegment,
) -> Result<SchematicMutation> {
    let s = schematic_ref(p, schematic)?;
    let id = segment.id();
    if s.net_segments.contains_key(&id) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::NetSegment,
            uuid: id.0,
        });
    }
    let anchors = check_net_segment(s, &s.derived.anchors, &segment, p.view(), false)?;
    p.refs.insert(net_segment_reference(schematic, &segment));
    let s = schematic_mut(p, schematic)?;
    s.derived.anchors.add_segment(id, &anchors);
    s.net_segments.insert(id, segment);
    record(p, schematic, SchematicChange::NetSegmentAdded(id));
    Ok(M::RemoveNetSegment(NetSegmentRef {
        schematic,
        segment: id,
    }))
}

fn remove_net_segment(p: &mut Project, r: NetSegmentRef) -> Result<SchematicMutation> {
    net_segment_ref(schematic_ref(p, r.schematic)?, r)?;
    let s = schematic_mut(p, r.schematic)?;
    let segment = s.net_segments.remove(&r.segment).expect("checked above");
    s.derived.anchors.remove_segment(r.segment, &segment);
    p.refs.remove(&net_segment_reference(r.schematic, &segment));
    record(
        p,
        r.schematic,
        SchematicChange::NetSegmentRemoved(r.segment),
    );
    Ok(M::AddNetSegment {
        schematic: r.schematic,
        segment,
    })
}

fn set_net_segment_net(
    p: &mut Project,
    r: NetSegmentRef,
    net: NetSignalId,
) -> Result<SchematicMutation> {
    let segment = net_segment_ref(schematic_ref(p, r.schematic)?, r)?;
    let old = segment.net();
    if old == net {
        return Ok(M::SetNetSegmentNet { segment: r, net });
    }
    if p.circuit.net_signal(net).is_none() {
        return Err(Error::InexistentNetSignal(net.0));
    }
    if segment.is_used() {
        return Err(Error::ItemInUse {
            kind: EntityKind::NetSegment,
            uuid: r.segment.0,
        });
    }
    let old_reference = net_segment_reference(r.schematic, segment);
    p.refs.remove(&old_reference);
    let segment = net_segment_mut(p, r)?;
    segment.set_net(net);
    let reference = net_segment_reference(r.schematic, segment);
    p.refs.insert(reference);
    record(
        p,
        r.schematic,
        SchematicChange::NetSegmentNetChanged(r.segment),
    );
    Ok(M::SetNetSegmentNet {
        segment: r,
        net: old,
    })
}

fn add_net_segment_elements(
    p: &mut Project,
    r: NetSegmentRef,
    junctions: Vec<Junction>,
    lines: Vec<NetLine>,
) -> Result<SchematicMutation> {
    let s = schematic_ref(p, r.schematic)?;
    let mut candidate = net_segment_ref(s, r)?.clone();
    let junction_uuids: Vec<Uuid> = junctions.iter().map(Junction::uuid).collect();
    let line_uuids: Vec<Uuid> = lines.iter().map(NetLine::uuid).collect();
    for junction in junctions {
        insert_new(candidate.junctions_mut(), junction, EntityKind::NetPoint)?;
    }
    for line in lines {
        insert_new(candidate.lines_mut(), line, EntityKind::NetLine)?;
    }
    let anchors = check_net_segment(s, &s.derived.anchors, &candidate, p.view(), false)?;
    replace_net_segment(p, r, candidate, &anchors)?;
    record(
        p,
        r.schematic,
        SchematicChange::NetSegmentElementsAdded {
            segment: r.segment,
            junctions: junction_uuids.clone(),
            lines: line_uuids.clone(),
        },
    );
    Ok(M::RemoveNetSegmentElements {
        segment: r,
        junctions: junction_uuids,
        lines: line_uuids,
    })
}

fn remove_net_segment_elements(
    p: &mut Project,
    r: NetSegmentRef,
    junctions: Vec<Uuid>,
    lines: Vec<Uuid>,
) -> Result<SchematicMutation> {
    let s = schematic_ref(p, r.schematic)?;
    let mut candidate = net_segment_ref(s, r)?.clone();
    let removed_lines = take_all(candidate.lines_mut(), &lines, EntityKind::NetLine)?;
    let removed_junctions = take_all(candidate.junctions_mut(), &junctions, EntityKind::NetPoint)?;
    for junction in &junctions {
        if candidate
            .lines_at(NetLineAnchor::Junction(*junction))
            .next()
            .is_some()
        {
            return Err(Error::ItemInUse {
                kind: EntityKind::NetPoint,
                uuid: *junction,
            });
        }
    }
    let anchors = check_net_segment(s, &s.derived.anchors, &candidate, p.view(), true)?;
    replace_net_segment(p, r, candidate, &anchors)?;
    record(
        p,
        r.schematic,
        SchematicChange::NetSegmentElementsRemoved {
            segment: r.segment,
            junctions,
            lines,
        },
    );
    Ok(M::AddNetSegmentElements {
        segment: r,
        junctions: removed_junctions,
        lines: removed_lines,
    })
}

/// Replaces a (validated) net segment and updates the anchor index.
fn replace_net_segment(
    p: &mut Project,
    r: NetSegmentRef,
    segment: SchematicNetSegment,
    anchors: &SegmentAnchors,
) -> Result<()> {
    let s = schematic_mut(p, r.schematic)?;
    let old = s
        .net_segments
        .insert(r.segment, segment)
        .expect("segment exists");
    s.derived.anchors.remove_segment(r.segment, &old);
    s.derived.anchors.add_segment(r.segment, anchors);
    Ok(())
}

fn set_net_point_position(
    p: &mut Project,
    r: NetSegmentRef,
    uuid: Uuid,
    position: Point,
) -> Result<SchematicMutation> {
    let junction = net_segment_mut(p, r)?
        .junctions_mut()
        .get_mut(&uuid)
        .ok_or(Error::NotFound {
            kind: EntityKind::NetPoint,
            uuid,
        })?;
    let old = junction.position();
    junction.set_position(position);
    record(
        p,
        r.schematic,
        SchematicChange::NetPointChanged {
            segment: r.segment,
            junction: uuid,
        },
    );
    Ok(M::SetNetPointPosition {
        segment: r,
        junction: uuid,
        position: old,
    })
}

fn set_net_line_width(
    p: &mut Project,
    r: NetSegmentRef,
    uuid: Uuid,
    width: UnsignedLength,
) -> Result<SchematicMutation> {
    let line = net_segment_mut(p, r)?
        .lines_mut()
        .get_mut(&uuid)
        .ok_or(Error::NotFound {
            kind: EntityKind::NetLine,
            uuid,
        })?;
    let old = line.width();
    line.set_width(width);
    record(
        p,
        r.schematic,
        SchematicChange::NetLineChanged {
            segment: r.segment,
            line: uuid,
        },
    );
    Ok(M::SetNetLineWidth {
        segment: r,
        line: uuid,
        width: old,
    })
}

fn add_net_label(p: &mut Project, r: NetSegmentRef, label: NetLabel) -> Result<SchematicMutation> {
    let uuid = label.uuid();
    insert_new(
        net_segment_mut(p, r)?.labels_mut(),
        label,
        EntityKind::NetLabel,
    )?;
    let change = SchematicChange::NetLabelAdded {
        segment: r.segment,
        label: uuid,
    };
    record(p, r.schematic, change);
    Ok(M::RemoveNetLabel {
        segment: r,
        label: uuid,
    })
}

fn remove_net_label(p: &mut Project, r: NetSegmentRef, uuid: Uuid) -> Result<SchematicMutation> {
    let label = take(
        net_segment_mut(p, r)?.labels_mut(),
        uuid,
        EntityKind::NetLabel,
    )?;
    let change = SchematicChange::NetLabelRemoved {
        segment: r.segment,
        label: uuid,
    };
    record(p, r.schematic, change);
    Ok(M::AddNetLabel { segment: r, label })
}

fn update_net_label(
    p: &mut Project,
    r: NetSegmentRef,
    label: NetLabel,
) -> Result<SchematicMutation> {
    let uuid = label.uuid();
    let old = replace(
        net_segment_mut(p, r)?.labels_mut(),
        label,
        EntityKind::NetLabel,
    )?;
    let change = SchematicChange::NetLabelChanged {
        segment: r.segment,
        label: uuid,
    };
    record(p, r.schematic, change);
    Ok(M::UpdateNetLabel {
        segment: r,
        label: old,
    })
}

// --- Bus segments ---

fn add_bus_segment(
    p: &mut Project,
    schematic: SchematicId,
    segment: SchematicBusSegment,
) -> Result<SchematicMutation> {
    let s = schematic_ref(p, schematic)?;
    let id = segment.id();
    if s.bus_segments.contains_key(&id) {
        return Err(Error::DuplicateUuid {
            kind: EntityKind::BusSegment,
            uuid: id.0,
        });
    }
    check_bus_segment(&segment, p.view())?;
    p.refs.insert(bus_segment_reference(schematic, &segment));
    schematic_mut(p, schematic)?
        .bus_segments
        .insert(id, segment);
    record(p, schematic, SchematicChange::BusSegmentAdded(id));
    Ok(M::RemoveBusSegment(BusSegmentRef {
        schematic,
        segment: id,
    }))
}

fn remove_bus_segment(p: &mut Project, r: BusSegmentRef) -> Result<SchematicMutation> {
    let s = schematic_ref(p, r.schematic)?;
    bus_segment_ref(s, r)?;
    if s.derived.anchors.is_bus_segment_wired(r.segment) {
        return Err(Error::ItemInUse {
            kind: EntityKind::BusSegment,
            uuid: r.segment.0,
        });
    }
    let segment = schematic_mut(p, r.schematic)?
        .bus_segments
        .remove(&r.segment)
        .expect("checked above");
    p.refs.remove(&bus_segment_reference(r.schematic, &segment));
    record(
        p,
        r.schematic,
        SchematicChange::BusSegmentRemoved(r.segment),
    );
    Ok(M::AddBusSegment {
        schematic: r.schematic,
        segment,
    })
}

fn set_bus_segment_bus(p: &mut Project, r: BusSegmentRef, bus: BusId) -> Result<SchematicMutation> {
    let segment = bus_segment_ref(schematic_ref(p, r.schematic)?, r)?;
    let old = segment.bus();
    if old == bus {
        return Ok(M::SetBusSegmentBus { segment: r, bus });
    }
    if p.circuit.bus(bus).is_none() {
        return Err(Error::InexistentBus(bus.0));
    }
    if segment.is_used() {
        return Err(Error::ItemInUse {
            kind: EntityKind::BusSegment,
            uuid: r.segment.0,
        });
    }
    let old_reference = bus_segment_reference(r.schematic, segment);
    p.refs.remove(&old_reference);
    let segment = bus_segment_mut(p, r)?;
    segment.set_bus(bus);
    let reference = bus_segment_reference(r.schematic, segment);
    p.refs.insert(reference);
    record(
        p,
        r.schematic,
        SchematicChange::BusSegmentBusChanged(r.segment),
    );
    Ok(M::SetBusSegmentBus {
        segment: r,
        bus: old,
    })
}

fn add_bus_segment_elements(
    p: &mut Project,
    r: BusSegmentRef,
    junctions: Vec<Junction>,
    lines: Vec<NetLine>,
) -> Result<SchematicMutation> {
    let s = schematic_ref(p, r.schematic)?;
    let mut candidate = bus_segment_ref(s, r)?.clone();
    let junction_uuids: Vec<Uuid> = junctions.iter().map(Junction::uuid).collect();
    let line_uuids: Vec<Uuid> = lines.iter().map(NetLine::uuid).collect();
    for junction in junctions {
        insert_new(candidate.junctions_mut(), junction, EntityKind::BusJunction)?;
    }
    for line in lines {
        insert_new(candidate.lines_mut(), line, EntityKind::BusLine)?;
    }
    check_bus_segment(&candidate, p.view())?;
    *bus_segment_mut(p, r)? = candidate;
    record(
        p,
        r.schematic,
        SchematicChange::BusSegmentElementsAdded {
            segment: r.segment,
            junctions: junction_uuids.clone(),
            lines: line_uuids.clone(),
        },
    );
    Ok(M::RemoveBusSegmentElements {
        segment: r,
        junctions: junction_uuids,
        lines: line_uuids,
    })
}

fn remove_bus_segment_elements(
    p: &mut Project,
    r: BusSegmentRef,
    junctions: Vec<Uuid>,
    lines: Vec<Uuid>,
) -> Result<SchematicMutation> {
    let s = schematic_ref(p, r.schematic)?;
    let mut candidate = bus_segment_ref(s, r)?.clone();
    let removed_lines = take_all(candidate.lines_mut(), &lines, EntityKind::BusLine)?;
    let removed_junctions = take_all(
        candidate.junctions_mut(),
        &junctions,
        EntityKind::BusJunction,
    )?;
    for junction in &junctions {
        let has_bus_lines = candidate.lines_at(*junction).next().is_some();
        let has_net_lines = s
            .derived
            .anchors
            .bus_junction_segments(r.segment, *junction)
            .next()
            .is_some();
        if has_bus_lines || has_net_lines {
            return Err(Error::ItemInUse {
                kind: EntityKind::BusJunction,
                uuid: *junction,
            });
        }
    }
    check_bus_segment(&candidate, p.view())?;
    *bus_segment_mut(p, r)? = candidate;
    record(
        p,
        r.schematic,
        SchematicChange::BusSegmentElementsRemoved {
            segment: r.segment,
            junctions,
            lines,
        },
    );
    Ok(M::AddBusSegmentElements {
        segment: r,
        junctions: removed_junctions,
        lines: removed_lines,
    })
}

fn set_bus_junction_position(
    p: &mut Project,
    r: BusSegmentRef,
    uuid: Uuid,
    position: Point,
) -> Result<SchematicMutation> {
    let junction = bus_segment_mut(p, r)?
        .junctions_mut()
        .get_mut(&uuid)
        .ok_or(Error::NotFound {
            kind: EntityKind::BusJunction,
            uuid,
        })?;
    let old = junction.position();
    junction.set_position(position);
    record(
        p,
        r.schematic,
        SchematicChange::BusJunctionChanged {
            segment: r.segment,
            junction: uuid,
        },
    );
    Ok(M::SetBusJunctionPosition {
        segment: r,
        junction: uuid,
        position: old,
    })
}

fn set_bus_line_width(
    p: &mut Project,
    r: BusSegmentRef,
    uuid: Uuid,
    width: UnsignedLength,
) -> Result<SchematicMutation> {
    let line = bus_segment_mut(p, r)?
        .lines_mut()
        .get_mut(&uuid)
        .ok_or(Error::NotFound {
            kind: EntityKind::BusLine,
            uuid,
        })?;
    let old = line.width();
    line.set_width(width);
    record(
        p,
        r.schematic,
        SchematicChange::BusLineChanged {
            segment: r.segment,
            line: uuid,
        },
    );
    Ok(M::SetBusLineWidth {
        segment: r,
        line: uuid,
        width: old,
    })
}

fn add_bus_label(p: &mut Project, r: BusSegmentRef, label: NetLabel) -> Result<SchematicMutation> {
    let uuid = label.uuid();
    insert_new(
        bus_segment_mut(p, r)?.labels_mut(),
        label,
        EntityKind::BusLabel,
    )?;
    let change = SchematicChange::BusLabelAdded {
        segment: r.segment,
        label: uuid,
    };
    record(p, r.schematic, change);
    Ok(M::RemoveBusLabel {
        segment: r,
        label: uuid,
    })
}

fn remove_bus_label(p: &mut Project, r: BusSegmentRef, uuid: Uuid) -> Result<SchematicMutation> {
    let label = take(
        bus_segment_mut(p, r)?.labels_mut(),
        uuid,
        EntityKind::BusLabel,
    )?;
    let change = SchematicChange::BusLabelRemoved {
        segment: r.segment,
        label: uuid,
    };
    record(p, r.schematic, change);
    Ok(M::AddBusLabel { segment: r, label })
}

fn update_bus_label(
    p: &mut Project,
    r: BusSegmentRef,
    label: NetLabel,
) -> Result<SchematicMutation> {
    let uuid = label.uuid();
    let old = replace(
        bus_segment_mut(p, r)?.labels_mut(),
        label,
        EntityKind::BusLabel,
    )?;
    let change = SchematicChange::BusLabelChanged {
        segment: r.segment,
        label: uuid,
    };
    record(p, r.schematic, change);
    Ok(M::UpdateBusLabel {
        segment: r,
        label: old,
    })
}

// --- Helpers ---

fn record(p: &mut Project, id: SchematicId, change: SchematicChange) {
    p.record(Change::Schematic { id, change });
}

fn schematic_index(p: &Project, id: SchematicId) -> Result<usize> {
    p.schematic_index(id).ok_or(Error::NotFound {
        kind: EntityKind::Schematic,
        uuid: id.0,
    })
}

fn schematic_ref(p: &Project, id: SchematicId) -> Result<&Schematic> {
    Ok(&p.schematics[schematic_index(p, id)?])
}

fn schematic_mut(p: &mut Project, id: SchematicId) -> Result<&mut Schematic> {
    let index = schematic_index(p, id)?;
    Ok(&mut p.schematics[index])
}

fn symbol_ref(s: &Schematic, r: SymbolRef) -> Result<&SchematicSymbol> {
    s.symbols.get(&r.symbol).ok_or(Error::NotFound {
        kind: EntityKind::Symbol,
        uuid: r.symbol.0,
    })
}

fn symbol_mut(p: &mut Project, r: SymbolRef) -> Result<&mut SchematicSymbol> {
    schematic_mut(p, r.schematic)?
        .symbols
        .get_mut(&r.symbol)
        .ok_or(Error::NotFound {
            kind: EntityKind::Symbol,
            uuid: r.symbol.0,
        })
}

fn net_segment_ref(s: &Schematic, r: NetSegmentRef) -> Result<&SchematicNetSegment> {
    s.net_segments.get(&r.segment).ok_or(Error::NotFound {
        kind: EntityKind::NetSegment,
        uuid: r.segment.0,
    })
}

fn net_segment_mut(p: &mut Project, r: NetSegmentRef) -> Result<&mut SchematicNetSegment> {
    schematic_mut(p, r.schematic)?
        .net_segments
        .get_mut(&r.segment)
        .ok_or(Error::NotFound {
            kind: EntityKind::NetSegment,
            uuid: r.segment.0,
        })
}

fn bus_segment_ref(s: &Schematic, r: BusSegmentRef) -> Result<&SchematicBusSegment> {
    s.bus_segments.get(&r.segment).ok_or(Error::NotFound {
        kind: EntityKind::BusSegment,
        uuid: r.segment.0,
    })
}

fn bus_segment_mut(p: &mut Project, r: BusSegmentRef) -> Result<&mut SchematicBusSegment> {
    schematic_mut(p, r.schematic)?
        .bus_segments
        .get_mut(&r.segment)
        .ok_or(Error::NotFound {
            kind: EntityKind::BusSegment,
            uuid: r.segment.0,
        })
}

/// Inserts `item` unless its UUID is taken (atomic).
fn insert_new<T: HasUuid>(map: &mut BTreeMap<Uuid, T>, item: T, kind: EntityKind) -> Result<()> {
    let uuid = item.uuid();
    if map.contains_key(&uuid) {
        return Err(Error::DuplicateUuid { kind, uuid });
    }
    map.insert(uuid, item);
    Ok(())
}

/// Removes the item with the given UUID (atomic).
fn take<T>(map: &mut BTreeMap<Uuid, T>, uuid: Uuid, kind: EntityKind) -> Result<T> {
    map.remove(&uuid).ok_or(Error::NotFound { kind, uuid })
}

/// Removes the items with the given UUIDs, in order (not atomic: used on
/// candidate copies only).
fn take_all<T>(map: &mut BTreeMap<Uuid, T>, uuids: &[Uuid], kind: EntityKind) -> Result<Vec<T>> {
    uuids.iter().map(|uuid| take(map, *uuid, kind)).collect()
}

/// Replaces the item with the same UUID, returns the old one (atomic).
fn replace<T: HasUuid>(map: &mut BTreeMap<Uuid, T>, item: T, kind: EntityKind) -> Result<T> {
    let uuid = item.uuid();
    let slot = map.get_mut(&uuid).ok_or(Error::NotFound { kind, uuid })?;
    Ok(std::mem::replace(slot, item))
}

// --- Typed convenience wrappers ---

impl Project {
    /// Adds a symbol to a schematic.
    pub fn add_symbol(&mut self, schematic: SchematicId, symbol: SchematicSymbol) -> Result<()> {
        self.apply(Mutation::Schematic(M::AddSymbol { schematic, symbol }))
            .map(drop)
    }

    /// Removes a symbol (its pins must not be connected) and returns it.
    pub fn remove_symbol(&mut self, symbol: SymbolRef) -> Result<SchematicSymbol> {
        match self.apply(Mutation::Schematic(M::RemoveSymbol(symbol)))? {
            Mutation::Schematic(M::AddSymbol { symbol, .. }) => Ok(symbol),
            _ => unreachable!("inverse of a removal is the addition"),
        }
    }

    /// Adds a net segment (with its elements) to a schematic.
    pub fn add_net_segment(
        &mut self,
        schematic: SchematicId,
        segment: SchematicNetSegment,
    ) -> Result<()> {
        self.apply(Mutation::Schematic(M::AddNetSegment { schematic, segment }))
            .map(drop)
    }

    /// Removes a net segment and returns it.
    pub fn remove_net_segment(&mut self, segment: NetSegmentRef) -> Result<SchematicNetSegment> {
        match self.apply(Mutation::Schematic(M::RemoveNetSegment(segment)))? {
            Mutation::Schematic(M::AddNetSegment { segment, .. }) => Ok(segment),
            _ => unreachable!("inverse of a removal is the addition"),
        }
    }

    /// Adds a bus segment (with its elements) to a schematic.
    pub fn add_bus_segment(
        &mut self,
        schematic: SchematicId,
        segment: SchematicBusSegment,
    ) -> Result<()> {
        self.apply(Mutation::Schematic(M::AddBusSegment { schematic, segment }))
            .map(drop)
    }

    /// Removes a bus segment (without attached net lines) and returns it.
    pub fn remove_bus_segment(&mut self, segment: BusSegmentRef) -> Result<SchematicBusSegment> {
        match self.apply(Mutation::Schematic(M::RemoveBusSegment(segment)))? {
            Mutation::Schematic(M::AddBusSegment { segment, .. }) => Ok(segment),
            _ => unreachable!("inverse of a removal is the addition"),
        }
    }
}
