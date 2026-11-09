//! Resolution of the entity references in command parameters (no upstream
//! counterpart: upstream gets the entities from the selection in the UI).

use librepcb_core::library::LibraryBaseElement;
use librepcb_core::project::board::Board;
use librepcb_core::project::schematic::Schematic;
use librepcb_core::project::{
    BoardId, ComponentInstanceId, NetSegmentId, NetSignalId, Project, SchematicId, SymbolId,
};
use librepcb_core::types::{Point, Uuid};

use super::{ComponentRef, NetRef, PadRef, PinRef};
use crate::error::{Error, Result};

/// Returns the component instance referenced by `r`.
pub(crate) fn component(p: &Project, r: &ComponentRef) -> Result<ComponentInstanceId> {
    match r {
        ComponentRef::Id(id) => p
            .circuit()
            .component_instance(*id)
            .map(|_| *id)
            .ok_or_else(|| Error::not_found("Component", id)),
        ComponentRef::Name(name) => p
            .circuit()
            .component_instance_by_name(name)
            .map(|(id, _)| id)
            .ok_or_else(|| Error::not_found("Component", name)),
    }
}

/// Returns the net signal referenced by `r`, if it exists.
pub(crate) fn net(p: &Project, r: &NetRef) -> Option<NetSignalId> {
    match r {
        NetRef::Id(id) => p.circuit().net_signal(*id).map(|_| *id),
        NetRef::Name(name) => p.circuit().net_signal_by_name(name).map(|(id, _)| id),
    }
}

/// Returns the net signal referenced by `r` or a "not found" error.
pub(crate) fn required_net(p: &Project, r: &NetRef) -> Result<NetSignalId> {
    net(p, r).ok_or_else(|| {
        Error::not_found(
            "Net",
            match r {
                NetRef::Id(id) => id.to_string(),
                NetRef::Name(name) => name.clone(),
            },
        )
    })
}

/// Returns the schematic, or the first one if `id` is `None`.
pub(crate) fn schematic(p: &Project, id: Option<SchematicId>) -> Result<&Schematic> {
    match id {
        Some(id) => p
            .schematic(id)
            .ok_or_else(|| Error::not_found("Schematic", id)),
        None => p
            .schematics()
            .first()
            .ok_or_else(|| Error::not_found("Schematic", "<first>")),
    }
}

/// Returns the board, or the primary board if `id` is `None`.
pub(crate) fn board(p: &Project, id: Option<BoardId>) -> Result<&Board> {
    match id {
        Some(id) => p.board(id).ok_or_else(|| Error::not_found("Board", id)),
        None => p
            .primary_board()
            .ok_or_else(|| Error::not_found("Board", "<primary>")),
    }
}

/// A resolved symbol pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedPin {
    pub schematic: SchematicId,
    pub symbol: SymbolId,
    pub pin: Uuid,
    pub position: Point,
    pub rotation: librepcb_core::types::Angle,
}

/// Resolves a pin reference to a connected pin of a placed symbol.
pub(crate) fn pin(p: &Project, r: &PinRef) -> Result<ResolvedPin> {
    let component = component(p, &r.component)?;
    let uses = p
        .component_uses(component)
        .filter(|u| !u.symbols.is_empty())
        .ok_or_else(|| {
            Error::InvalidArgument(format!(
                "The component \"{}\" has no symbols in the schematics.",
                component_name(p, component)
            ))
        })?;
    let mut by_uuid = Vec::new();
    let mut by_pin_name = Vec::new();
    let mut by_signal_name = Vec::new();
    for (schematic_id, symbol_id) in uses.symbols.values() {
        let Some(schematic) = p.schematic(*schematic_id) else {
            continue;
        };
        let Some(symbol) = schematic.symbols().get(symbol_id) else {
            continue;
        };
        for view in symbol.pins(p.view())? {
            let resolved = ResolvedPin {
                schematic: *schematic_id,
                symbol: *symbol_id,
                pin: view.uuid(),
                position: view.position(),
                rotation: view.rotation(),
            };
            if view.uuid().to_string() == r.pin {
                by_uuid.push((resolved, view.signal()));
            }
            if view.lib_pin().name().as_str() == r.pin {
                by_pin_name.push((resolved, view.signal()));
            }
            if view.lib_signal().name().as_str() == r.pin {
                by_signal_name.push((resolved, view.signal()));
            }
        }
    }
    for matches in [by_uuid, by_pin_name, by_signal_name] {
        if let Some((first, signal)) = matches.first() {
            if matches.iter().any(|(_, s)| s != signal) {
                return Err(Error::Ambiguous {
                    kind: "Pin",
                    id: format!("{}:{}", component_name(p, component), r.pin),
                });
            }
            return Ok(*first);
        }
    }
    Err(Error::not_found(
        "Pin",
        format!("{}:{}", component_name(p, component), r.pin),
    ))
}

/// A resolved footprint pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedPad {
    pub component: ComponentInstanceId,
    pub pad: Uuid,
}

/// Resolves a pad reference on `board`.
pub(crate) fn pad(p: &Project, board: &Board, r: &PadRef) -> Result<ResolvedPad> {
    let component = component(p, &r.component)?;
    let device = board.device(component).ok_or_else(|| {
        Error::not_found(
            "Device",
            format!("{} on board {}", component_name(p, component), board.name()),
        )
    })?;
    let pads = device.pads(p.library(), p.circuit())?;
    let lib_component = p
        .circuit()
        .component_instance(component)
        .and_then(|c| p.library().component(&c.lib_component()));
    let signal_name = |signal: Option<librepcb_core::project::ComponentSignalRef>| {
        signal
            .and_then(|s| lib_component?.signals().by_uuid(&s.signal))
            .map(|s| s.name().to_string())
    };
    let found = pads
        .iter()
        .find(|v| v.uuid().to_string() == r.pad)
        .or_else(|| {
            pads.iter().find(|v| {
                v.package_pad()
                    .is_some_and(|pp| pp.name().as_str() == r.pad)
            })
        })
        .or_else(|| {
            pads.iter()
                .find(|v| signal_name(v.component_signal()).as_deref() == Some(r.pad.as_str()))
        })
        .ok_or_else(|| {
            Error::not_found("Pad", format!("{}:{}", component_name(p, component), r.pad))
        })?;
    Ok(ResolvedPad {
        component,
        pad: found.uuid(),
    })
}

/// Returns the name of a component for messages.
pub(crate) fn component_name(p: &Project, id: ComponentInstanceId) -> String {
    p.circuit()
        .component_instance(id)
        .map_or_else(|| id.to_string(), |c| c.name().to_string())
}

/// Returns the name of a net for messages.
pub(crate) fn net_name(p: &Project, id: Option<NetSignalId>) -> String {
    id.and_then(|id| p.circuit().net_signal(id))
        .map(|n| n.name().to_string())
        .unwrap_or_default()
}

/// Returns the name of the library component of a component instance.
#[allow(dead_code)]
pub(crate) fn lib_component_name(p: &Project, id: ComponentInstanceId) -> String {
    p.circuit()
        .component_instance(id)
        .and_then(|c| p.library().component(&c.lib_component()))
        .map(|c| c.metadata().name().to_string())
        .unwrap_or_default()
}

/// Returns the schematic net segments of a net (all schematics).
pub(crate) fn schematic_segments_of_net(
    p: &Project,
    net: NetSignalId,
) -> Vec<(SchematicId, NetSegmentId)> {
    p.net_signal_uses(net)
        .filter_map(|u| match u {
            librepcb_core::project::NetUse::SchematicSegment(s, seg) => Some((*s, *seg)),
            _ => None,
        })
        .collect()
}
