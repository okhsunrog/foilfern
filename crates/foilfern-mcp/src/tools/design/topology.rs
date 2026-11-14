//! Reviewed connectivity edits using the editor's native cascade rules.

use std::collections::{BTreeMap, BTreeSet};

use foilfern_core::project::{
    ComponentInstanceId, ComponentSignalRef, NetSignalId, Project, SchematicId,
};
use foilfern_editor::Transaction;
use foilfern_editor::commands::{
    EditNet, NetRef, connect_design_pin, disconnect_design_signals, remove_disconnected_component,
    remove_unused_net_signals,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    error::{ErrorKind, ToolError, ToolResult},
    resolve,
};

use super::ManagedValue;

pub(super) fn rename_nets(
    tx: &mut Transaction<'_>,
    renames: &BTreeMap<String, ManagedValue>,
    diff: &mut Vec<Value>,
) -> ToolResult<()> {
    let circuit = tx.project().circuit();
    let mut seen = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut commands = Vec::new();
    for (selector, rename) in renames {
        let id = NetSignalId(resolve::required_uuid(selector, "net")?);
        if !seen.insert(id) {
            return Err(ToolError::invalid(format!(
                "Duplicate rename for net {id}."
            )));
        }
        let desired = super::super::schematic_edit::identifier(&rename.set, "net name")?;
        let net = circuit
            .net_signal(id)
            .ok_or_else(|| ToolError::not_found(format!("Net {id} not found.")))?;
        if !targets.insert(desired.clone())
            || circuit
                .net_signal_by_name(desired.as_str())
                .is_some_and(|(owner, _)| owner != id)
        {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                format!(
                    "Net name {desired:?} is already in use or requested by more than one net."
                ),
            ));
        }
        let current = net.name().as_str();
        if current == desired.as_str() {
            continue;
        }
        if current != rename.expected {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                format!(
                    "Net {id}: expected {:?}, found {current:?}.",
                    rename.expected
                ),
            ));
        }
        diff.push(json!({"field":"net_name","uuid":id,"before":current,"after":desired.as_str()}));
        commands.push(EditNet {
            net: NetRef::Id(id),
            name: Some(desired),
            net_class: None,
            merge: false,
        });
    }
    for command in commands {
        tx.run(command)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ManagedNet {
    /// Baseline net name, or null for an unconnected pin.
    #[serde(deserialize_with = "explicit_net")]
    expected: Option<String>,
    /// Desired net name, or null to disconnect.
    #[serde(deserialize_with = "explicit_net")]
    set: Option<String>,
}

fn explicit_net<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(Default)]
pub(super) struct TopologyEdits {
    removed: BTreeSet<ComponentInstanceId>,
    reassigned: Vec<(ComponentSignalRef, Option<NetSignalId>, Option<NetSignalId>)>,
    previous_nets: BTreeSet<NetSignalId>,
}

impl TopologyEdits {
    pub(super) fn remove_unused_nets(
        &self,
        tx: &mut Transaction<'_>,
        required: &[String],
    ) -> ToolResult<()> {
        let circuit = tx.project().circuit();
        let unused_candidates: Vec<_> = self
            .previous_nets
            .iter()
            .copied()
            .filter(|id| {
                circuit
                    .net_signal(*id)
                    .is_some_and(|net| !required.iter().any(|name| name == net.name().as_str()))
            })
            .collect();
        remove_unused_net_signals(tx, unused_candidates)?;
        Ok(())
    }
}

fn signal_nets(p: &Project) -> impl Iterator<Item = (ComponentSignalRef, Option<NetSignalId>)> {
    p.circuit()
        .component_instances()
        .iter()
        .flat_map(|(&component, instance)| {
            instance.signals().iter().map(move |(&signal, state)| {
                (ComponentSignalRef { component, signal }, state.net())
            })
        })
}

fn verify_connections(
    p: &Project,
    expected: Vec<(ComponentSignalRef, Option<NetSignalId>)>,
) -> ToolResult<()> {
    for (signal, net) in expected {
        if p.circuit()
            .component_instance(signal.component)
            .and_then(|component| component.signal(&signal.signal))
            .is_none_or(|state| state.net() != net)
        {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                format!(
                    "Cannot preserve the requested connectivity on {}. Review the shared segment and its labels.",
                    resolve::signal_label(p, signal),
                ),
            ));
        }
    }
    Ok(())
}

pub(super) fn remove_components(
    tx: &mut Transaction<'_>,
    targets: &BTreeSet<ComponentInstanceId>,
    diff: &mut Vec<Value>,
    topology: &mut TopologyEdits,
) -> ToolResult<()> {
    if targets.is_empty() {
        return Ok(());
    }
    let (removed, retained): (Vec<_>, Vec<_>) =
        signal_nets(tx.project()).partition(|(signal, _)| targets.contains(&signal.component));
    let signals = removed
        .into_iter()
        .filter_map(|(signal, net)| {
            topology.previous_nets.insert(net?);
            Some(signal)
        })
        .collect::<Vec<_>>();
    disconnect_design_signals(tx, &signals)?;
    for &id in targets {
        if let Some(component) = tx.project().circuit().component_instance(id) {
            diff.push(
                json!({"field":"remove_component","component":component.name().as_str(),"uuid":id}),
            );
            remove_disconnected_component(tx, id)?;
            topology.removed.insert(id);
        }
    }
    verify_connections(tx.project(), retained)
}

