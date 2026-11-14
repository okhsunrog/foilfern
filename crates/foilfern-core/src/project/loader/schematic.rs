//! Schematic part of the project loader (upstream
//! `ProjectLoader::loadSchematics()`, `loadSchematic()`,
//! `loadSchematicSymbol()`, `loadSchematicBusSegment()`,
//! `loadSchematicNetSegment()`, `loadSchematicUserSettings()`).
//!
//! Differences to upstream: each schematic is deserialized into a complete
//! value first and then added to the project in one step, which validates
//! all items (references into the circuit and library, net line anchors,
//! cohesion) with the upstream messages; upstream adds the empty page
//! first and then item by item. Duplicate item UUIDs in the file are
//! rejected like upstream's `add*()` methods do.

use std::collections::BTreeMap;

use super::{parse_file, split_index_path};
use crate::geometry::{Image, Junction, NetLabel, NetLine, Polygon, Text};
use crate::project::Project;
use crate::project::error::{EntityKind, Error, Result};
use crate::project::schematic::{
    Schematic, SchematicBusSegment, SchematicNetSegment, SchematicSymbol,
};
use crate::serialization::{DeserializeObject, HasUuid, SExpression};
use crate::types::{Point, Uuid};

pub(super) fn load_schematics(p: &mut Project) -> Result<()> {
    let index = parse_file(p, "schematics/schematics.lp")?;
    for node in index.children_named("schematic") {
        let relative: String = node.child_value("@0")?;
        load_schematic(p, &relative)?;
    }
    log::debug!("Successfully loaded {} schematics.", p.schematics.len());
    Ok(())
}

fn load_schematic(p: &mut Project, relative_file_path: &str) -> Result<()> {
    let (dir, directory_name) = split_index_path(relative_file_path);
    let root = parse_file(p, relative_file_path)?;
    let mut schematic = Schematic::new(
        root.child_value("@0")?,
        root.child_value("name/@0")?,
        directory_name,
    );
    schematic.set_grid_interval(root.child_value("grid/interval/@0")?);
    schematic.set_grid_unit(root.child_value("grid/unit/@0")?);
    for node in root.children_named("symbol") {
        let symbol = load_symbol(node)?;
        insert_unique(
            &mut schematic.symbols,
            symbol.id(),
            symbol,
            EntityKind::Symbol,
        )?;
    }
    for node in root.children_named("bussegment") {
        let segment = load_bus_segment(node)?;
        insert_unique(
            &mut schematic.bus_segments,
            segment.id(),
            segment,
            EntityKind::BusSegment,
        )?;
    }
    for node in root.children_named("netsegment") {
        let segment = load_net_segment(node)?;
        insert_unique(
            &mut schematic.net_segments,
            segment.id(),
            segment,
            EntityKind::NetSegment,
        )?;
    }
    load_items::<Polygon>(
        &root,
        "polygon",
        &mut schematic.polygons,
        EntityKind::SchematicPolygon,
    )?;
    load_items::<Text>(
        &root,
        "text",
        &mut schematic.texts,
        EntityKind::SchematicText,
    )?;
    load_items::<Image>(
        &root,
        "image",
        &mut schematic.images,
        EntityKind::SchematicImage,
    )?;
    p.add_schematic(schematic, None)?;
    load_schematic_user_settings(p, &dir);
    Ok(())
}

fn load_symbol(node: &SExpression) -> Result<SchematicSymbol> {
    let mut symbol = SchematicSymbol::new(
        node.child_value("@0")?,
        node.child_value("component/@0")?,
        node.child_value("lib_gate/@0")?,
        Point::deserialize(node.required_child("position")?)?,
        node.child_value("rotation/@0")?,
        node.child_value("mirror/@0")?,
    );
    for child in node.children_named("text") {
        let text = Text::deserialize(child)?;
        if symbol.texts().contains_key(&text.uuid()) {
            return Err(Error::DuplicateUuid {
                kind: EntityKind::SchematicText,
                uuid: text.uuid(),
            });
        }
        symbol.insert_text(text);
    }
    Ok(symbol)
}

fn load_bus_segment(node: &SExpression) -> Result<SchematicBusSegment> {
    let mut segment =
        SchematicBusSegment::new(node.child_value("@0")?, node.child_value("bus/@0")?);
    load_items::<Junction>(
        node,
        "junction",
        segment.junctions_mut(),
        EntityKind::BusJunction,
    )?;
    load_items::<NetLine>(node, "line", segment.lines_mut(), EntityKind::BusLine)?;
    load_items::<NetLabel>(node, "label", segment.labels_mut(), EntityKind::BusLabel)?;
    Ok(segment)
}

fn load_net_segment(node: &SExpression) -> Result<SchematicNetSegment> {
    let mut segment =
        SchematicNetSegment::new(node.child_value("@0")?, node.child_value("net/@0")?);
    load_items::<Junction>(
        node,
        "junction",
        segment.junctions_mut(),
        EntityKind::NetPoint,
    )?;
    load_items::<NetLine>(node, "line", segment.lines_mut(), EntityKind::NetLine)?;
    load_items::<NetLabel>(node, "label", segment.labels_mut(), EntityKind::NetLabel)?;
    Ok(segment)
}

/// Deserializes all children `name` of `node` into `map`, rejecting
/// duplicate UUIDs.
fn load_items<T: DeserializeObject + HasUuid>(
    node: &SExpression,
    name: &str,
    map: &mut BTreeMap<Uuid, T>,
    kind: EntityKind,
) -> Result<()> {
    for child in node.children_named(name) {
        let item = T::deserialize(child)?;
        insert_unique(map, item.uuid(), item, kind)?;
    }
    Ok(())
}

fn insert_unique<K: Ord + Copy + Into<Uuid>, V>(
    map: &mut BTreeMap<K, V>,
    key: K,
    value: V,
    kind: EntityKind,
) -> Result<()> {
    if map.contains_key(&key) {
        return Err(Error::DuplicateUuid {
            kind,
            uuid: key.into(),
        });
    }
    map.insert(key, value);
    Ok(())
}

fn load_schematic_user_settings(p: &Project, dir: &str) {
    // upstream: parses `settings.user.lp` (currently without content) and
    // uses defaults on error.
    if let Err(e) = parse_file(p, &format!("{dir}/settings.user.lp")) {
        log::error!("Could not load schematic user settings, defaults will be used instead: {e}");
    }
}
