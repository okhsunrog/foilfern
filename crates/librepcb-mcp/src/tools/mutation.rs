//! The raw escape hatch: `mutation_apply` and `mutation_schema`.
//!
//! `mutation_apply` takes a core [`Mutation`] in its serde JSON form
//! (integer nanometers, microdegrees, UUID strings; see
//! `docs/project-model-design.md`, "Serde representation"), validates it
//! with [`Project::apply()`] (check-then-commit: a failed mutation changes
//! nothing) and keeps the inverse for undo.
//!
//! The core types do not implement `JsonSchema`, so `mutation_schema`
//! describes the wire form by conventions, the variant list and examples
//! generated from real values (compile-checked, and verified to
//! deserialize in the tests), and can return the prefilled update
//! mutation of an existing entity.

use librepcb_core::project::circuit::NetSignal;
use librepcb_core::project::{
    BoardMutation, ComponentInstanceId, DeviceRef, Mutation, NetClassId, NetSignalId, PlaneRef,
    SchematicMutation, SymbolRef,
};
use librepcb_core::types::{Angle, CircuitIdentifier, Point, Uuid};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::resolve::{self, parse_uuid};
use crate::session::Session;
use crate::tools::write::write;
use librepcb_editor::commands::ApplyMutations;

/// Arguments of `mutation_apply`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MutationApplyArgs {
    /// The mutation in the core serde JSON form (externally tagged enum,
    /// e.g. `{"SetComponentSignalNet": {"signal": {"component": "<uuid>",
    /// "signal": "<uuid>"}, "net": "<uuid>"}}`); see mutation_schema.
    pub mutation: Value,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `mutation_schema`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MutationSchemaArgs {
    /// Return the prefilled update mutation of this entity: a designator,
    /// net name, or UUID of a component, net, net class, schematic,
    /// board, symbol, device (component UUID) or plane.
    #[serde(default)]
    pub entity: Option<String>,
}

/// `mutation_apply`.
pub fn mutation_apply(session: &mut Session, args: MutationApplyArgs) -> ToolResult<ToolOutput> {
    let mutation: Mutation = serde_json::from_value(args.mutation).map_err(|e| {
        ToolError::invalid(format!(
            "The mutation does not match the core Mutation format ({e}); see mutation_schema."
        ))
    })?;
    let done = write(
        session,
        "mutation_apply",
        args.expected_revision,
        |editor| {
            Ok(editor.execute(ApplyMutations {
                text: None,
                mutations: vec![mutation],
            })?)
        },
    )?;
    let open = session.project()?;
    done.output(
        open,
        format!(
            "Mutation applied: revision {} -> {}.",
            done.revision_before,
            open.project().revision()
        ),
        json!({}),
    )
}

const CONVENTIONS: &str = "Externally tagged enums: {\"Variant\": payload} (unit variants as \
\"Variant\"). Lengths are integer nanometers (1 mm = 1000000), angles integer microdegrees \
(90 deg = 90000000), points {\"x\": nm, \"y\": nm} (y upwards), UUIDs and typed ids \
(NetSignalId, ComponentInstanceId, ...) are UUID strings, names are validated strings, layers \
are ids like \"top_cu\", \"bot_cu\", \"brd_outlines\". Nested entity paths: ComponentSignalRef \
{component, signal}, SymbolRef {schematic, symbol}, NetSegmentRef {schematic, segment}, \
DeviceRef {board, component}, BoardNetSegmentRef {board, segment}, PlaneRef {board, plane}. \
Removals fail while the entity is in use (no cascade); Batch applies a list atomically.";

