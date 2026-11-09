//! Board tools: `board_add`, `board_set_outline`, `device_place`,
//! `device_auto_place`, `trace_add`, `via_add`, `trace_remove`,
//! `plane_add`, `design_rules_set`, `autoroute` (write tools, one undo
//! group each, see [`write`]), `specctra_import`, `planes_rebuild`
//! (derived data only), `unrouted` and `specctra_export` (read).

use librepcb_core::fileio::{CleanFileNameOptions, FileNameCase, FilePath};
use librepcb_core::geometry::Path;
use librepcb_core::project::board::PlaneConnectStyle;
use librepcb_core::project::{BoardId, ComponentInstanceId, Project};
use librepcb_core::types::{
    Angle, ElementName, Layer, Length, Point, PositiveLength, UnsignedLength,
};
use librepcb_editor::commands::{
    AddBoard, AddDevice, AddPlane, AddTrace, AddVia, AutoPlaceDevices, Autoroute, BoardSelection,
    EditBoardSettings, ImportSpecctraSession, MessageLevel, MoveDevice, NetRef, PadRef,
    PlaneSettings, RemoveBoardItems, ReplaceDevice, SetBoardOutline, SpecctraExportManifest,
    TraceEndpoint, closed_path, export_specctra_dsn,
};
use librepcb_editor::{
    FreeroutingError, FreeroutingOutput, FreeroutingRouter, RoutingStats, dsn_for_freerouting,
    routing_stats,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::resolve;
use crate::session::Session;
use crate::tools::circuit::device_json;
use crate::tools::layout::air_wires_json;
use crate::tools::write::write;
use crate::units::{PointMm, angle, length, mm};
use crate::views;

fn positive(mm: f64, what: &str) -> ToolResult<PositiveLength> {
    PositiveLength::new(length(mm)?)
        .map_err(|_| ToolError::invalid(format!("{what} must be greater than zero")))
}

fn unsigned(mm: f64, what: &str) -> ToolResult<UnsignedLength> {
    UnsignedLength::new(length(mm)?)
        .map_err(|_| ToolError::invalid(format!("{what} must not be negative")))
}

/// Parses a copper layer: "top", "bottom", "inner1".., or a layer id like
/// "top_cu", "bot_cu", "in1_cu".
pub fn copper_layer(s: &str) -> ToolResult<Layer> {
    let s = s.trim().to_lowercase();
    let layer = match s.as_str() {
        "top" | "front" => Some(Layer::TOP_COPPER),
        "bottom" | "bot" | "back" => Some(Layer::BOT_COPPER),
        _ => s
            .strip_prefix("inner")
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(Layer::inner_copper)
            .or_else(|| Layer::from_id(&s)),
    };
    layer.filter(|l| l.is_copper()).ok_or_else(|| {
        ToolError::invalid(format!(
            "invalid copper layer \"{s}\" (use top, bottom, inner1.. or top_cu, bot_cu, in1_cu..)"
        ))
    })
}

/// Resolves a board argument.
fn board_id(p: &Project, key: Option<&str>) -> ToolResult<BoardId> {
    Ok(resolve::board(p, key)?.1)
}

/// Overview of a board after a change: outline, devices, counts, air
/// wires.
fn board_overview(p: &Project, board: BoardId) -> ToolResult<Value> {
    let (index, _, b) = resolve::board(p, Some(&board.0.to_string()))?;
    let outlines: Vec<Value> = b
        .polygons()
        .values()
        .filter(|poly| poly.layer().is_board_edge())
        .map(|poly| views::path(poly.path()))
        .collect();
    let devices: Vec<Value> = b
        .devices()
        .values()
        .map(|d| {
            json!({
                "designator": resolve::designator(p, d.component()),
                "position": views::point(d.position()),
                "rotation": views::angle(d.rotation()),
                "side": if d.mirrored() { "bottom" } else { "top" },
            })
        })
        .collect();
    let (traces, vias) = b.net_segments().values().fold((0, 0), |(t, v), s| {
        (t + s.traces().len(), v + s.vias().len())
    });
    let air_wires = air_wires_json(p, b);
    Ok(json!({
        "board": { "index": index, "uuid": b.uuid(), "name": b.name().as_str() },
        "outlines": outlines,
        "devices": devices,
        "traces": traces,
        "vias": vias,
        "planes": b.planes().len(),
        "unrouted": air_wires.len(),
    }))
}

/// Arguments of `board_add`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardAddArgs {
    /// Name of the board.
    pub name: String,
    /// Board (name, index or UUID) to copy with its settings and all items
    /// (default: a new board with the default settings and outline).
    #[serde(default)]
    pub copy_from: Option<String>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `board_add`.
pub fn board_add(session: &mut Session, args: BoardAddArgs) -> ToolResult<ToolOutput> {
    let name = ElementName::new(args.name.trim())
        .map_err(|e| ToolError::invalid(format!("invalid board name: {e}")))?;
    let copy_from = match args.copy_from.as_deref() {
        Some(key) => Some(resolve::board(session.project()?.project(), Some(key))?.1),
        None => None,
    };
    let done = write(session, "board_add", args.expected_revision, |editor| {
        let mut command = AddBoard::new(name);
        command.copy_from = copy_from;
        Ok(editor.execute(command)?)
    })?;
    let open = session.project()?;
    let result = board_overview(open.project(), done.value.board)?;
    let what = if copy_from.is_some() {
        "a copy of the board"
    } else {
        "default 100x80 mm outline"
    };
    done.output(
        open,
        format!("Added board \"{}\" ({what}).", args.name.trim()),
        result,
    )
}

/// Arguments of `board_set_outline`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardSetOutlineArgs {
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Rectangle width in mm.
    #[serde(default)]
    pub width: Option<f64>,
    /// Rectangle height in mm.
    #[serde(default)]
    pub height: Option<f64>,
    /// Bottom left corner of the rectangle in mm (default 0,0).
    #[serde(default)]
    pub origin: Option<PointMm>,
    /// Corner radius of the rectangle in mm (default 0).
    #[serde(default)]
    pub corner_radius: Option<f64>,
    /// Instead of a rectangle: polygon vertices in mm (closed
    /// automatically).
    #[serde(default)]
    pub polygon: Option<Vec<PointMm>>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `board_set_outline`.
pub fn board_set_outline(
    session: &mut Session,
    args: BoardSetOutlineArgs,
) -> ToolResult<ToolOutput> {
    let board = board_id(session.project()?.project(), args.board.as_deref())?;
    let outline = match (&args.polygon, args.width, args.height) {
        (Some(points), None, None) => {
            if points.len() < 3 {
                return Err(ToolError::invalid("A polygon needs at least 3 points."));
            }
            let points: Vec<Point> = points
                .iter()
                .map(|p| p.to_point())
                .collect::<ToolResult<_>>()?;
            closed_path(&points)
        }
        (None, Some(w), Some(h)) => {
            let w = positive(w, "width")?;
            let h = positive(h, "height")?;
            let r = unsigned(args.corner_radius.unwrap_or(0.0), "corner_radius")?;
            let origin = args.origin.map(|o| o.to_point()).transpose()?;
            let origin = origin.unwrap_or(Point::ORIGIN);
            Path::centered_rect(w, h, r)
                .translated(Point::new(origin.x + *w / 2, origin.y + *h / 2))
        }
        _ => {
            return Err(ToolError::invalid(
                "Pass either width and height (with optional origin and corner_radius) or \
                 polygon.",
            ));
        }
    };
    let done = write(
        session,
        "board_set_outline",
        args.expected_revision,
        |editor| {
            Ok(editor.execute(SetBoardOutline {
                board: Some(board),
                outline,
            })?)
        },
    )?;
    let open = session.project()?;
    let result = board_overview(open.project(), board)?;
    done.output(open, "Board outline set.", result)
}

/// Board side of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Top side (default).
    Top,
    /// Bottom side (mirrored).
    Bottom,
}

