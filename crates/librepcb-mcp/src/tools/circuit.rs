//! Circuit read tools: `component_list`, `component_get`, `net_list`,
//! `netlist`.

use std::collections::BTreeMap;

use librepcb_core::library::LibraryBaseElement;
use librepcb_core::project::circuit::ComponentInstance;
use librepcb_core::project::{ComponentInstanceId, NetSignalId, NetUse, Project};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ToolResult;
use crate::outcome::ToolOutput;
use crate::resolve;
use crate::session::Session;
use crate::units::deg;
use crate::views;

/// Arguments of `component_get`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentGetArgs {
    /// Designator (e.g. "R1") or UUID of the component.
    pub component: String,
}

/// Arguments of `net_list`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetListArgs {
    /// Only this net (name or UUID); default all nets.
    #[serde(default)]
    pub net: Option<String>,
}

/// `component_list`.
pub fn component_list(session: &Session) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let mut list: Vec<(&ComponentInstance, Value)> = p
        .circuit()
        .component_instances()
        .iter()
        .map(|(id, c)| (c, component_summary(p, *id, c)))
        .collect();
    list.sort_by(|a, b| crate::views::natural_cmp(a.0.name().as_str(), b.0.name().as_str()));
    let items: Vec<Value> = list.into_iter().map(|(_, v)| v).collect();
    let text: Vec<String> = items
        .iter()
        .map(|v| {
            format!(
                "{} {} ({})",
                v["designator"].as_str().unwrap_or_default(),
                v["value"].as_str().unwrap_or_default(),
                v["library_component"]["name"].as_str().unwrap_or_default()
            )
        })
        .collect();
    ToolOutput::new(
        format!("{} components: {}", items.len(), text.join(", ")),
        json!({ "components": items }),
    )
}

fn component_summary(p: &Project, id: ComponentInstanceId, c: &ComponentInstance) -> Value {
    let lib = p.library().component(&c.lib_component());
    let symbols = p
        .schematics()
        .iter()
        .flat_map(|s| s.symbols().values())
        .filter(|s| s.component() == id)
        .count();
    let gates = lib
        .and_then(|l| l.symbol_variants().by_uuid(&c.lib_variant()))
        .map_or(0, |v| v.symbol_items().len());
    let boards: Vec<Value> = p
        .boards()
        .iter()
        .filter_map(|b| {
            b.device(id).map(|d| {
                json!({
                    "board": b.name().as_str(),
                    "device": p.library().device(&d.lib_device()).map(|x| x.metadata().name().as_str()),
                })
            })
        })
        .collect();
    json!({
        "uuid": id.0,
        "designator": c.name().as_str(),
        "value": c.value(),
        "library_component": {
            "uuid": c.lib_component(),
            "name": lib.map(|l| l.metadata().name().as_str()),
        },
        "schematic_only": lib.is_some_and(|l| l.schematic_only()),
        "symbols_placed": symbols,
        "gates": gates,
        "devices": boards,
    })
}

/// `component_get`.
pub fn component_get(session: &Session, args: ComponentGetArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (id, _) = resolve::component(p, &args.component)?;
    let (summary, v) = component_detail(p, id)?;
    ToolOutput::new(summary, v)
}

