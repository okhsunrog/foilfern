//! Read-only pad-to-pad centerline measurements, driven by PCB review friction.

use std::collections::BTreeSet;

use librepcb_core::project::board::{Board, TracePath, centerline_length_mm};
use librepcb_core::project::{ComponentInstanceId, ComponentSignalRef, NetSignalId, Project};
use librepcb_core::types::{Point, Uuid};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::write;
use crate::error::{ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::Session;
use crate::{resolve, units, views};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PadAddress {
    /// Designator plus package pad name or component signal, e.g. OLED1.28.
    Label(String),
    Identity(PadIdentity),
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PadIdentity {
    /// Component designator or instance UUID.
    pub component: String,
    /// Footprint pad UUID, package pad name or component signal name.
    pub pad: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TracePathArgs {
    /// Board name, index or UUID; defaults to the primary board.
    pub board: Option<String>,
    pub from: PadAddress,
    pub to: PadAddress,
    pub expected_revision: Option<u64>,
}

struct Endpoint {
    component: ComponentInstanceId,
    pad: Uuid,
    signal: Option<ComponentSignalRef>,
    net: Option<NetSignalId>,
    position: Point,
    label: String,
}

impl Endpoint {
    fn json(&self) -> Value {
        json!({"component": self.component, "pad": self.pad, "signal": self.signal,
            "label": self.label, "position_mm": views::point(self.position)})
    }
}

fn endpoint(p: &Project, board: &Board, address: &PadAddress) -> ToolResult<Endpoint> {
    let (component, key) = match address {
        PadAddress::Label(label) => resolve::split_pin_ref(label)?,
        PadAddress::Identity(identity) => (identity.component.as_str(), identity.pad.as_str()),
    };
    let (id, component) = resolve::component(p, component)?;
    let device = board
        .device(id)
        .ok_or_else(|| ToolError::not_found("Component has no device on this board."))?;
    let pads = device.pads(p.library(), p.circuit())?;
    let matches = [
        pads.iter()
            .filter(|pad| pad.uuid().to_string() == key)
            .collect::<Vec<_>>(),
        pads.iter()
            .filter(|pad| {
                pad.package_pad()
                    .is_some_and(|pad| pad.name().as_str() == key)
            })
            .collect(),
        pads.iter()
            .filter(|pad| pad.component_signal_name() == Some(key))
            .collect(),
    ];
    let matching = matches
        .iter()
        .find(|pads| !pads.is_empty())
        .ok_or_else(|| {
            ToolError::not_found(format!(
                "Pad {}.{key} does not exist on this board.",
                component.name()
            ))
        })?;
    let [pad] = matching.as_slice() else {
        return Err(ToolError::invalid(
            "Endpoint resolves to multiple footprint pads; supply its footprint pad UUID.",
        ));
    };
    Ok(Endpoint {
        component: id,
        pad: pad.uuid(),
        signal: pad.component_signal(),
        net: pad.net(),
        position: pad.position(),
        label: format!("{}.{}", component.name(), pad.name_or_uuid()),
    })
}

pub fn trace_path_measure(session: &Session, args: TracePathArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    write::check_revision(p, args.expected_revision)?;
    let (_, board_id, board) = resolve::board(p, args.board.as_deref())?;
    let from = endpoint(p, board, &args.from)?;
    let to = endpoint(p, board, &args.to)?;
    let net = from
        .net
        .filter(|net| Some(*net) == to.net)
        .ok_or_else(|| ToolError::invalid("Both pads must belong to the same assigned net."))?;
    let path = p.trace_path(
        board_id,
        net,
        (from.component, from.pad),
        (to.component, to.pad),
    )?;
    let direct_mm = centerline_length_mm(from.position, to.position);
    let mut result = json!({"project": p.uuid(), "base_revision": p.revision(), "board": board_id,
        "net": {"uuid": net, "name": resolve::net_name(p, net)}, "from": from.json(), "to": to.json(),
        "direct_mm": direct_mm, "length_mm": null, "status": "not_measured", "trace_path_only": true,
        "plane_copper_measured": false, "via_barrel_length_measured": false,
        "unanchored_copper_contacts_measured": false,
        "same_net_plane_present": board.planes().values().any(|plane| plane.net() == Some(net))});
    if let Some(TracePath {
        length_mm,
        traces,
        vias,
    }) = path
    {
        let layers: BTreeSet<_> = traces.iter().map(|trace| trace.layer.id()).collect();
        let measured: Vec<_> = traces
            .iter()
            .map(|trace| {
                json!({"segment": trace.segment,
            "uuid": trace.trace, "layer": trace.layer.id(), "width_mm": units::mm(*trace.width),
            "length_mm": trace.length_mm})
            })
            .collect();
        result["status"] = json!("measured");
        result["length_mm"] = json!(length_mm);
        result["layers"] = json!(layers);
        result["via_count"] = json!(vias.len());
        result["vias"] = json!(
            vias.iter()
                .map(|(segment, uuid)| json!({"segment": segment, "uuid": uuid}))
                .collect::<Vec<_>>()
        );
        result["traces"] = json!(measured);
        ToolOutput::new(
            "Measured shortest anchored trace centerline; plane copper and via barrels are excluded.",
            result,
        )
    } else {
        result["reason"] = json!(
            "No explicit anchored trace path. A plane or unanchored copper contact may connect these pads; that length is not measured. This is not a connectivity verdict."
        );
        Ok(ToolOutput::new("No trace-only path measurement available.", result)?.partial())
    }
}