/// Arguments of `device_place`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DevicePlaceArgs {
    /// Designator (e.g. "R1") or UUID of the component.
    pub component: String,
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Position of the device origin in mm (default: keep, or the board
    /// center for a new device).
    #[serde(default)]
    pub position: Option<PointMm>,
    /// Rotation in degrees (counter-clockwise).
    #[serde(default)]
    pub rotation: Option<f64>,
    /// Board side.
    #[serde(default)]
    pub side: Option<Side>,
    /// UUID of the library device (default: the component's assembly
    /// option; another device replaces the current one).
    #[serde(default)]
    pub device: Option<String>,
    /// UUID of the footprint (default: the first footprint).
    #[serde(default)]
    pub footprint: Option<String>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `device_place`.
pub fn device_place(session: &mut Session, args: DevicePlaceArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (_, board, b) = resolve::board(p, args.board.as_deref())?;
    let (component, _) = resolve::component(p, &args.component)?;
    let existing = b
        .device(component)
        .map(|d| (d.lib_device(), d.lib_footprint()));
    let position = args.position.map(|x| x.to_point()).transpose()?;
    let rotation = args.rotation.map(angle).transpose()?;
    let mirrored = args.side.map(|s| s == Side::Bottom);
    let device = args
        .device
        .as_deref()
        .map(|d| resolve::required_uuid(d, "device"))
        .transpose()?;
    let footprint = args
        .footprint
        .as_deref()
        .map(|f| resolve::required_uuid(f, "footprint"))
        .transpose()?;
    let center = || -> Point {
        b.polygons()
            .values()
            .filter(|poly| poly.layer() == Layer::BOARD_OUTLINES)
            .flat_map(|poly| poly.path().vertices().iter().map(|v| v.pos))
            .fold(None, |acc: Option<(Point, Point)>, v| {
                Some(match acc {
                    None => (v, v),
                    Some((a, b)) => (
                        Point::new(a.x.min(v.x), a.y.min(v.y)),
                        Point::new(b.x.max(v.x), b.y.max(v.y)),
                    ),
                })
            })
            .map_or(Point::ORIGIN, |(a, b)| {
                Point::new((a.x + b.x) / 2, (a.y + b.y) / 2)
            })
    };
    let default_position = center();
    let done = write(session, "device_place", args.expected_revision, |editor| {
        match existing {
            None => {
                editor.execute(AddDevice {
                    component: component.into(),
                    board: Some(board),
                    device,
                    footprint,
                    position: position.unwrap_or(default_position),
                    rotation: rotation.unwrap_or(Angle::DEG0),
                    mirrored: mirrored.unwrap_or(false),
                })?;
            }
            Some((cur_device, cur_footprint)) => {
                let new_device = device.unwrap_or(cur_device);
                if new_device != cur_device || footprint.is_some_and(|f| f != cur_footprint) {
                    editor.execute(ReplaceDevice {
                        component: component.into(),
                        board: Some(board),
                        device: new_device,
                        footprint,
                    })?;
                }
                editor.execute(MoveDevice {
                    component: component.into(),
                    board: Some(board),
                    position,
                    rotation,
                    mirrored,
                    locked: None,
                })?;
            }
        }
        Ok(())
    })?;
    let open = session.project()?;
    let p = open.project();
    let (_, _, b) = resolve::board(p, Some(&board.0.to_string()))?;
    let device = b
        .device(component)
        .ok_or_else(|| ToolError::internal("the device is missing after placing it"))?;
    let result = device_json(p, b, device)?;
    done.output(
        open,
        format!(
            "Placed {} at ({}, {}) mm, {}°, {}.",
            result["designator"].as_str().unwrap_or_default(),
            mm(device.position().x),
            mm(device.position().y),
            views::angle(device.rotation()),
            if device.mirrored() { "bottom" } else { "top" }
        ),
        result,
    )
}

/// Arguments of `device_auto_place`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceAutoPlaceArgs {
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Components to place (default: all without device on the board).
    #[serde(default)]
    pub components: Vec<String>,
    /// Space between footprints in mm (default 1).
    #[serde(default)]
    pub spacing: Option<f64>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `device_auto_place`.
pub fn device_auto_place(
    session: &mut Session,
    args: DeviceAutoPlaceArgs,
) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let board = board_id(p, args.board.as_deref())?;
    let components: Vec<ComponentInstanceId> = args
        .components
        .iter()
        .map(|c| resolve::component(p, c).map(|(id, _)| id))
        .collect::<ToolResult<_>>()?;
    let spacing = args.spacing.map(|s| unsigned(s, "spacing")).transpose()?;
    let done = write(
        session,
        "device_auto_place",
        args.expected_revision,
        |editor| {
            Ok(editor.execute(AutoPlaceDevices {
                board: Some(board),
                components: components.into_iter().map(Into::into).collect(),
                spacing,
            })?)
        },
    )?;
    let open = session.project()?;
    let p = open.project();
    let r = &done.value;
    let names = |ids: &[ComponentInstanceId]| -> Vec<String> {
        ids.iter().map(|id| resolve::designator(p, *id)).collect()
    };
    let placed: Vec<String> = r
        .placed
        .iter()
        .map(|(id, _)| resolve::designator(p, *id))
        .collect();
    let mut result = board_overview(p, board)?;
    result["placed"] = json!(placed);
    result["outside"] = json!(names(&r.outside));
    result["skipped"] = json!(names(&r.skipped));
    let mut out = done.output(
        open,
        format!("Placed {} device(s): {}.", placed.len(), placed.join(", ")),
        result,
    )?;
    if !r.outside.is_empty() {
        out = out
            .warn(format!(
                "Not enough space inside the board outline for {}; placed right of the \
                 board. Enlarge the outline (board_set_outline) and place them with \
                 device_place.",
                names(&r.outside).join(", ")
            ))
            .partial();
    }
    Ok(out)
}