pub(super) fn set_pin_nets(
    tx: &mut Transaction<'_>,
    page: SchematicId,
    bindings: &BTreeMap<&str, ComponentInstanceId>,
    edits: &BTreeMap<String, ManagedNet>,
    diff: &mut Vec<Value>,
    topology: &mut TopologyEdits,
) -> ToolResult<()> {
    if edits.is_empty() {
        return Ok(());
    }
    let p = tx.project();
    let mut seen = BTreeSet::new();
    let mut changes = Vec::new();
    for (address, edit) in edits {
        let (key, pin) = resolve::split_pin_ref(address)?;
        let id = bindings
            .get(key)
            .ok_or_else(|| ToolError::invalid(format!("Unknown component key {key:?}.")))?;
        let resolved = resolve::pin(p, &format!("{id}.{pin}"))?;
        if !seen.insert(resolved.signal) {
            return Err(ToolError::invalid(format!(
                "Multiple edits target {}.",
                resolved.label
            )));
        }
        let old = p
            .circuit()
            .component_instance(*id)
            .and_then(|part| part.signal(&resolved.signal.signal))
            .and_then(|pin| pin.net());
        let current = old
            .and_then(|id| p.circuit().net_signal(id))
            .map(|net| net.name().as_str());
        if current == edit.set.as_deref() {
            continue;
        }
        if current != edit.expected.as_deref() {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                format!(
                    "{address}: expected {:?}, found {current:?}.",
                    edit.expected
                ),
            ));
        }
        let target = edit
            .set
            .as_deref()
            .map(|name| {
                p.circuit()
                    .net_signal_by_name(name)
                    .map(|(id, _)| id)
                    .ok_or_else(|| ToolError::not_found(format!("Net {name:?} not found.")))
            })
            .transpose()?;
        changes.push((resolved.signal, old, target));
        diff.push(json!({"field":"pin_net","pin":address,"signal":resolved.signal,"before":current,"after":edit.set}));
    }
    if changes.is_empty() {
        return Ok(());
    }
    let targets: BTreeMap<_, _> = changes
        .iter()
        .map(|&(signal, _, net)| (signal, net))
        .collect();
    let expected = signal_nets(p)
        .map(|(signal, old)| (signal, targets.get(&signal).copied().unwrap_or(old)))
        .collect();
    let disconnected = changes
        .iter()
        .filter(|(_, old, _)| old.is_some())
        .map(|(signal, _, _)| *signal)
        .collect::<Vec<_>>();
    disconnect_design_signals(tx, &disconnected)?;
    for &(signal, old, target) in &changes {
        if let Some(target) = target {
            connect_design_pin(tx, page, signal, target, false)?;
        }
        topology.previous_nets.extend(old);
    }
    verify_connections(tx.project(), expected)?;
    topology.reassigned = changes;
    Ok(())
}

fn affected<K: Ord + Copy, V: PartialEq>(
    before: &BTreeMap<K, V>,
    after: &BTreeMap<K, V>,
) -> (Vec<K>, Vec<K>) {
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for (&key, value) in before {
        match after.get(&key) {
            None => removed.push(key),
            Some(after) if after != value => changed.push(key),
            Some(_) => {}
        }
    }
    (removed, changed)
}

pub(super) fn copper_impact(
    before: &Project,
    after: &Project,
    topology: &TopologyEdits,
) -> Vec<Value> {
    if topology.removed.is_empty() && topology.reassigned.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::new();
    for board in before.boards() {
        let Some(next) = after.board(board.id()) else {
            continue;
        };
        let traces = |board: &foilfern_core::project::board::Board| {
            board
                .net_segments()
                .values()
                .flat_map(|segment| {
                    segment
                        .traces()
                        .iter()
                        .map(move |(&id, trace)| (id, (trace.clone(), segment.net())))
                })
                .collect::<BTreeMap<_, _>>()
        };
        let vias = |board: &foilfern_core::project::board::Board| {
            board
                .net_segments()
                .values()
                .flat_map(|segment| {
                    segment
                        .vias()
                        .iter()
                        .map(move |(&id, via)| (id, (via.clone(), segment.net())))
                })
                .collect::<BTreeMap<_, _>>()
        };
        let (removed_traces, changed_traces) = affected(&traces(board), &traces(next));
        let (removed_vias, changed_vias) = affected(&vias(board), &vias(next));
        let (removed_planes, changed_planes) = affected(board.planes(), next.planes());
        let removed_devices = board
            .devices()
            .keys()
            .filter(|id| !next.devices().contains_key(id))
            .collect::<Vec<_>>();

        let mut affected_nets = BTreeSet::new();
        for &(signal, old, new) in &topology.reassigned {
            if board.devices().contains_key(&signal.component) {
                affected_nets.extend([old, new].into_iter().flatten());
            }
        }
        for &id in &topology.removed {
            if board.devices().contains_key(&id)
                && let Some(component) = before.circuit().component_instance(id)
            {
                affected_nets.extend(
                    component
                        .signals()
                        .values()
                        .filter_map(|signal| signal.net()),
                );
            }
        }
        let refill_planes = board
            .planes()
            .iter()
            .filter_map(|(id, plane)| {
                (next.planes().contains_key(id)
                    && plane.net().is_some_and(|net| affected_nets.contains(&net)))
                .then_some(id)
            })
            .collect::<Vec<_>>();
        if removed_traces.is_empty()
            && changed_traces.is_empty()
            && removed_vias.is_empty()
            && changed_vias.is_empty()
            && removed_planes.is_empty()
            && changed_planes.is_empty()
            && removed_devices.is_empty()
            && refill_planes.is_empty()
        {
            continue;
        }
        result.push(json!({
            "board": board.id(), "name":board.name().as_str(),
            "removed_traces":removed_traces,"changed_traces":changed_traces,
            "removed_vias":removed_vias,"changed_vias":changed_vias,
            "removed_planes":removed_planes,"changed_planes":changed_planes,
            "removed_devices":removed_devices,"planes_on_affected_nets":refill_planes,
        }));
    }
    result
}
