//! Schematic and board read tools: `schematic_list`, `schematic_get`,
//! `board_get`.

use librepcb_core::geometry::{NetLineAnchor, TraceAnchor};
use librepcb_core::project::board::{Board, BoardAirWiresBuilder};
use librepcb_core::project::schematic::Schematic;
use librepcb_core::project::{ComponentInstanceId, NetSegmentId, Project, SymbolId};
use librepcb_core::types::Point;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ToolResult;
use crate::outcome::ToolOutput;
use crate::resolve;
use crate::session::Session;
use crate::tools::circuit::device_json;
use crate::units::{deg, mm};
use crate::views;

/// Arguments of `schematic_get`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicGetArgs {
    /// Schematic page name, index ("0") or UUID (default: first page).
    #[serde(default)]
    pub schematic: Option<String>,
}

/// Arguments of `board_get`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardGetArgs {
    /// Board name, index ("0") or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Include the pads of every device (default true).
    #[serde(default)]
    pub include_pads: Option<bool>,
}

/// `schematic_list`.
pub fn schematic_list(session: &Session) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let pages: Vec<Value> = p
        .schematics()
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "index": i,
                "uuid": s.uuid(),
                "name": s.name().as_str(),
                "grid_interval": mm(s.grid_interval()),
                "symbols": s.symbols().len(),
                "net_segments": s.net_segments().len(),
                "bus_segments": s.bus_segments().len(),
                "texts": s.texts().len(),
                "polygons": s.polygons().len(),
            })
        })
        .collect();
    let names: Vec<&str> = p.schematics().iter().map(|s| s.name().as_str()).collect();
    ToolOutput::new(
        format!("{} schematic page(s): {}", pages.len(), names.join(", ")),
        json!({ "schematics": pages }),
    )
}

fn net_line_anchor(
    p: &Project,
    schematic: &Schematic,
    segment: NetSegmentId,
    anchor: NetLineAnchor,
) -> Value {
    let position = schematic
        .net_line_anchor_position(segment, anchor, p.view())
        .map(views::point);
    match anchor {
        NetLineAnchor::Junction(j) => json!({ "junction": j, "position": position }),
        NetLineAnchor::BusJunction { segment, junction } => {
            json!({ "bus_segment": segment, "bus_junction": junction, "position": position })
        }
        NetLineAnchor::Pin { symbol, pin } => {
            let label = schematic
                .symbols()
                .get(&SymbolId(symbol))
                .and_then(|s| s.pin(p.view(), pin).ok().flatten())
                .map(|pv| resolve::signal_label(p, pv.signal()));
            json!({ "pin": label, "symbol": symbol, "pin_uuid": pin, "position": position })
        }
    }
}

/// `schematic_get`.
pub fn schematic_get(session: &Session, args: SchematicGetArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (index, _, s) = resolve::schematic(p, args.schematic.as_deref())?;
    let ctx = p.view();
    let mut symbols = Vec::new();
    for sym in s.symbols().values() {
        let pins: Vec<Value> = sym
            .pins(ctx)?
            .iter()
            .map(|pin| {
                json!({
                    "name": pin.name(),
                    "pin": resolve::signal_label(p, pin.signal()),
                    "net": pin.net().map(|n| resolve::net_name(p, n)),
                    "position": views::point(pin.position()),
                    "wired": s.pin_net_segment(sym.id(), pin.uuid()).is_some(),
                })
            })
            .collect();
        symbols.push(json!({
            "uuid": sym.uuid(),
            "name": sym.name(ctx).ok(),
            "component": resolve::designator(p, sym.component()),
            "gate": sym.lib_gate(),
            "position": views::point(sym.position()),
            "rotation": deg(sym.rotation()),
            "mirrored": sym.mirrored(),
            "pins": pins,
        }));
    }
    let mut segments = Vec::new();
    for (id, seg) in s.net_segments() {
        let junctions: Vec<Value> = seg
            .junctions()
            .values()
            .map(|j| json!({ "uuid": j.uuid(), "position": views::point(j.position()) }))
            .collect();
        let lines: Vec<Value> = seg
            .lines()
            .values()
            .map(|l| {
                json!({
                    "uuid": l.uuid(),
                    "from": net_line_anchor(p, s, *id, l.p1()),
                    "to": net_line_anchor(p, s, *id, l.p2()),
                    "width": mm(l.width()),
                })
            })
            .collect();
        let labels: Vec<Value> = seg
            .labels()
            .values()
            .map(|l| {
                json!({
                    "uuid": l.uuid(),
                    "position": views::point(l.position()),
                    "rotation": deg(l.rotation()),
                    "mirrored": l.mirrored(),
                })
            })
            .collect();
        segments.push(json!({
            "uuid": id.0,
            "net": resolve::net_name(p, seg.net()),
            "junctions": junctions,
            "lines": lines,
            "labels": labels,
        }));
    }
    let texts: Vec<Value> = s
        .texts()
        .values()
        .map(|t| json!({ "uuid": t.uuid(), "text": t.text(), "position": views::point(t.position()) }))
        .collect();
    let summary = format!(
        "Schematic page {index} \"{}\": {} symbol(s), {} net segment(s).",
        s.name().as_str(),
        symbols.len(),
        segments.len()
    );
    ToolOutput::new(
        summary,
        json!({
            "index": index,
            "uuid": s.uuid(),
            "name": s.name().as_str(),
            "grid_interval": mm(s.grid_interval()),
            "symbols": symbols,
            "net_segments": segments,
            "bus_segments": s.bus_segments().len(),
            "polygons": s.polygons().len(),
            "texts": texts,
        }),
    )
}