/// A trace end: a pad "R1.1" (pad or signal name), a via or junction
/// UUID, or a point in mm.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EndpointArg {
    /// A point in mm (a new junction).
    Point(PointMm),
    /// A pad "R1.1" or the UUID of a via or junction.
    Ref(String),
}

fn endpoint(p: &Project, board: BoardId, arg: &EndpointArg) -> ToolResult<TraceEndpoint> {
    match arg {
        EndpointArg::Point(pt) => Ok(TraceEndpoint::Point(pt.to_point()?)),
        EndpointArg::Ref(s) => {
            if let Some(uuid) = resolve::parse_uuid(s) {
                let b = p
                    .board(board)
                    .ok_or_else(|| ToolError::not_found("board not found"))?;
                for seg in b.net_segments().values() {
                    if seg.vias().contains_key(&uuid) {
                        return Ok(TraceEndpoint::Via(uuid));
                    }
                    if seg.junctions().contains_key(&uuid) {
                        return Ok(TraceEndpoint::Junction(uuid));
                    }
                }
                return Err(ToolError::not_found(format!(
                    "There is no via or junction {uuid} on the board."
                )));
            }
            let (designator, pad) = resolve::split_pin_ref(s)?;
            let (id, _) = resolve::component(p, designator)?;
            Ok(TraceEndpoint::Pad(PadRef {
                component: id.into(),
                pad: pad.to_owned(),
            }))
        }
    }
}