/// The mutation variants with a short description.
const VARIANTS: &[(&str, &str)] = &[
    (
        "SetProjectMetadata",
        "replace name, author, version, created, attributes",
    ),
    (
        "SetProjectSettings",
        "replace locale/norm order, custom BOM attributes",
    ),
    ("SetOutputJobs", "replace all output jobs"),
    ("SetErcApprovals", "replace all ERC approvals"),
    (
        "SetErcApproval",
        "{approval, approved}: approve one ERC message",
    ),
    ("AddAssemblyVariant", "{variant, index}"),
    ("RemoveAssemblyVariant", "AssemblyVariantId"),
    ("UpdateAssemblyVariant", "AssemblyVariant"),
    ("AddNetClass", "NetClass"),
    ("RemoveNetClass", "NetClassId"),
    ("UpdateNetClass", "NetClass"),
    (
        "AddNetSignal",
        "NetSignal {uuid, name, auto_name, net_class}",
    ),
    ("RemoveNetSignal", "NetSignalId (fails while used)"),
    ("UpdateNetSignal", "NetSignal"),
    ("AddBus", "Bus"),
    ("RemoveBus", "BusId"),
    ("UpdateBus", "Bus"),
    (
        "AddComponentInstance",
        "ComponentInstance (library component must be in the project library)",
    ),
    (
        "RemoveComponentInstance",
        "ComponentInstanceId (fails while placed)",
    ),
    (
        "UpdateComponentInstance",
        "ComponentInstanceProperties (name, value, attributes, ...)",
    ),
    (
        "SetComponentSignalNet",
        "{signal: ComponentSignalRef, net: NetSignalId|null}",
    ),
    ("AddSchematic", "{schematic, index}"),
    ("RemoveSchematic", "SchematicId (must be empty)"),
    ("UpdateSchematic", "SchematicProperties"),
    (
        "Schematic",
        "SchematicMutation: AddSymbol, RemoveSymbol, SetSymbolPlacement, \
      AddSymbolText, RemoveSymbolText, UpdateSymbolText, AddNetSegment, RemoveNetSegment, \
      SetNetSegmentNet, AddNetSegmentElements, RemoveNetSegmentElements, SetNetPointPosition, \
      SetNetLineWidth, AddNetLabel, RemoveNetLabel, UpdateNetLabel, AddBusSegment, \
      RemoveBusSegment, SetBusSegmentBus, AddBusSegmentElements, RemoveBusSegmentElements, \
      SetBusJunctionPosition, SetBusLineWidth, AddBusLabel, RemoveBusLabel, UpdateBusLabel, \
      AddPolygon, RemovePolygon, UpdatePolygon, AddText, RemoveText, UpdateText, AddImage, \
      RemoveImage, UpdateImage",
    ),
    ("AddBoard", "{board, index}"),
    ("RemoveBoard", "BoardId"),
    ("UpdateBoard", "BoardProperties"),
    (
        "Board",
        "BoardMutation: SetSettings, SetDrcApprovals, SetDrcApproval, \
      SetLayersVisibility, AddDevice, RemoveDevice, UpdateDevice, AddNetSegment, \
      RemoveNetSegment, SetNetSegmentNet, AddNetSegmentElements, RemoveNetSegmentElements, \
      UpdateNetSegmentElements, AddPlane, RemovePlane, UpdatePlane, AddItem, RemoveItem, \
      UpdateItem",
    ),
    (
        "Batch",
        "[Mutation, ...] applied in order, rolled back on error",
    ),
];

