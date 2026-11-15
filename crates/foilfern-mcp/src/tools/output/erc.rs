//! Resolved ERC subjects for callers that should not parse debug strings.

use foilfern_core::project::erc::ErcMessageKind;
use foilfern_core::project::{ComponentSignalRef, Project};
use serde_json::{Value, json};

use crate::resolve;

fn signal_object(project: &Project, signal: ComponentSignalRef) -> Value {
    let component = project.circuit().component_instance(signal.component);
    let name = component
        .and_then(|c| project.library().component(&c.lib_component()))
        .and_then(|c| c.signals().by_uuid(&signal.signal))
        .map(|s| s.name().as_str());
    let net = component
        .and_then(|c| c.signals().get(&signal.signal))
        .and_then(|s| s.net());
    let pads: Vec<_> = project
        .boards()
        .iter()
        .flat_map(|board| {
            board
                .device(signal.component)
                .into_iter()
                .flat_map(move |device| {
                    device
                        .pads(project.library(), project.circuit())
                        .unwrap_or_default()
                        .into_iter()
                        .filter(move |p| p.component_signal() == Some(signal))
                        .map(move |pad| {
                            json!({"board":board.id(),"component":signal.component,
                    "pad":pad.uuid(),"name":pad.name_or_uuid()})
                        })
                })
        })
        .collect();
    json!({"kind":"component_signal","component":signal.component,
        "designator":resolve::designator(project,signal.component),
        "signal":signal.signal,"signal_name":name,"net":net,
        "net_name":net.and_then(|id|project.circuit().net_signal(id)).map(|n|n.name().as_str()),
        "pads":pads})
}

pub(super) fn objects(project: &Project, kind: ErcMessageKind) -> Vec<Value> {
    use ErcMessageKind::*;
    match kind {
        ConnectedPinWithoutWire {
            schematic,
            symbol,
            pin,
        } => {
            let mut object =
                json!({"kind":"symbol_pin","schematic":schematic,"symbol":symbol,"pin":pin});
            if let Some(s) = project
                .schematic(schematic)
                .and_then(|s| s.symbols().get(&symbol))
                && let Ok(pins) = s.pins(project.view())
                && let Some(p) = pins.iter().find(|p| p.uuid() == pin)
            {
                object = signal_object(project, p.signal());
                object["kind"] = json!("symbol_pin");
                object["schematic"] = json!(schematic);
                object["symbol"] = json!(symbol);
                object["pin"] = json!(pin);
                object["pin_name"] = json!(p.lib_pin().name().as_str());
                object["displayed_pin_name"] = json!(p.name());
            }
            vec![object]
        }
        UnconnectedRequiredSignal(s) | ForcedNetSignalNameConflict(s) => {
            vec![signal_object(project, s)]
        }
        UnplacedRequiredGate { component, gate } | UnplacedOptionalGate { component, gate } => {
            vec![
                json!({"kind":"gate","component":component,"designator":resolve::designator(project,component),"gate":gate}),
            ]
        }
        UnusedNetClass(id) => vec![json!({"kind":"net_class","uuid":id,
            "name":project.circuit().net_classes().get(&id).map(|n|n.name().as_str())})],
        UnusedBus(id) => vec![
            json!({"kind":"bus","uuid":id,"name":project.circuit().bus(id).map(|n|n.name().as_str())}),
        ],
        OpenNet(id) => vec![
            json!({"kind":"net","uuid":id,"name":project.circuit().net_signal(id).map(|n|n.name().as_str())}),
        ],
        OpenNetInBus { bus, net, segment } | UnnamedNetInBus { bus, net, segment } => vec![json!({
            "kind":"bus_net","bus":bus,"bus_name":project.circuit().bus(bus).map(|b|b.name().as_str()),
            "net":net,"net_name":project.circuit().net_signal(net).map(|n|n.name().as_str()),
            "schematic":segment.schematic,"segment":segment.segment})],
        OpenWireInSegment { segment, line } => vec![
            json!({"kind":"wire","schematic":segment.schematic,"segment":segment.segment,"line":line}),
        ],
        UnconnectedNetJunction { segment, junction } => vec![
            json!({"kind":"net_junction","schematic":segment.schematic,"segment":segment.segment,"junction":junction}),
        ],
        UnconnectedBusJunction { segment, junction } => vec![
            json!({"kind":"bus_junction","schematic":segment.schematic,"segment":segment.segment,"junction":junction}),
        ],
        _ => Vec::new(),
    }
}