/// Arguments of `trace_add`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceAddArgs {
    /// Start: pad "R1.1", via/junction UUID, or point {x, y} in mm.
    pub from: EndpointArg,
    /// End: pad "R1.1", via/junction UUID, or point {x, y} in mm.
    pub to: EndpointArg,
    /// Corner points in mm between start and end.
    #[serde(default)]
    pub points: Vec<PointMm>,
    /// Copper layer: top, bottom, inner1.. (default: the layer of an SMT
    /// pad at an end, else top).
    #[serde(default)]
    pub layer: Option<String>,
    /// Width in mm (default: net class / design rules default).
    #[serde(default)]
    pub width: Option<f64>,
    /// Net name if both ends are points.
    #[serde(default)]
    pub net: Option<String>,
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `trace_add`.
pub fn trace_add(session: &mut Session, args: TraceAddArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let board = board_id(p, args.board.as_deref())?;
    let start = endpoint(p, board, &args.from)?;
    let end = endpoint(p, board, &args.to)?;
    let points: Vec<Point> = args
        .points
        .iter()
        .map(|x| x.to_point())
        .collect::<ToolResult<_>>()?;
    let layer = args.layer.as_deref().map(copper_layer).transpose()?;
    let width = args.width.map(|w| positive(w, "width")).transpose()?;
    let net = args
        .net
        .as_deref()
        .map(|n| resolve::net(p, n).map(|(id, _)| NetRef::Id(id)))
        .transpose()?;
    let done = write(session, "trace_add", args.expected_revision, |editor| {
        Ok(editor.execute(AddTrace {
            board: Some(board),
            start,
            end,
            points,
            layer,
            width,
            net,
        })?)
    })?;
    let open = session.project()?;
    let p = open.project();
    let r = &done.value;
    let b = p
        .board(board)
        .ok_or_else(|| ToolError::internal("board vanished"))?;
    let traces: Vec<Value> = b
        .net_segment(r.segment)
        .map(|seg| {
            r.traces
                .iter()
                .filter_map(|t| seg.traces().get(t))
                .map(|t| {
                    let p1 = b.anchor_position(seg, t.p1(), p.library(), p.circuit());
                    let p2 = b.anchor_position(seg, t.p2(), p.library(), p.circuit());
                    json!({
                        "uuid": t.uuid(),
                        "layer": t.layer().id(),
                        "width": mm(t.width()),
                        "from": p1.map(views::point),
                        "to": p2.map(views::point),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let net = r.net.map(|n| resolve::net_name(p, n));
    let unrouted = air_wires_json(p, b).len();
    done.output(
        open,
        format!(
            "Added {} trace(s) on {} (net {}); {unrouted} unrouted connection(s) left.",
            traces.len(),
            r.layer.id(),
            net.as_deref().unwrap_or("none")
        ),
        json!({
            "segment": r.segment.0,
            "net": net,
            "layer": r.layer.id(),
            "traces": traces,
            "unrouted": unrouted,
        }),
    )
}

/// Arguments of `via_add`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViaAddArgs {
    /// Position in mm.
    pub position: PointMm,
    /// Net name or UUID.
    pub net: String,
    /// Drill diameter in mm (default: design rules).
    #[serde(default)]
    pub drill: Option<f64>,
    /// Outer diameter in mm (default: from the design rules).
    #[serde(default)]
    pub size: Option<f64>,
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `via_add`.
pub fn via_add(session: &mut Session, args: ViaAddArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let board = board_id(p, args.board.as_deref())?;
    let (net, _) = resolve::net(p, &args.net)?;
    let position = args.position.to_point()?;
    let drill = args.drill.map(|d| positive(d, "drill")).transpose()?;
    let size = args.size.map(|d| positive(d, "size")).transpose()?;
    let done = write(session, "via_add", args.expected_revision, |editor| {
        Ok(editor.execute(AddVia {
            board: Some(board),
            position,
            net: Some(NetRef::Id(net)),
            start_layer: None,
            end_layer: None,
            drill_diameter: drill,
            size,
            exposure: None,
        })?)
    })?;
    let open = session.project()?;
    let p = open.project();
    let via = p
        .board(board)
        .and_then(|b| b.net_segment(done.value.segment))
        .and_then(|s| s.vias().get(&done.value.via));
    let result = json!({
        "uuid": done.value.via,
        "segment": done.value.segment.0,
        "net": resolve::net_name(p, net),
        "position": via.map(|v| views::point(v.position())),
        "drill": via.and_then(|v| v.drill_diameter()).map(mm),
        "size": via.and_then(|v| v.size()).map(mm),
    });
    done.output(
        open,
        format!("Added via {} of net {}.", done.value.via, args.net),
        result,
    )
}

/// Arguments of `trace_remove`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceRemoveArgs {
    /// Remove all traces and vias of these nets.
    #[serde(default)]
    pub nets: Vec<String>,
    /// Remove these traces or vias (UUIDs).
    #[serde(default)]
    pub uuids: Vec<String>,
    /// Remove all traces and vias of the board.
    #[serde(default)]
    pub all: bool,
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `trace_remove`.
pub fn trace_remove(session: &mut Session, args: TraceRemoveArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (_, board, b) = resolve::board(p, args.board.as_deref())?;
    let nets: Vec<_> = args
        .nets
        .iter()
        .map(|n| resolve::net(p, n).map(|(id, _)| id))
        .collect::<ToolResult<_>>()?;
    let mut selection = BoardSelection::default();
    let mut found = std::collections::BTreeSet::new();
    let uuids: Vec<_> = args
        .uuids
        .iter()
        .map(|u| resolve::required_uuid(u, "trace/via"))
        .collect::<ToolResult<_>>()?;
    for (id, seg) in b.net_segments() {
        let whole = args.all || seg.net().is_some_and(|n| nets.contains(&n));
        for t in seg.traces().keys() {
            if whole || uuids.contains(t) {
                selection.traces.push((*id, *t));
                found.insert(*t);
            }
        }
        for v in seg.vias().keys() {
            if whole || uuids.contains(v) {
                selection.vias.push((*id, *v));
                found.insert(*v);
            }
        }
    }
    if let Some(missing) = uuids.iter().find(|u| !found.contains(u)) {
        return Err(ToolError::not_found(format!(
            "There is no trace or via {missing} on the board."
        )));
    }
    if !args.all && nets.is_empty() && uuids.is_empty() {
        return Err(ToolError::invalid("Pass nets, uuids or all=true."));
    }
    let (traces, vias) = (selection.traces.len(), selection.vias.len());
    let done = write(session, "trace_remove", args.expected_revision, |editor| {
        Ok(editor.execute(RemoveBoardItems {
            board: Some(board),
            selection,
        })?)
    })?;
    let open = session.project()?;
    let result = board_overview(open.project(), board)?;
    done.output(
        open,
        format!("Removed {traces} trace(s) and {vias} via(s)."),
        result,
    )
}

/// How pads connect to a plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectStyleArg {
    /// Thermal relief spokes (default).
    Thermal,
    /// Solid connection.
    Solid,
    /// Not connected.
    None,
}

/// Arguments of `plane_add`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaneAddArgs {
    /// Net name or UUID (e.g. "GND").
    pub net: String,
    /// Copper layer: top, bottom, inner1.. (default bottom).
    #[serde(default)]
    pub layer: Option<String>,
    /// Outline vertices in mm (default: the whole board outline).
    #[serde(default)]
    pub outline: Option<Vec<PointMm>>,
    /// Clearance to other copper in mm (default 0.3).
    #[serde(default)]
    pub clearance: Option<f64>,
    /// Minimum copper width in mm (default 0.2).
    #[serde(default)]
    pub min_width: Option<f64>,
    /// Pad connection style (default thermal).
    #[serde(default)]
    pub connect_style: Option<ConnectStyleArg>,
    /// Fill priority (higher fills first).
    #[serde(default)]
    pub priority: Option<i32>,
    /// Keep unconnected islands (default false).
    #[serde(default)]
    pub keep_islands: Option<bool>,
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `plane_add`.
pub fn plane_add(session: &mut Session, args: PlaneAddArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let board = board_id(p, args.board.as_deref())?;
    let (net, _) = resolve::net(p, &args.net)?;
    let layer = copper_layer(args.layer.as_deref().unwrap_or("bottom"))?;
    let outline = match &args.outline {
        Some(points) => {
            if points.len() < 3 {
                return Err(ToolError::invalid(
                    "A plane outline needs at least 3 points.",
                ));
            }
            let points: Vec<Point> = points
                .iter()
                .map(|x| x.to_point())
                .collect::<ToolResult<_>>()?;
            Some(closed_path(&points))
        }
        None => None,
    };
    let settings = PlaneSettings {
        min_width: args
            .min_width
            .map(|w| unsigned(w, "min_width"))
            .transpose()?,
        min_clearance_to_copper: args
            .clearance
            .map(|c| unsigned(c, "clearance"))
            .transpose()?,
        keep_islands: args.keep_islands,
        priority: args.priority,
        connect_style: args.connect_style.map(|c| match c {
            ConnectStyleArg::Thermal => PlaneConnectStyle::ThermalRelief,
            ConnectStyleArg::Solid => PlaneConnectStyle::Solid,
            ConnectStyleArg::None => PlaneConnectStyle::None,
        }),
        ..PlaneSettings::default()
    };
    let done = write(session, "plane_add", args.expected_revision, |editor| {
        Ok(editor.execute(AddPlane {
            board: Some(board),
            net: Some(NetRef::Id(net)),
            layer: Some(layer),
            outline,
            settings,
        })?)
    })?;
    let open = session.project()?;
    let p = open.project();
    let plane = p.board(board).and_then(|b| b.plane(done.value));
    let result = json!({
        "uuid": done.value.0,
        "net": resolve::net_name(p, net),
        "layer": layer.id(),
        "outline": plane.map(|pl| views::path(pl.outline())),
        "min_width": plane.map(|pl| mm(pl.min_width())),
        "clearance": plane.map(|pl| mm(pl.min_clearance_to_copper())),
        "connect_style": plane.map(|pl| pl.connect_style().to_str()),
    });
    done.output(
        open,
        format!(
            "Added a {} plane on {} (fill it with planes_rebuild; drc_run and exports \
             rebuild planes automatically).",
            args.net,
            layer.id()
        ),
        result,
    )
}

/// Arguments of `design_rules_set`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DesignRulesSetArgs {
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Default trace width in mm.
    #[serde(default)]
    pub default_trace_width: Option<f64>,
    /// Default via drill diameter in mm.
    #[serde(default)]
    pub default_via_drill: Option<f64>,
    /// DRC: minimum copper to copper clearance in mm.
    #[serde(default)]
    pub min_copper_clearance: Option<f64>,
    /// DRC: minimum copper to board edge clearance in mm.
    #[serde(default)]
    pub min_copper_board_clearance: Option<f64>,
    /// DRC: minimum copper width in mm.
    #[serde(default)]
    pub min_copper_width: Option<f64>,
    /// DRC: minimum annular ring of plated holes in mm.
    #[serde(default)]
    pub min_annular_ring: Option<f64>,
    /// DRC: minimum plated drill diameter in mm.
    #[serde(default)]
    pub min_drill_diameter: Option<f64>,
    /// DRC: minimum drill to drill clearance in mm.
    #[serde(default)]
    pub min_drill_clearance: Option<f64>,
    /// Number of inner copper layers (0 = 2-layer board).
    #[serde(default)]
    pub inner_layers: Option<u32>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

fn rules_json(p: &Project, board: BoardId) -> Value {
    let Some(b) = p.board(board) else {
        return json!({});
    };
    let s = b.settings();
    let r = &s.design_rules;
    let d = &s.drc_settings;
    json!({
        "copper_layers": s.inner_layer_count + 2,
        "default_trace_width": mm(r.default_trace_width()),
        "default_via_drill": mm(r.default_via_drill_diameter()),
        "min_copper_clearance": mm(d.min_copper_copper_clearance()),
        "min_copper_board_clearance": mm(d.min_copper_board_clearance()),
        "min_copper_width": mm(d.min_copper_width()),
        "min_annular_ring": mm(d.min_pth_annular_ring()),
        "min_drill_diameter": mm(d.min_pth_drill_diameter()),
        "min_drill_clearance": mm(d.min_drill_drill_clearance()),
    })
}

/// `design_rules_set`.
pub fn design_rules_set(session: &mut Session, args: DesignRulesSetArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (_, board, b) = resolve::board(p, args.board.as_deref())?;
    let mut rules = b.settings().design_rules.clone();
    let mut drc = b.settings().drc_settings.clone();
    if let Some(v) = args.default_trace_width {
        rules.set_default_trace_width(positive(v, "default_trace_width")?);
    }
    if let Some(v) = args.default_via_drill {
        rules.set_default_via_drill_diameter(positive(v, "default_via_drill")?);
    }
    if let Some(v) = args.min_copper_clearance {
        drc.set_min_copper_copper_clearance(unsigned(v, "min_copper_clearance")?);
    }
    if let Some(v) = args.min_copper_board_clearance {
        drc.set_min_copper_board_clearance(unsigned(v, "min_copper_board_clearance")?);
    }
    if let Some(v) = args.min_copper_width {
        drc.set_min_copper_width(unsigned(v, "min_copper_width")?);
    }
    if let Some(v) = args.min_annular_ring {
        drc.set_min_pth_annular_ring(unsigned(v, "min_annular_ring")?);
    }
    if let Some(v) = args.min_drill_diameter {
        drc.set_min_pth_drill_diameter(unsigned(v, "min_drill_diameter")?);
    }
    if let Some(v) = args.min_drill_clearance {
        drc.set_min_drill_drill_clearance(unsigned(v, "min_drill_clearance")?);
    }
    let done = write(
        session,
        "design_rules_set",
        args.expected_revision,
        |editor| {
            Ok(editor.execute(EditBoardSettings {
                board: Some(board),
                settings: None,
                design_rules: Some(rules),
                drc_settings: Some(drc),
                inner_layer_count: args.inner_layers,
            })?)
        },
    )?;
    let open = session.project()?;
    let result = rules_json(open.project(), board);
    done.output(open, "Design rules updated.", result)
}

/// Routing backend of `autoroute`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouterBackend {
    /// Freerouting if it is installed (and no nets/layers filter is given),
    /// else the built-in router.
    #[default]
    Auto,
    /// The built-in grid router (2-layer boards with a few dozen nets).
    Builtin,
    /// Freerouting (external, via Specctra DSN/SES; routes the whole board
    /// and may improve existing traces).
    Freerouting,
}

/// Arguments of `autoroute`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutorouteArgs {
    /// Router backend (default auto: Freerouting if installed, else
    /// builtin).
    #[serde(default)]
    pub backend: RouterBackend,
    /// Builtin only: route only these nets (default: all unrouted
    /// connections).
    #[serde(default)]
    pub nets: Vec<String>,
    /// Builtin only: copper layers to use (default: all), e.g. ["top",
    /// "bottom"].
    #[serde(default)]
    pub layers: Vec<String>,
    /// Builtin only: trace width in mm (default: per net class / design
    /// rules).
    #[serde(default)]
    pub trace_width: Option<f64>,
    /// Builtin only: via drill in mm (default: design rules).
    #[serde(default)]
    pub via_drill: Option<f64>,
    /// Builtin only: via outer diameter in mm (default: from drill and
    /// annular ring).
    #[serde(default)]
    pub via_size: Option<f64>,
    /// Builtin only: routing grid pitch in mm (default: automatic).
    #[serde(default)]
    pub grid_pitch: Option<f64>,
    /// Freerouting only: timeout in seconds (default 600).
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// Freerouting only: maximum number of routing passes.
    #[serde(default)]
    pub max_passes: Option<u32>,
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// What `autoroute` does, decided under the session lock (see
/// [`autoroute_prepare()`]).
#[derive(Debug)]
pub enum AutoroutePlan {
    /// Run the built-in router (under the lock), with notes for the
    /// response.
    Builtin(Box<Autoroute>, Vec<String>),
    /// Run Freerouting (without lock) on the exported DSN, then import.
    Freerouting(Box<FreeroutingJob>),
}

/// A prepared Freerouting run.
#[derive(Debug)]
pub struct FreeroutingJob {
    /// The router.
    pub router: FreeroutingRouter,
    /// The DSN file for Freerouting.
    pub dsn: Vec<u8>,
    /// The export manifest (for the strict import).
    pub manifest: SpecctraExportManifest,
    /// Unrouted connections before routing.
    pub before: RoutingStats,
    /// The arguments (for the fallback to the builtin router).
    pub args: AutorouteArgs,
}

impl FreeroutingJob {
    /// Runs Freerouting (blocking; no session lock needed).
    pub fn run(&self) -> Result<FreeroutingOutput, FreeroutingError> {
        self.router.run(&self.dsn)
    }
}

fn builtin_command(p: &Project, board: BoardId, args: &AutorouteArgs) -> ToolResult<Autoroute> {
    let nets = if args.nets.is_empty() {
        None
    } else {
        Some(
            args.nets
                .iter()
                .map(|n| resolve::net(p, n).map(|(id, _)| NetRef::Id(id)))
                .collect::<ToolResult<Vec<_>>>()?,
        )
    };
    let layers = if args.layers.is_empty() {
        None
    } else {
        Some(
            args.layers
                .iter()
                .map(|l| copper_layer(l))
                .collect::<ToolResult<Vec<_>>>()?,
        )
    };
    Ok(Autoroute {
        board: Some(board),
        nets,
        layers,
        trace_width: args
            .trace_width
            .map(|v| positive(v, "trace_width"))
            .transpose()?,
        via_drill: args
            .via_drill
            .map(|v| positive(v, "via_drill"))
            .transpose()?,
        via_size: args.via_size.map(|v| positive(v, "via_size")).transpose()?,
        grid_pitch: args
            .grid_pitch
            .map(|v| positive(v, "grid_pitch"))
            .transpose()?,
        via_cost: None,
    })
}

/// Decides the backend of `autoroute` and prepares it: validates the
/// arguments and the revision, detects Freerouting and exports the DSN.
pub fn autoroute_prepare(session: &Session, args: AutorouteArgs) -> ToolResult<AutoroutePlan> {
    let p = session.project()?.project();
    crate::tools::write::check_revision(p, args.expected_revision)?;
    let board = board_id(p, args.board.as_deref())?;
    let filtered = !args.nets.is_empty() || !args.layers.is_empty();
    let mut notes = Vec::new();
    let router = match args.backend {
        RouterBackend::Builtin => None,
        RouterBackend::Freerouting => {
            if filtered {
                return Err(ToolError::invalid(
                    "Freerouting routes the whole board; nets and layers are only supported \
                     by the builtin backend.",
                ));
            }
            Some(FreeroutingRouter::detect().map_err(|e| {
                ToolError::new(
                    ErrorKind::NotAvailable,
                    format!("Freerouting is not available: {e}"),
                )
            })?)
        }
        RouterBackend::Auto if filtered => None,
        RouterBackend::Auto => match FreeroutingRouter::detect() {
            Ok(r) => Some(r),
            Err(e) => {
                notes.push(format!(
                    "Freerouting is not available ({e}); used the builtin router."
                ));
                None
            }
        },
    };
    match router {
        None => Ok(AutoroutePlan::Builtin(
            Box::new(builtin_command(p, board, &args)?),
            notes,
        )),
        Some(router) => {
            let mut config = router.config().clone();
            if let Some(t) = args.timeout_s {
                config.timeout = std::time::Duration::from_secs(t.max(1));
            }
            if args.max_passes.is_some() {
                config.max_passes = args.max_passes;
            }
            let export = export_specctra_dsn(p, Some(board))?;
            Ok(AutoroutePlan::Freerouting(Box::new(FreeroutingJob {
                router: FreeroutingRouter::new(config),
                dsn: dsn_for_freerouting(&export.dsn),
                before: routing_stats(p, board),
                manifest: export.manifest,
                args,
            })))
        }
    }
}

/// Runs the built-in router (one undo group).
pub fn autoroute_builtin(
    session: &mut Session,
    command: Autoroute,
    expected_revision: Option<u64>,
    notes: Vec<String>,
) -> ToolResult<ToolOutput> {
    let board = command
        .board
        .ok_or_else(|| ToolError::internal("no board"))?;
    let done = write(session, "autoroute", expected_revision, |editor| {
        Ok(editor.execute(command)?)
    })?;
    let open = session.project()?;
    let p = open.project();
    let r = &done.value;
    let remaining: Vec<Value> = r
        .remaining_air_wires
        .iter()
        .map(|aw| {
            json!({
                "net": aw.net_name,
                "from": views::point(aw.p1_position),
                "to": views::point(aw.p2_position),
                "reason": aw.reason,
            })
        })
        .collect();
    let mut result = board_overview(p, board)?;
    result["backend"] = json!("builtin");
    result["air_wires"] = json!(r.air_wires);
    result["routed"] = json!(r.routed);
    result["failed"] = json!(r.unrouted);
    result["new_traces"] = json!(r.traces);
    result["new_vias"] = json!(r.vias);
    result["grid_pitch"] = json!(mm(Length::new(r.grid_pitch)));
    result["remaining"] = json!(remaining);
    let out = done
        .output(
            open,
            format!(
                "Autorouted (builtin) {}/{} connection(s): {} trace(s), {} via(s){}.",
                r.routed,
                r.air_wires,
                r.traces,
                r.vias,
                if remaining.is_empty() {
                    String::new()
                } else {
                    format!(", {} connection(s) remain unrouted", remaining.len())
                }
            ),
            result,
        )?
        .warnings(notes);
    Ok(if remaining.is_empty() {
        out
    } else {
        out.partial().warn(
            "Some connections could not be routed: move devices apart (device_place), \
             route them manually (trace_add, via_add) or retry with other layers or a finer \
             grid_pitch.",
        )
    })
}

/// Imports the result of Freerouting (one undo group, strict: fails with
/// `stale_revision` if the project changed while Freerouting ran). If
/// Freerouting failed and the backend was `auto`, the built-in router runs
/// instead.
pub fn autoroute_freerouting_finish(
    session: &mut Session,
    job: FreeroutingJob,
    output: Result<FreeroutingOutput, FreeroutingError>,
) -> ToolResult<ToolOutput> {
    let board = job.manifest.board;
    let revision = job.manifest.revision;
    let output = match output {
        Ok(o) => o,
        Err(e) if job.args.backend == RouterBackend::Auto => {
            let p = session.project()?.project();
            let command = builtin_command(p, board, &job.args)?;
            return autoroute_builtin(
                session,
                command,
                Some(revision),
                vec![format!(
                    "Freerouting failed ({e}); used the builtin router."
                )],
            );
        }
        Err(e) => {
            return Err(ToolError::internal(format!("Freerouting failed: {e}")));
        }
    };
    let elapsed = output.elapsed;
    let manifest = job.manifest.clone();
    let done = write(session, "autoroute", Some(revision), |editor| {
        Ok(editor.execute(ImportSpecctraSession {
            board: Some(board),
            session: output.session,
            manifest: Some(manifest),
        })?)
    })?;
    let open = session.project()?;
    let p = open.project();
    let after = routing_stats(p, board);
    let r = &done.value;
    let mut result = board_overview(p, board)?;
    result["backend"] = json!("freerouting");
    result["unrouted_before"] = json!(job.before.air_wires);
    result["unrouted_after"] = json!(after.air_wires);
    result["new_objects"] = json!(r.new_objects);
    result["seconds"] = json!(elapsed.as_secs_f64());
    let warnings: Vec<String> = r
        .messages
        .iter()
        .filter(|m| m.level == MessageLevel::Warning)
        .map(|m| m.message.clone())
        .collect();
    let out = done
        .output(
            open,
            format!(
                "Autorouted with Freerouting in {:.1} s: {} -> {} unrouted connection(s).",
                elapsed.as_secs_f64(),
                job.before.air_wires,
                after.air_wires
            ),
            result,
        )?
        .warnings(warnings);
    Ok(if after.air_wires == 0 {
        out
    } else {
        out.partial().warn(
            "Some connections are still unrouted (see unrouted): route them with trace_add, \
             or retry with backend \"builtin\".",
        )
    })
}

/// `autoroute` in one call (the session stays locked while Freerouting
/// runs; the MCP server runs the steps separately).
pub fn autoroute(session: &mut Session, args: AutorouteArgs) -> ToolResult<ToolOutput> {
    let expected = args.expected_revision;
    match autoroute_prepare(session, args)? {
        AutoroutePlan::Builtin(command, notes) => {
            autoroute_builtin(session, *command, expected, notes)
        }
        AutoroutePlan::Freerouting(job) => {
            let output = job.run();
            autoroute_freerouting_finish(session, *job, output)
        }
    }
}

/// Arguments of `specctra_export`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecctraExportArgs {
    /// Output file (default: `<project>/output/<version>/<board>.dsn`).
    #[serde(default)]
    pub path: Option<String>,
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
}

/// `specctra_export`.
pub fn specctra_export(session: &Session, args: SpecctraExportArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (_, board, b) = resolve::board(p, args.board.as_deref())?;
    let export = export_specctra_dsn(p, Some(board))?;
    let clean = |s: &str| {
        FilePath::clean_file_name(s, CleanFileNameOptions::new(true, FileNameCase::Keep), 120)
    };
    let path = match &args.path {
        Some(path) => crate::session::absolute_path(path)?,
        None => p
            .path()
            .ok_or_else(|| ToolError::internal("the project has no directory"))?
            .path_to(&format!(
                "output/{}/{}.dsn",
                clean(p.metadata().version.as_str()),
                clean(b.name().as_str())
            )),
    };
    if let Some(parent) = path.as_path().parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ToolError::new(ErrorKind::Io, e.to_string()))?;
    }
    std::fs::write(path.as_path(), &export.dsn)
        .map_err(|e| ToolError::new(ErrorKind::Io, e.to_string()))?;
    ToolOutput::new(
        format!(
            "Exported the Specctra DSN of board \"{}\" to {} ({} components, {} nets).",
            b.name().as_str(),
            path.to_native(),
            export.manifest.components.len(),
            export.manifest.nets.len()
        ),
        json!({
            "path": path.to_native(),
            "bytes": export.dsn.len(),
            "components": export.manifest.components.len(),
            "nets": export.manifest.nets.len(),
        }),
    )
}

/// Arguments of `specctra_import`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecctraImportArgs {
    /// The Specctra session file (*.ses), e.g. written by an external
    /// autorouter from a specctra_export file.
    pub path: String,
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `specctra_import`.
pub fn specctra_import(session: &mut Session, args: SpecctraImportArgs) -> ToolResult<ToolOutput> {
    let board = board_id(session.project()?.project(), args.board.as_deref())?;
    let path = crate::session::absolute_path(&args.path)?;
    let content = std::fs::read(path.as_path())
        .map_err(|e| ToolError::new(ErrorKind::Io, format!("{}: {e}", path.to_native())))?;
    let content = String::from_utf8(content)
        .map_err(|_| ToolError::invalid("The session file is not valid UTF-8."))?;
    let done = write(
        session,
        "specctra_import",
        args.expected_revision,
        |editor| {
            Ok(editor.execute(ImportSpecctraSession {
                board: Some(board),
                session: content,
                manifest: None,
            })?)
        },
    )?;
    let open = session.project()?;
    let mut result = board_overview(open.project(), board)?;
    result["updated_components"] = json!(done.value.updated_components);
    result["new_objects"] = json!(done.value.new_objects);
    let warnings: Vec<String> = done
        .value
        .messages
        .iter()
        .filter(|m| m.level == MessageLevel::Warning)
        .map(|m| m.message.clone())
        .collect();
    Ok(done
        .output(open, "Imported the Specctra session.", result)?
        .warnings(warnings))
}

/// Arguments of `planes_rebuild` and `unrouted`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardArgs {
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
}

