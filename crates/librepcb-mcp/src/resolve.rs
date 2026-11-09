//! Addressing of project entities by what an agent sees (no upstream
//! counterpart): designators (`"R1"`), pins (`"R1.1"`, `"U1.VCC"`), net
//! names, schematic/board names or indices; UUIDs are accepted everywhere
//! as an alternative.
//!
//! Pin references are `<designator>.<pin>` where `<pin>` is the name of a
//! component signal (`U1.VCC`), or the name of a package pad of the
//! component's device (`R1.1`, resolved through the device's pad-signal
//! map). Designators and signal names are circuit identifiers, which
//! cannot contain `.`, so the reference is split at the first dot. A
//! designator alone (`GND1`) addresses the only signal of a single-pin
//! component such as a supply symbol.

use std::str::FromStr;

use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cmp::Component;
use librepcb_core::project::board::{Board, BoardDevice};
use librepcb_core::project::circuit::{ComponentInstance, NetSignal};
use librepcb_core::project::schematic::Schematic;
use librepcb_core::project::{
    BoardId, ComponentInstanceId, ComponentSignalRef, NetSignalId, Project, SchematicId,
};
use librepcb_core::types::Uuid;

use crate::error::{ToolError, ToolResult};

/// Parses `s` as UUID, if it is one.
pub fn parse_uuid(s: &str) -> Option<Uuid> {
    Uuid::from_str(s.trim()).ok()
}

/// Parses a required UUID argument.
pub fn required_uuid(s: &str, what: &str) -> ToolResult<Uuid> {
    parse_uuid(s).ok_or_else(|| ToolError::invalid(format!("invalid {what} UUID: \"{s}\"")))
}

/// Resolves a component instance by designator (`"R1"`) or UUID.
pub fn component<'a>(
    project: &'a Project,
    key: &str,
) -> ToolResult<(ComponentInstanceId, &'a ComponentInstance)> {
    let key = key.trim();
    let circuit = project.circuit();
    if let Some(found) = circuit.component_instance_by_name(key) {
        return Ok(found);
    }
    if let Some(uuid) = parse_uuid(key) {
        let id = ComponentInstanceId(uuid);
        if let Some(c) = circuit.component_instance(id) {
            return Ok((id, c));
        }
    }
    // Designators are case sensitive, but help with a case-insensitive
    // match if it is unambiguous.
    let mut matches = circuit
        .component_instances()
        .iter()
        .filter(|(_, c)| c.name().as_str().eq_ignore_ascii_case(key));
    if let (Some((id, c)), None) = (matches.next(), matches.next()) {
        return Ok((*id, c));
    }
    Err(ToolError::not_found(format!(
        "There is no component \"{key}\" (use component_list to see all designators)."
    )))
}

/// Resolves a net by name or UUID.
pub fn net<'a>(project: &'a Project, key: &str) -> ToolResult<(NetSignalId, &'a NetSignal)> {
    let key = key.trim();
    let circuit = project.circuit();
    if let Some(found) = circuit.net_signal_by_name(key) {
        return Ok(found);
    }
    if let Some(uuid) = parse_uuid(key) {
        let id = NetSignalId(uuid);
        if let Some(n) = circuit.net_signal(id) {
            return Ok((id, n));
        }
    }
    Err(ToolError::not_found(format!(
        "There is no net \"{key}\" (use net_list to see all nets)."
    )))
}

/// Resolves a schematic page by name, index (`"0"`) or UUID; `None` is the
/// first page.
pub fn schematic<'a>(
    project: &'a Project,
    key: Option<&str>,
) -> ToolResult<(usize, SchematicId, &'a Schematic)> {
    let pages = project.schematics();
    let index = match key.map(str::trim).filter(|k| !k.is_empty()) {
        None => (!pages.is_empty()).then_some(0),
        Some(key) => pages
            .iter()
            .position(|s| s.name().as_str() == key)
            .or_else(|| {
                let uuid = parse_uuid(key)?;
                pages.iter().position(|s| s.uuid() == uuid)
            })
            .or_else(|| key.parse::<usize>().ok().filter(|i| *i < pages.len()))
            .or_else(|| {
                let mut it = pages
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.name().as_str().eq_ignore_ascii_case(key));
                match (it.next(), it.next()) {
                    (Some((i, _)), None) => Some(i),
                    _ => None,
                }
            }),
    };
    let index = index.ok_or_else(|| match key {
        Some(key) => ToolError::not_found(format!(
            "There is no schematic page \"{key}\" (use schematic_list)."
        )),
        None => ToolError::not_found("The project has no schematic page."),
    })?;
    let s = &pages[index];
    Ok((index, s.id(), s))
}