fn trace_anchor(p: &Project, board: &Board, position: Option<Point>, anchor: TraceAnchor) -> Value {
    let position = position.map(views::point);
    match anchor {
        TraceAnchor::Junction(j) => json!({ "junction": j, "position": position }),
        TraceAnchor::Via(v) => json!({ "via": v, "position": position }),
        TraceAnchor::Pad(pad) => json!({ "board_pad": pad, "position": position }),
        TraceAnchor::FootprintPad { device, pad } => {
            let component = ComponentInstanceId(device);
            let name = board
                .device(component)
                .and_then(|d| d.pad(&pad, p.library(), p.circuit()).ok().flatten())
                .and_then(|pv| pv.package_pad().map(|pp| pp.name().as_str().to_owned()));
            json!({
                "pad": name.map(|n| format!("{}.{n}", resolve::designator(p, component))),
                "pad_uuid": pad,
                "position": position,
            })
        }
    }
}

/// The unrouted connections (air wires) of a board, computed without
/// changing the model (the stored air wires may be outdated).
pub fn air_wires_json(p: &Project, b: &Board) -> Vec<Value> {
    let builder = BoardAirWiresBuilder::new(b, p.library(), p.circuit());
    let mut air_wires = Vec::new();
    for net in p.circuit().net_signals().keys() {
        for aw in builder.build_air_wires(*net) {
            air_wires.push(json!({
                "net": resolve::net_name(p, *net),
                "from": trace_anchor(p, b, Some(aw.p1_position()), aw.p1()),
                "to": trace_anchor(p, b, Some(aw.p2_position()), aw.p2()),
                "length": mm(*(aw.p2_position() - aw.p1_position()).length()),
            }));
        }
    }
    air_wires
}

/// `board_get`.
pub fn board_get(session: &Session, args: BoardGetArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (index, _, b) = resolve::board(p, args.board.as_deref())?;
    let include_pads = args.include_pads.unwrap_or(true);
    let settings = b.settings();

    let mut outlines = Vec::new();
    let mut other_polygons = 0;
    for poly in b.polygons().values() {
        if poly.layer().is_board_edge() {
            outlines.push(json!({
                "uuid": poly.uuid(),
                "layer": poly.layer().id(),
                "path": views::path(poly.path()),
            }));
        } else {
            other_polygons += 1;
        }
    }

    let mut devices = Vec::new();
    for d in b.devices().values() {
        let mut v = device_json(p, b, d)?;
        if !include_pads && let Some(o) = v.as_object_mut() {
            o.remove("pads");
        }
        devices.push(v);
    }

    let mut traces = Vec::new();
    let mut vias = Vec::new();
    for seg in b.net_segments().values() {
        let net = seg.net().map(|n| resolve::net_name(p, n));
        for t in seg.traces().values() {
            let p1 = b.anchor_position(seg, t.p1(), p.library(), p.circuit());
            let p2 = b.anchor_position(seg, t.p2(), p.library(), p.circuit());
            traces.push(json!({
                "uuid": t.uuid(),
                "segment": seg.uuid(),
                "net": net,
                "layer": t.layer().id(),
                "width": mm(t.width()),
                "from": trace_anchor(p, b, p1, t.p1()),
                "to": trace_anchor(p, b, p2, t.p2()),
            }));
        }
        for v in seg.vias().values() {
            vias.push(json!({
                "uuid": v.uuid(),
                "segment": seg.uuid(),
                "net": net,
                "position": views::point(v.position()),
                "drill": v.drill_diameter().map(mm),
                "size": v.size().map(mm),
                "start_layer": v.start_layer().id(),
                "end_layer": v.end_layer().id(),
            }));
        }
    }

    let planes: Vec<Value> = b
        .planes()
        .values()
        .map(|pl| {
            json!({
                "uuid": pl.uuid(),
                "net": pl.net().map(|n| resolve::net_name(p, n)),
                "layer": pl.layer().id(),
                "outline": views::path(pl.outline()),
                "min_width": mm(pl.min_width()),
                "min_clearance": mm(pl.min_clearance_to_copper()),
                "connect_style": pl.connect_style().to_str(),
                "priority": pl.priority(),
                "keep_islands": pl.keep_islands(),
                "fragments": b.derived().fragments_of(pl.id()).len(),
            })
        })
        .collect();

    // Air wires, computed without changing the model (the stored air
    // wires may be outdated). Plane fragments are only up to date after a
    // plane rebuild (e.g. by export_fabrication).
    let air_wires = air_wires_json(p, b);

    let summary = format!(
        "Board {index} \"{}\": {} copper layers, {} device(s), {} trace(s), {} via(s), {} plane(s), \
         {} unrouted connection(s).",
        b.name().as_str(),
        settings.inner_layer_count + 2,
        devices.len(),
        traces.len(),
        vias.len(),
        planes.len(),
        air_wires.len()
    );
    ToolOutput::new(
        summary,
        json!({
            "index": index,
            "uuid": b.uuid(),
            "name": b.name().as_str(),
            "copper_layers": settings.inner_layer_count + 2,
            "thickness": mm(settings.pcb_thickness),
            "outlines": outlines,
            "other_polygons": other_polygons,
            "devices": devices,
            "traces": traces,
            "vias": vias,
            "planes": planes,
            "air_wires": air_wires,
            "zones": b.zones().len(),
            "holes": b.holes().len(),
            "stroke_texts": b.stroke_texts().len(),
        }),
    )
}