/// `planes_rebuild`.
pub fn planes_rebuild(session: &mut Session, args: BoardArgs) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    let board = board_id(open.project(), args.board.as_deref())?;
    let fragments = open
        .editor
        .update_derived_data(|p| p.rebuild_planes(board, None))??;
    let p = open.project();
    let planes: Vec<Value> = fragments
        .iter()
        .map(|(id, paths)| {
            let plane = p.board(board).and_then(|b| b.plane(*id));
            json!({
                "uuid": id.0,
                "net": plane.and_then(|pl| pl.net()).map(|n| resolve::net_name(p, n)),
                "layer": plane.map(|pl| pl.layer().id()),
                "fragments": paths.len(),
            })
        })
        .collect();
    let empty = planes.iter().filter(|p| p["fragments"] == json!(0)).count();
    let out = ToolOutput::new(
        format!(
            "Rebuilt {} plane(s){}.",
            planes.len(),
            if empty > 0 {
                format!(", {empty} without copper")
            } else {
                String::new()
            }
        ),
        json!({ "planes": planes }),
    )?;
    Ok(if empty > 0 {
        out.warn("Planes without fragments have no copper (no connected pads or no space).")
    } else {
        out
    })
}

/// `unrouted`.
pub fn unrouted(session: &Session, args: BoardArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (_, _, b) = resolve::board(p, args.board.as_deref())?;
    let air_wires = air_wires_json(p, b);
    let unplaced: Vec<String> = p
        .circuit()
        .component_instances()
        .iter()
        .filter(|(id, c)| {
            b.device(**id).is_none()
                && p.library()
                    .component(&c.lib_component())
                    .is_some_and(|lc| !lc.schematic_only())
        })
        .map(|(_, c)| c.name().as_str().to_owned())
        .collect();
    let lines: Vec<String> = air_wires
        .iter()
        .take(50)
        .map(|a| {
            format!(
                "{}: {} -> {}",
                a["net"].as_str().unwrap_or_default(),
                a["from"]["pad"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| a["from"]["position"].to_string()),
                a["to"]["pad"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| a["to"]["position"].to_string()),
            )
        })
        .collect();
    let mut summary = format!(
        "{} unrouted connection(s), {} unplaced device(s).",
        air_wires.len(),
        unplaced.len()
    );
    if !lines.is_empty() {
        summary.push('\n');
        summary.push_str(&lines.join("\n"));
    }
    ToolOutput::new(
        summary,
        json!({ "air_wires": air_wires, "count": air_wires.len(), "unplaced_devices": unplaced }),
    )
}