/// Resolves a board by name, index (`"0"`) or UUID; `None` is the first
/// (primary) board.
pub fn board<'a>(
    project: &'a Project,
    key: Option<&str>,
) -> ToolResult<(usize, BoardId, &'a Board)> {
    let boards = project.boards();
    let index = match key.map(str::trim).filter(|k| !k.is_empty()) {
        None => (!boards.is_empty()).then_some(0),
        Some(key) => boards
            .iter()
            .position(|b| b.name().as_str() == key)
            .or_else(|| {
                let uuid = parse_uuid(key)?;
                boards.iter().position(|b| b.uuid() == uuid)
            })
            .or_else(|| key.parse::<usize>().ok().filter(|i| *i < boards.len()))
            .or_else(|| {
                let mut it = boards
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.name().as_str().eq_ignore_ascii_case(key));
                match (it.next(), it.next()) {
                    (Some((i, _)), None) => Some(i),
                    _ => None,
                }
            }),
    };
    let index = index.ok_or_else(|| match key {
        Some(key) => ToolError::not_found(format!(
            "There is no board \"{key}\" (use project_summary to list the boards)."
        )),
        None => ToolError::not_found("The project has no board."),
    })?;
    let b = &boards[index];
    Ok((index, b.id(), b))
}

/// Returns the library component of a component instance.
pub fn lib_component<'a>(
    project: &'a Project,
    component: &ComponentInstance,
) -> ToolResult<&'a Component> {
    project
        .library()
        .component(&component.lib_component())
        .ok_or_else(|| {
            ToolError::not_found(format!(
                "The component {} is missing in the project library.",
                component.lib_component()
            ))
        })
}

/// A resolved pin reference: a component signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPin {
    /// The component signal.
    pub signal: ComponentSignalRef,
    /// Display form `<designator>.<signal name>`.
    pub label: String,
}

/// Splits `"R1.1"` into designator and pin part.
pub fn split_pin_ref(pin: &str) -> ToolResult<(&str, &str)> {
    pin.trim()
        .split_once('.')
        .filter(|(c, p)| !c.is_empty() && !p.is_empty())
        .ok_or_else(|| {
            ToolError::invalid(format!(
                "Invalid pin reference \"{pin}\": expected <designator>.<pin>, e.g. \"R1.1\" or \
                 \"U1.VCC\"."
            ))
        })
}