/// Example mutations built from real values (with placeholder UUIDs).
pub fn examples() -> Vec<(&'static str, Mutation)> {
    let uuid = |n: u8| {
        format!("00000000-0000-4000-8000-{n:012}")
            .parse::<Uuid>()
            .unwrap_or_else(|_| Uuid::new_random())
    };
    let mut out = Vec::new();
    if let Ok(name) = CircuitIdentifier::new("VCC") {
        out.push((
            "add a net named VCC in the net class 3",
            Mutation::AddNetSignal(NetSignal::new(uuid(1), NetClassId(uuid(3)), name, false)),
        ));
    }
    out.push((
        "connect signal 5 of component 4 to net 1 (net: null disconnects)",
        Mutation::SetComponentSignalNet {
            signal: librepcb_core::project::ComponentSignalRef {
                component: ComponentInstanceId(uuid(4)),
                signal: uuid(5),
            },
            net: Some(NetSignalId(uuid(1))),
        },
    ));
    out.push((
        "move symbol 7 on schematic 6 to (10.16 mm, 5.08 mm), rotated 90 deg",
        Mutation::Schematic(SchematicMutation::SetSymbolPlacement {
            symbol: SymbolRef {
                schematic: librepcb_core::project::SchematicId(uuid(6)),
                symbol: librepcb_core::project::SymbolId(uuid(7)),
            },
            position: Point::from_nm(10_160_000, 5_080_000),
            rotation: Angle::new(90_000_000),
            mirrored: false,
        }),
    ));
    out.push((
        "remove device of component 4 from board 8",
        Mutation::Board(BoardMutation::RemoveDevice(DeviceRef {
            board: librepcb_core::project::BoardId(uuid(8)),
            component: ComponentInstanceId(uuid(4)),
        })),
    ));
    out.push((
        "remove plane 9 of board 8",
        Mutation::Board(BoardMutation::RemovePlane(PlaneRef {
            board: librepcb_core::project::BoardId(uuid(8)),
            plane: librepcb_core::project::PlaneId(uuid(9)),
        })),
    ));
    out
}

/// `mutation_schema`.
pub fn mutation_schema(session: &Session, args: MutationSchemaArgs) -> ToolResult<ToolOutput> {
    let examples: Vec<Value> = examples()
        .into_iter()
        .map(|(what, m)| json!({ "description": what, "mutation": m }))
        .collect();
    let variants: Vec<Value> = VARIANTS
        .iter()
        .map(|(n, d)| json!({ "variant": n, "payload": d }))
        .collect();
    let mut result = json!({
        "conventions": CONVENTIONS,
        "variants": variants,
        "examples": examples,
    });
    let mut summary = format!(
        "{} mutation variants; pass entity=<designator|net|uuid> to get a prefilled update \
         mutation.",
        VARIANTS.len()
    );
    if let Some(entity) = args.entity.as_deref() {
        let (kind, update) = entity_update(session, entity)?;
        summary = format!("Prefilled update mutation of the {kind} \"{entity}\".");
        result["entity"] = json!({ "kind": kind, "update_mutation": update });
    }
    ToolOutput::new(summary, result)
}

fn entity_update(session: &Session, key: &str) -> ToolResult<(&'static str, Mutation)> {
    let p = session.project()?.project();
    if let Ok((_, c)) = resolve::component(p, key) {
        return Ok((
            "component",
            Mutation::UpdateComponentInstance(c.properties()),
        ));
    }
    if let Ok((_, n)) = resolve::net(p, key) {
        return Ok(("net", Mutation::UpdateNetSignal(n.clone())));
    }
    let uuid = parse_uuid(key)
        .ok_or_else(|| ToolError::not_found(format!("There is no entity \"{key}\".")))?;
    if let Some(nc) = p.circuit().net_class(NetClassId(uuid)) {
        return Ok(("net class", Mutation::UpdateNetClass(nc.clone())));
    }
    for s in p.schematics() {
        if s.uuid() == uuid {
            return Ok(("schematic", Mutation::UpdateSchematic(s.properties())));
        }
        if let Some(sym) = s.symbols().get(&librepcb_core::project::SymbolId(uuid)) {
            return Ok((
                "symbol",
                Mutation::Schematic(SchematicMutation::SetSymbolPlacement {
                    symbol: SymbolRef {
                        schematic: s.id(),
                        symbol: sym.id(),
                    },
                    position: sym.position(),
                    rotation: sym.rotation(),
                    mirrored: sym.mirrored(),
                }),
            ));
        }
    }
    for b in p.boards() {
        if b.uuid() == uuid {
            return Ok(("board", Mutation::UpdateBoard(b.properties())));
        }
        if let Some(pl) = b.plane(librepcb_core::project::PlaneId(uuid)) {
            return Ok((
                "plane",
                Mutation::Board(BoardMutation::UpdatePlane {
                    board: b.id(),
                    plane: pl.clone(),
                }),
            ));
        }
    }
    Err(ToolError::not_found(format!(
        "There is no entity \"{key}\"."
    )))
}