/// Details of a component (see `component_get`) and a one-line summary.
pub fn component_detail(p: &Project, id: ComponentInstanceId) -> ToolResult<(String, Value)> {
    let c = p.circuit().component_instance(id).ok_or_else(|| {
        crate::error::ToolError::not_found(format!("There is no component {}.", id.0))
    })?;
    let lib = resolve::lib_component(p, c)?;
    let mut v = component_summary(p, id, c);
    v["attributes"] = views::attributes(c.attributes());
    v["symbol_variant"] = json!({
        "uuid": c.lib_variant(),
        "name": lib.symbol_variants().by_uuid(&c.lib_variant()).map(|x| x.name().as_str()),
    });
    let designator = c.name().as_str();
    v["signals"] = Value::Array(
        lib.signals()
            .iter()
            .map(|s| {
                let net = c.signal(&s.uuid()).and_then(|x| x.net());
                json!({
                    "uuid": s.uuid(),
                    "pin": format!("{designator}.{}", s.name().as_str()),
                    "name": s.name().as_str(),
                    "role": s.role().to_str(),
                    "required": s.is_required(),
                    "net": net.map(|n| resolve::net_name(p, n)),
                })
            })
            .collect(),
    );
    let ctx = p.view();
    let mut symbols = Vec::new();
    for (index, schematic) in p.schematics().iter().enumerate() {
        for sym in schematic.symbols().values().filter(|s| s.component() == id) {
            let gate = lib
                .symbol_variants()
                .by_uuid(&c.lib_variant())
                .and_then(|var| var.symbol_items().by_uuid(&sym.lib_gate()));
            let pins: Vec<Value> = sym
                .pins(ctx)?
                .iter()
                .map(|pin| {
                    json!({
                        "uuid": pin.uuid(),
                        "name": pin.name(),
                        "signal": pin.lib_signal().name().as_str(),
                        "net": pin.net().map(|n| resolve::net_name(p, n)),
                        "position": views::point(pin.position()),
                        "rotation": deg(pin.rotation()),
                        "wired": schematic.pin_net_segment(sym.id(), pin.uuid()).is_some(),
                    })
                })
                .collect();
            symbols.push(json!({
                "uuid": sym.uuid(),
                "name": sym.name(ctx).ok(),
                "schematic": { "index": index, "name": schematic.name().as_str() },
                "gate": sym.lib_gate(),
                "gate_suffix": gate.map(|g| g.suffix().as_str()),
                "position": views::point(sym.position()),
                "rotation": deg(sym.rotation()),
                "mirrored": sym.mirrored(),
                "pins": pins,
            }));
        }
    }
    v["symbols"] = Value::Array(symbols);
    let mut devices = Vec::new();
    for board in p.boards() {
        if let Some(d) = board.device(id) {
            devices.push(device_json(p, board, d)?);
        }
    }
    v["devices"] = Value::Array(devices);
    let summary = format!(
        "{} = {} ({}), {} signal(s), {} symbol(s), {} device(s).",
        designator,
        c.value(),
        lib.metadata().name().as_str(),
        lib.signals().len(),
        v["symbols"].as_array().map_or(0, Vec::len),
        v["devices"].as_array().map_or(0, Vec::len),
    );
    Ok((summary, v))
}

/// A placed device with its pads (absolute positions).
pub fn device_json(
    p: &Project,
    board: &librepcb_core::project::board::Board,
    d: &librepcb_core::project::board::BoardDevice,
) -> ToolResult<Value> {
    let designator = resolve::designator(p, d.component());
    let pads: Vec<Value> = d
        .pads(p.library(), p.circuit())?
        .iter()
        .map(|pad| {
            let mut v = views::pad_geometry(pad.properties(), pad.position(), pad.rotation());
            let name = pad.package_pad().map(|pp| pp.name().as_str().to_owned());
            v["uuid"] = json!(pad.uuid());
            v["pad"] = json!(name.as_ref().map(|n| format!("{designator}.{n}")));
            v["name"] = json!(name);
            v["signal"] = json!(pad.component_signal_name());
            v["net"] = json!(pad.net().map(|n| resolve::net_name(p, n)));
            if d.mirrored() && !pad.properties().is_tht() {
                // The pad is on the other side of the board.
                v["side"] = json!(match pad.properties().component_side().to_str() {
                    "top" => "bottom",
                    _ => "top",
                });
            }
            v
        })
        .collect();
    let lib_dev = p.library().device(&d.lib_device());
    let package = lib_dev.and_then(|x| p.library().package(&x.package_uuid()));
    Ok(json!({
        "designator": designator,
        "component": d.component().0,
        "board": board.name().as_str(),
        "device": { "uuid": d.lib_device(), "name": lib_dev.map(|x| x.metadata().name().as_str()) },
        "package": package.map(|x| json!({ "uuid": x.metadata().uuid(), "name": x.metadata().name().as_str() })),
        "footprint": d.lib_footprint(),
        "position": views::point(d.position()),
        "rotation": deg(d.rotation()),
        "side": if d.mirrored() { "bottom" } else { "top" },
        "locked": d.locked(),
        "pads": pads,
    }))
}