/// Resolves a pin reference (see the module docs) to a component signal.
/// A designator alone (`"GND1"`) addresses the only signal of a
/// single-pin component (e.g. a supply symbol).
pub fn pin(project: &Project, reference: &str) -> ToolResult<ResolvedPin> {
    if !reference.contains('.')
        && let Ok((id, component)) = self::component(project, reference.trim())
    {
        let lib = lib_component(project, component)?;
        let mut signals = lib.signals().iter();
        return match (signals.next(), signals.next()) {
            (Some(sig), None) => Ok(ResolvedPin {
                signal: ComponentSignalRef {
                    component: id,
                    signal: sig.uuid(),
                },
                label: format!("{}.{}", component.name().as_str(), sig.name().as_str()),
            }),
            _ => Err(ToolError::invalid(format!(
                "Invalid pin reference \"{reference}\": {} has {} signals, use \
                 <designator>.<pin> (signals: {}).",
                component.name().as_str(),
                lib.signals().len(),
                lib.signals()
                    .iter()
                    .map(|s| s.name().as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        };
    }
    let (designator, pin) = split_pin_ref(reference)?;
    let (id, component) = self::component(project, designator)?;
    let lib = lib_component(project, component)?;
    let label = |name: &str| format!("{}.{}", component.name().as_str(), name);
    // 1. Signal name (exact), 2. signal UUID.
    if let Some(sig) = lib.signals().iter().find(|s| s.name().as_str() == pin) {
        return Ok(ResolvedPin {
            signal: ComponentSignalRef {
                component: id,
                signal: sig.uuid(),
            },
            label: label(sig.name().as_str()),
        });
    }
    if let Some(uuid) = parse_uuid(pin)
        && let Some(sig) = lib.signals().by_uuid(&uuid)
    {
        return Ok(ResolvedPin {
            signal: ComponentSignalRef {
                component: id,
                signal: uuid,
            },
            label: label(sig.name().as_str()),
        });
    }
    // 3. Pad name of the device (on any board), mapped to its signal.
    let mut signals = Vec::new();
    for board in project.boards() {
        if let Some(device) = board.device(id) {
            signals.extend(pad_signals(project, device, pin));
        }
    }
    // Without a device: pad names of the only library device of the
    // component in the project library.
    if signals.is_empty() {
        let devices = project
            .library()
            .devices_of_component(&lib.metadata().uuid());
        if let [dev] = devices.as_slice()
            && let Some(pkg) = project.library().package(&dev.package_uuid())
        {
            for pad in pkg.pads().iter().filter(|p| p.name().as_str() == pin) {
                if let Some(sig) = dev
                    .pad_signal_map()
                    .by_uuid(&pad.uuid())
                    .and_then(|m| m.signal_uuid())
                {
                    signals.push(sig);
                }
            }
        }
    }
    signals.sort();
    signals.dedup();
    match signals.as_slice() {
        [signal] => {
            let name = lib
                .signals()
                .by_uuid(signal)
                .map(|s| s.name().as_str().to_owned())
                .unwrap_or_else(|| signal.to_string());
            Ok(ResolvedPin {
                signal: ComponentSignalRef {
                    component: id,
                    signal: *signal,
                },
                label: label(&name),
            })
        }
        [] => {
            let names: Vec<&str> = lib.signals().iter().map(|s| s.name().as_str()).collect();
            Err(ToolError::not_found(format!(
                "The component {designator} has no pin \"{pin}\" (signals: {}).",
                names.join(", ")
            )))
        }
        _ => Err(ToolError::invalid(format!(
            "The pad \"{pin}\" of {designator} is connected to several signals; use the \
             signal name."
        ))),
    }
}

/// Returns the signals connected to the pads named `pad` of a device.
fn pad_signals(project: &Project, device: &BoardDevice, pad: &str) -> Vec<Uuid> {
    let lib = project.library();
    let (Some(dev), Some(pkg)) = (
        lib.device(&device.lib_device()),
        lib.device(&device.lib_device())
            .and_then(|d| lib.package(&d.package_uuid())),
    ) else {
        return Vec::new();
    };
    pkg.pads()
        .iter()
        .filter(|p| p.name().as_str() == pad)
        .filter_map(|p| dev.pad_signal_map().by_uuid(&p.uuid())?.signal_uuid())
        .collect()
}

/// Returns the display form `<designator>.<signal>` of a component signal.
pub fn signal_label(project: &Project, signal: ComponentSignalRef) -> String {
    let Some(component) = project.circuit().component_instance(signal.component) else {
        return format!("{}.{}", signal.component.0, signal.signal);
    };
    let name = project
        .library()
        .component(&component.lib_component())
        .and_then(|c| c.signals().by_uuid(&signal.signal))
        .map(|s| s.name().as_str().to_owned())
        .unwrap_or_else(|| signal.signal.to_string());
    format!("{}.{}", component.name().as_str(), name)
}

/// Returns the name of a net (or its UUID if it does not exist).
pub fn net_name(project: &Project, net: NetSignalId) -> String {
    project
        .circuit()
        .net_signal(net)
        .map(|n| n.name().as_str().to_owned())
        .unwrap_or_else(|| net.0.to_string())
}

/// Returns the designator of a component (or its UUID if it does not
/// exist).
pub fn designator(project: &Project, component: ComponentInstanceId) -> String {
    project
        .circuit()
        .component_instance(component)
        .map(|c| c.name().as_str().to_owned())
        .unwrap_or_else(|| component.0.to_string())
}