/// Pins (component signals) per net, sorted by label.
fn pins_by_net(p: &Project) -> (BTreeMap<NetSignalId, Vec<String>>, Vec<String>) {
    let mut nets: BTreeMap<NetSignalId, Vec<String>> = BTreeMap::new();
    let mut unconnected = Vec::new();
    for c in p.circuit().component_instances().values() {
        let lib = p.library().component(&c.lib_component());
        for (sig, inst) in c.signals() {
            let name = lib
                .and_then(|l| l.signals().by_uuid(sig))
                .map_or_else(|| sig.to_string(), |s| s.name().as_str().to_owned());
            let label = format!("{}.{}", c.name().as_str(), name);
            match inst.net() {
                Some(net) => nets.entry(net).or_default().push(label),
                None => unconnected.push(label),
            }
        }
    }
    let sort = |v: &mut Vec<String>| {
        v.sort_by(|a, b| crate::views::natural_cmp(a, b));
    };
    nets.values_mut().for_each(sort);
    sort(&mut unconnected);
    (nets, unconnected)
}

/// `net_list`.
pub fn net_list(session: &Session, args: NetListArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let only = args
        .net
        .as_deref()
        .map(|n| resolve::net(p, n).map(|x| x.0))
        .transpose()?;
    let mut nets = nets_json(p, only);
    nets.sort_by(|a, b| {
        crate::views::natural_cmp(
            a["name"].as_str().unwrap_or_default(),
            b["name"].as_str().unwrap_or_default(),
        )
    });
    ToolOutput::new(format!("{} net(s).", nets.len()), json!({ "nets": nets }))
}

/// Nets (all or only one) with pins, pads and usage counts.
pub fn nets_json(p: &Project, only: Option<NetSignalId>) -> Vec<Value> {
    let (pins, _) = pins_by_net(p);
    let mut nets = Vec::new();
    for (id, net) in p.circuit().net_signals() {
        if only.is_some_and(|o| o != *id) {
            continue;
        }
        let mut schematic_segments = 0;
        let mut board_segments = 0;
        let mut planes = 0;
        for u in p.net_signal_uses(*id) {
            match u {
                NetUse::SchematicSegment(..) => schematic_segments += 1,
                NetUse::BoardSegment(..) => board_segments += 1,
                NetUse::Plane(..) => planes += 1,
                _ => {}
            }
        }
        let mut pads = Vec::new();
        for board in p.boards() {
            for d in board.devices().values() {
                if let Ok(views) = d.pads(p.library(), p.circuit()) {
                    for pad in views.iter().filter(|x| x.net() == Some(*id)) {
                        if let Some(pp) = pad.package_pad() {
                            pads.push(format!(
                                "{}.{}",
                                resolve::designator(p, d.component()),
                                pp.name().as_str()
                            ));
                        }
                    }
                }
            }
        }
        pads.sort_by(|a, b| crate::views::natural_cmp(a, b));
        pads.dedup();
        nets.push(json!({
            "uuid": id.0,
            "name": net.name().as_str(),
            "auto_name": net.has_auto_name(),
            "net_class": p.circuit().net_class(net.net_class()).map(|nc| nc.name().as_str()),
            "pins": pins.get(id).cloned().unwrap_or_default(),
            "pads": pads,
            "schematic_segments": schematic_segments,
            "board_segments": board_segments,
            "planes": planes,
        }));
    }
    nets
}

/// `netlist`.
pub fn netlist(session: &Session) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (pins, unconnected) = pins_by_net(p);
    let mut by_name: Vec<(String, Vec<String>)> = p
        .circuit()
        .net_signals()
        .iter()
        .map(|(id, n)| {
            (
                n.name().as_str().to_owned(),
                pins.get(id).cloned().unwrap_or_default(),
            )
        })
        .collect();
    by_name.sort_by(|a, b| crate::views::natural_cmp(&a.0, &b.0));
    let text: Vec<String> = by_name
        .iter()
        .map(|(n, pins)| format!("{n}: {}", pins.join(" ")))
        .collect();
    let nets: serde_json::Map<String, Value> = by_name
        .into_iter()
        .map(|(n, pins)| (n, json!(pins)))
        .collect();
    ToolOutput::new(
        format!(
            "{} net(s), {} unconnected pin(s).\n{}",
            nets.len(),
            unconnected.len(),
            text.join("\n")
        ),
        json!({ "nets": nets, "unconnected_pins": unconnected }),
    )
}
