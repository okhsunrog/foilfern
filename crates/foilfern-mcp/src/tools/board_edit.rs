//! Board tools: `board_add`, `board_set_outline`, `board_polygon_add`, `device_place`,
//! `device_auto_place`, `trace_add`, `trace_update`, `via_add`, `trace_remove`,
//! `plane_add`, `design_rules_set`, `autoroute` (write tools, one undo
//! group each, see [`write`]), `specctra_import`, `planes_rebuild`
//! (derived data only), `unrouted` and `specctra_export` (read).

use std::collections::BTreeSet;

use foilfern_core::fileio::{CleanFileNameOptions, FileNameCase, FilePath};
use foilfern_core::geometry::Path;
use foilfern_core::project::board::{
    BoardItem, BoardStrokeTextData, PlaneConnectStyle, PlaneFragmentDiagnostic,
    PlaneFragmentStatus, PlaneJob,
};
use foilfern_core::project::{
    BoardId, BoardMutation, ComponentInstanceId, Mutation, PlaneId, Project,
};
use foilfern_core::types::{
    Angle, ElementName, Layer, Length, Point, PositiveLength, UnsignedLength,
};
use foilfern_editor::board_requirements::RoutingGuard;
use foilfern_editor::commands::{
    AddBoard, AddBoardPolygon, AddDevice, AddPlane, AddTrace, AddVia, AutoPlaceDevices, Autoroute,
    BoardSelection, EditPlane, EditTraceWidths, ImportSpecctraSession, MessageLevel, MoveDevice,
    NetRef, PadRef, PlaneSettings, RemoveBoardItems, ReplaceDevice, SetBoardOutline,
    SpecctraExportManifest, TraceEndpoint, closed_path, export_specctra_dsn,
};
use foilfern_editor::routing_validation::{
    RoutingSnapshot, RoutingValidation, capture_routing_state,
};
use foilfern_editor::{
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
pub use crate::tools::design_rules::{DesignRulesSetArgs, set as design_rules_set};
use crate::tools::layout::{air_wires_json, air_wires_json_with_fill, current_plane_fill};
use crate::tools::write::write;
use crate::units::{PointMm, angle, length, mm};
use crate::views;

pub(crate) fn positive(mm: f64, what: &str) -> ToolResult<PositiveLength> {
    PositiveLength::new(length(mm)?)
        .map_err(|_| ToolError::invalid(format!("{what} must be greater than zero")))
}

pub(super) fn unsigned(mm: f64, what: &str) -> ToolResult<UnsignedLength> {
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
pub(crate) fn board_id(p: &Project, key: Option<&str>) -> ToolResult<BoardId> {
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
    let done = write(session, "board_add", args.expected_revision, |tx| {
        let mut command = AddBoard::new(name);
        command.copy_from = copy_from;
        Ok(tx.run(command)?)
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
    let done = write(session, "board_set_outline", args.expected_revision, |tx| {
        Ok(tx.run(SetBoardOutline {
            board: Some(board),
            outline,
        })?)
    })?;
    let open = session.project()?;
    let result = board_overview(open.project(), board)?;
    done.output(open, "Board outline set.", result)
}

/// Arguments of `board_polygon_add`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardPolygonAddArgs {
    /// Board name, index or UUID (default: first board).
    pub board: Option<String>,
    /// LibrePCB board layer ID, e.g. top_legend, top_documentation, or brd_cutouts.
    /// Copper polygons have no net; use plane_add for a net-connected pour.
    pub layer: String,
    /// At least three distinct vertices in board millimeters. Closed automatically.
    pub vertices: Vec<PointMm>,
    /// Stroke width in mm. Default: 0.2 for a stroked graphic, zero for fills or cuts.
    pub line_width: Option<f64>,
    /// Fill the polygon (default false).
    #[serde(default)]
    pub filled: bool,
    /// Fail with stale_revision if the project revision differs.
    pub expected_revision: Option<u64>,
}

pub fn board_polygon_add(
    session: &mut Session,
    args: BoardPolygonAddArgs,
) -> ToolResult<ToolOutput> {
    let (_, board, model) = resolve::board(session.project()?.project(), args.board.as_deref())?;
    let layer = Layer::from_id(args.layer.trim())
        .filter(|layer| layer.is_board())
        .ok_or_else(|| {
            ToolError::invalid("Use a board layer ID, e.g. top_legend or brd_cutouts.")
        })?;
    if layer.is_copper() && !model.copper_layers().contains(&layer) {
        return Err(ToolError::invalid(
            "The selected copper layer is not present on this board.",
        ));
    }
    let mut points = args
        .vertices
        .iter()
        .copied()
        .map(PointMm::to_point)
        .collect::<ToolResult<Vec<_>>>()?;
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    if points.len() < 3 || points.iter().collect::<BTreeSet<_>>().len() != points.len() {
        return Err(ToolError::invalid(
            "A polygon needs at least three distinct vertices.",
        ));
    }
    let path = closed_path(&points);
    let line_width = unsigned(
        args.line_width
            .unwrap_or(if layer.is_board_edge() || args.filled {
                0.0
            } else {
                0.2
            }),
        "line_width",
    )?;
    let done = write(session, "board_polygon_add", args.expected_revision, |tx| {
        Ok(tx.run(AddBoardPolygon {
            board: Some(board),
            layer,
            path: path.clone(),
            line_width: Some(line_width),
            filled: args.filled,
            grab_area: false,
        })?)
    })?;
    done.output(
        session.project()?,
        "Added a closed polygon to the board.",
        json!({
            "uuid":done.value, "board":board, "layer":layer.id(), "path":views::path(&path),
            "line_width":mm(line_width), "filled":args.filled,
        }),
    )
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardLegendArgs {
    pub board: Option<String>,
}

fn legend_json(text: &BoardStrokeTextData, component: Option<String>) -> Value {
    json!({
        "uuid": text.uuid(),
        "component": component,
        "text": text.text(),
        "layer": text.layer().id(),
        "position": views::point(text.position()),
        "rotation": text.rotation().to_deg(),
        "height": mm(text.height()),
        "locked": text.locked(),
    })
}

pub fn board_legend_list(session: &Session, args: BoardLegendArgs) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    let (_, id, board) = resolve::board(project, args.board.as_deref())?;
    let is_printed = |text: &&BoardStrokeTextData| {
        let settings = board.settings();
        settings.silkscreen_layers_top.contains(&text.layer())
            || settings.silkscreen_layers_bot.contains(&text.layer())
    };
    let mut texts: Vec<_> = board
        .stroke_texts()
        .values()
        .filter(is_printed)
        .map(|text| legend_json(text, None))
        .collect();
    for device in board.devices().values() {
        texts.extend(
            device
                .stroke_texts()
                .values()
                .filter(is_printed)
                .map(|text| {
                    legend_json(text, Some(resolve::designator(project, device.component())))
                }),
        );
    }
    ToolOutput::new(
        "Board and footprint silkscreen text.",
        json!({"board":id,"texts":texts}),
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardTextMoveArgs {
    pub board: Option<String>,
    /// UUID from board_legend_list.
    pub uuid: String,
    /// Device designator or UUID. Omit for standalone board text.
    pub component: Option<String>,
    /// New position in board millimeters.
    pub position: PointMm,
    /// Optional absolute rotation in degrees.
    pub rotation: Option<f64>,
    pub expected_revision: Option<u64>,
}

pub fn board_text_move(session: &mut Session, args: BoardTextMoveArgs) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    let (_, id, board) = resolve::board(project, args.board.as_deref())?;
    let uuid = resolve::required_uuid(&args.uuid, "board text")?;
    let mut device = args
        .component
        .as_deref()
        .map(|key| {
            let (component, _) = resolve::component(project, key)?;
            board
                .device(component)
                .cloned()
                .ok_or_else(|| ToolError::not_found("Device is not placed."))
        })
        .transpose()?;
    let texts = device
        .as_ref()
        .map_or(board.stroke_texts(), |d| d.stroke_texts());
    let mut text = texts
        .get(&uuid)
        .cloned()
        .ok_or_else(|| ToolError::not_found("Board text not found."))?;
    if text.locked() {
        return Err(ToolError::new(ErrorKind::Refused, "Board text is locked."));
    }
    text.set_position(args.position.to_point()?);
    if let Some(rotation) = args.rotation {
        text.set_rotation(angle(rotation)?);
    }
    let result = legend_json(
        &text,
        device
            .as_ref()
            .map(|d| resolve::designator(project, d.component())),
    );
    let mutation = match device.as_mut() {
        Some(device) => {
            device.insert_stroke_text(text);
            BoardMutation::UpdateDevice {
                board: id,
                device: device.clone(),
            }
        }
        None => BoardMutation::UpdateItem {
            board: id,
            item: BoardItem::StrokeText(text),
        },
    };
    let done = write(session, "board_text_move", args.expected_revision, |tx| {
        Ok(tx.apply(Mutation::Board(mutation))?)
    })?;
    done.output(session.project()?, "Moved board text.", result)
}

/// Board side of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize, JsonSchema)]
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
    /// Include the device's pad geometry in the response (default true).
    #[serde(default)]
    pub include_pads: Option<bool>,
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
    let done = write(session, "device_place", args.expected_revision, |tx| {
        let guard = RoutingGuard::capture(tx.project(), board)?;
        match existing {
            None => {
                tx.run(AddDevice {
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
                    tx.run(ReplaceDevice {
                        component: component.into(),
                        board: Some(board),
                        device: new_device,
                        footprint,
                    })?;
                }
                tx.run(MoveDevice {
                    component: component.into(),
                    board: Some(board),
                    position,
                    rotation,
                    mirrored,
                    locked: None,
                })?;
            }
        }
        guard.enforce(tx.project())?;
        Ok(())
    })?;
    let open = session.project()?;
    let p = open.project();
    let (_, _, b) = resolve::board(p, Some(&board.0.to_string()))?;
    let device = b
        .device(component)
        .ok_or_else(|| ToolError::internal("the device is missing after placing it"))?;
    let result = device_json(p, b, device, args.include_pads.unwrap_or(true))?;
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
    let done = write(session, "device_auto_place", args.expected_revision, |tx| {
        let guard = RoutingGuard::capture(tx.project(), board)?;
        let result = tx.run(AutoPlaceDevices {
            board: Some(board),
            components: components.into_iter().map(Into::into).collect(),
            spacing,
        })?;
        guard.enforce(tx.project())?;
        Ok(result)
    })?;
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

pub(super) fn endpoint(
    p: &Project,
    board: BoardId,
    arg: &EndpointArg,
) -> ToolResult<TraceEndpoint> {
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
    let done = write(session, "trace_add", args.expected_revision, |tx| {
        let guard = RoutingGuard::capture(tx.project(), board)?;
        let result = tx.run(AddTrace {
            board: Some(board),
            start,
            end,
            points,
            layer,
            width,
            net,
        })?;
        guard.enforce(tx.project())?;
        Ok(result)
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

/// Arguments of `trace_update`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceUpdateArgs {
    /// UUIDs of existing traces; can be combined with nets.
    #[serde(default)]
    pub uuids: Vec<String>,
    /// Change all traces of these net names or UUIDs.
    #[serde(default)]
    pub nets: Vec<String>,
    /// New width in millimeters. UUIDs and connected geometry are preserved.
    pub width: f64,
    #[serde(default)]
    pub board: Option<String>,
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// `trace_update`: grouped width changes, with applied-model DRC evidence.
pub fn trace_update(session: &mut Session, args: TraceUpdateArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    crate::tools::write::check_revision(p, args.expected_revision)?;
    let (_, board, b) = resolve::board(p, args.board.as_deref())?;
    let width = positive(args.width, "width")?;
    let nets = args
        .nets
        .iter()
        .map(|n| resolve::net(p, n).map(|(id, _)| id))
        .collect::<ToolResult<BTreeSet<_>>>()?;
    let mut traces = args
        .uuids
        .iter()
        .map(|u| resolve::required_uuid(u, "trace"))
        .collect::<ToolResult<BTreeSet<_>>>()?;
    for segment in b.net_segments().values() {
        if segment.net().is_some_and(|net| nets.contains(&net)) {
            traces.extend(segment.traces().keys().copied());
        }
    }
    if traces.is_empty() {
        return Err(ToolError::invalid(
            "Select existing traces with uuids or nets.",
        ));
    }
    let existing: BTreeSet<_> = b
        .net_segments()
        .values()
        .flat_map(|s| s.traces().keys().copied())
        .collect();
    if let Some(missing) = traces.difference(&existing).next() {
        return Err(ToolError::not_found(format!(
            "There is no trace {missing} on the board."
        )));
    }
    let before = capture_routing_state(&mut session.project_mut()?.editor, board)?;
    let revision = session.project()?.project().revision();
    let done = write(session, "trace_update", Some(revision), |tx| {
        let guard = RoutingGuard::capture(tx.project(), board)?;
        let result = tx.run(EditTraceWidths {
            board: Some(board),
            traces: traces.into_iter().collect(),
            width,
        })?;
        guard.enforce(tx.project())?;
        Ok(result)
    })?;
    let validation = RoutingValidation::compare(
        before,
        capture_routing_state(&mut session.project_mut()?.editor, board)?,
    );
    let open = session.project()?;
    let p = open.project();
    let b = p
        .board(board)
        .ok_or_else(|| ToolError::not_found("Board no longer exists."))?;
    let changed_ids: BTreeSet<_> = done.value.iter().copied().collect();
    let mut changed = Vec::new();
    for (segment, data) in b.net_segments() {
        for trace in data
            .traces()
            .values()
            .filter(|t| changed_ids.contains(&t.uuid()))
        {
            changed.push(json!({
                "uuid": trace.uuid(), "segment": segment.0,
                "net": data.net().map(|n| resolve::net_name(p, n)),
                "width": mm(trace.width()), "layer": trace.layer().id(),
            }));
        }
    }
    let out = done.output(
        open,
        format!("Updated {} trace width(s).", changed.len()),
        json!({"traces": changed}),
    )?;
    Ok(with_routing_validation(out, validation))
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
    let done = write(session, "via_add", args.expected_revision, |tx| {
        let guard = RoutingGuard::capture(tx.project(), board)?;
        let result = tx.run(AddVia {
            board: Some(board),
            position,
            net: Some(NetRef::Id(net)),
            start_layer: None,
            end_layer: None,
            drill_diameter: drill,
            size,
            exposure: None,
        })?;
        guard.enforce(tx.project())?;
        Ok(result)
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
    let done = write(session, "trace_remove", args.expected_revision, |tx| {
        let guard = RoutingGuard::capture(tx.project(), board)?;
        let result = tx.run(RemoveBoardItems {
            board: Some(board),
            selection,
        })?;
        guard.enforce(tx.project())?;
        Ok(result)
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
    let done = write(session, "plane_add", args.expected_revision, |tx| {
        Ok(tx.run(AddPlane {
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

/// Arguments of `plane_set_outline`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaneSetOutlineArgs {
    /// UUID of an existing copper plane from board_get.
    pub plane: String,
    /// Outline vertices in board millimeters. Closed automatically.
    pub outline: Vec<PointMm>,
    /// Board name, index or UUID (default: first board).
    pub board: Option<String>,
    /// Fail with stale_revision if the project revision differs.
    pub expected_revision: Option<u64>,
}

/// Edit the outline of an existing pour while retaining its ID and rules.
pub fn plane_set_outline(
    session: &mut Session,
    args: PlaneSetOutlineArgs,
) -> ToolResult<ToolOutput> {
    let board = board_id(session.project()?.project(), args.board.as_deref())?;
    let plane = PlaneId(resolve::required_uuid(&args.plane, "plane")?);
    let points = args
        .outline
        .into_iter()
        .map(PointMm::to_point)
        .collect::<ToolResult<Vec<_>>>()?;
    if points.iter().collect::<BTreeSet<_>>().len() < 3 {
        return Err(ToolError::invalid(
            "A plane outline needs at least three distinct points.",
        ));
    }
    let done = write(session, "plane_set_outline", args.expected_revision, |tx| {
        Ok(tx.run(EditPlane {
            board: Some(board),
            plane,
            net: None,
            layer: None,
            outline: Some(closed_path(&points)),
            settings: PlaneSettings::default(),
        })?)
    })?;
    let open = session.project()?;
    let updated = open
        .project()
        .board(board)
        .and_then(|board| board.plane(plane))
        .ok_or_else(|| ToolError::not_found("Updated plane was not found."))?;
    done.output(
        open,
        "Updated plane outline. Rebuild planes to inspect the fill.",
        json!({
            "uuid": plane.0,
            "layer": updated.layer().id(),
            "net": updated.net().map(|id| resolve::net_name(open.project(), id)),
            "outline": views::path(updated.outline()),
        }),
    )
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
    /// Full validation of the model exported to Freerouting.
    pub validation_before: RoutingSnapshot,
    /// The arguments (for the fallback to the builtin router).
    pub args: AutorouteArgs,
}

impl FreeroutingJob {
    /// Runs Freerouting (blocking; no session lock needed).
    pub fn run(&self) -> Result<FreeroutingOutput, FreeroutingError> {
        self.router.run(&self.dsn)
    }
}

pub(crate) fn builtin_command(
    p: &Project,
    board: BoardId,
    args: &AutorouteArgs,
) -> ToolResult<Autoroute> {
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
        search_evidence: false,
        detour_search: false,
        pad_escape: false,
        neck_down: None,
        congestion_history: false,
        order: None,
        critical_nets: Vec::new(),
        region: None,
    })
}

/// Decides the backend of `autoroute` and prepares it: validates the
/// arguments and the revision, detects Freerouting and exports the DSN.
pub fn autoroute_prepare(session: &mut Session, args: AutorouteArgs) -> ToolResult<AutoroutePlan> {
    let p = session.project()?.project();
    crate::tools::write::check_revision(p, args.expected_revision)?;
    let board = board_id(p, args.board.as_deref())?;
    let requirements = foilfern_core::project::board_requirements::Requirements::load(p)
        .map_err(foilfern_editor::Error::from)?;
    let constrained = requirements.on_board(board).any(|rule| {
        matches!(
            rule.check,
            foilfern_core::project::board_requirements::Check::NetRouting { .. }
        )
    });
    let filtered = !args.nets.is_empty() || !args.layers.is_empty();
    let mut notes = Vec::new();
    let router = match args.backend {
        RouterBackend::Builtin => None,
        RouterBackend::Freerouting => {
            if constrained {
                return Err(ToolError::invalid(
                    "Use the builtin router to search with this board's saved net routing rules.",
                ));
            }
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
        RouterBackend::Auto if filtered || constrained => None,
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
            let validation_before =
                capture_routing_state(&mut session.project_mut()?.editor, board)?;
            let p = session.project()?.project();
            let export = export_specctra_dsn(p, Some(board))?;
            Ok(AutoroutePlan::Freerouting(Box::new(FreeroutingJob {
                router: FreeroutingRouter::new(config),
                dsn: dsn_for_freerouting(&export.dsn),
                before: routing_stats(p, board),
                validation_before,
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
    crate::tools::write::check_revision(session.project()?.project(), expected_revision)?;
    let before = capture_routing_state(&mut session.project_mut()?.editor, board)?;
    // Full DRC can publish derived-data changes. Check the caller's revision
    // before it, then use the current revision while holding the same lock.
    let revision = session.project()?.project().revision();
    let done = write(session, "autoroute", Some(revision), |tx| {
        let guard = RoutingGuard::capture(tx.project(), board)?;
        let result = tx.run(command)?;
        guard.enforce(tx.project())?;
        Ok(result)
    })?;
    let validation = RoutingValidation::compare(
        before,
        capture_routing_state(&mut session.project_mut()?.editor, board)?,
    );
    let open = session.project()?;
    let p = open.project();
    let r = &done.value;
    let remaining: Vec<Value> = r
        .remaining_air_wires
        .iter()
        .map(|aw| {
            let mut wire = json!({
                "net": aw.net_name,
                "from": views::point(aw.p1_position),
                "to": views::point(aw.p2_position),
                "reason": aw.reason,
            });
            if let Some(reason) = &aw.soft_reason {
                wire["soft_reason"] = json!(reason);
            }
            wire
        })
        .collect();
    let mut result = board_overview(p, board)?;
    result["backend"] = json!("builtin");
    result["air_wires"] = json!(r.air_wires);
    result["routed"] = json!(r.routed);
    result["failed"] = json!(r.unrouted);
    result["failed_requests"] = json!(r.failed_requests);
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
    let out = if remaining.is_empty() {
        out
    } else {
        out.partial().warn(
            "Some connections could not be routed: move devices apart (device_place), \
             route them manually (trace_add, via_add) or retry with other layers or a finer \
             grid_pitch.",
        )
    };
    Ok(with_routing_validation(out, validation))
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
    let done = write(session, "autoroute", Some(revision), |tx| {
        let guard = RoutingGuard::capture(tx.project(), board)?;
        let result = tx.run(ImportSpecctraSession {
            board: Some(board),
            session: output.session,
            manifest: Some(manifest),
        })?;
        guard.enforce(tx.project())?;
        Ok(result)
    })?;
    let validation = RoutingValidation::compare(
        job.validation_before,
        capture_routing_state(&mut session.project_mut()?.editor, board)?,
    );
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
    let out = if after.air_wires == 0 {
        out
    } else {
        out.partial().warn(
            "Some connections are still unrouted (see unrouted): route them with trace_add, \
             or retry with backend \"builtin\".",
        )
    };
    Ok(with_routing_validation(out, validation))
}

fn with_routing_validation(mut out: ToolOutput, validation: RoutingValidation) -> ToolOutput {
    out.summary.push('\n');
    out.summary.push_str(&validation.summary());
    out.result["unrouted_before"] = json!(validation.before.unrouted);
    out.result["unrouted_after"] = json!(validation.after.unrouted);
    if !validation.acceptable {
        out = out.partial();
    }
    if !validation.before.complete || !validation.after.complete {
        out = out.warn("Full before/after DRC did not complete; the applied board change has not been validated (see validation.before.check_failures and validation.after.check_failures).");
    }
    if !validation.netlist_preserved {
        out = out.warn(
            "The operation changed the circuit netlist. Inspect the result or undo the operation.",
        );
    }
    if validation.new_error_count > 0 {
        out = out.warn(format!("The operation introduced {} unapproved DRC error(s), including {} copper error(s). The result is kept as one undo step; inspect validation.new_errors or undo it.",
            validation.new_error_count, validation.new_copper_error_count));
    } else if !validation.drc_clean {
        out = out.warn("The applied board is not DRC clean; see validation.after and run drc_run for all messages.");
    }
    out.result["validation"] = json!(validation);
    out
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
    crate::tools::write::check_revision(session.project()?.project(), args.expected_revision)?;
    let before = capture_routing_state(&mut session.project_mut()?.editor, board)?;
    let revision = session.project()?.project().revision();
    let done = write(session, "specctra_import", Some(revision), |tx| {
        let guard = RoutingGuard::capture(tx.project(), board)?;
        let result = tx.run(ImportSpecctraSession {
            board: Some(board),
            session: content,
            manifest: None,
        })?;
        guard.enforce(tx.project())?;
        Ok(result)
    })?;
    let validation = RoutingValidation::compare(
        before,
        capture_routing_state(&mut session.project_mut()?.editor, board)?,
    );
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
    let out = done
        .output(open, "Imported the Specctra session.", result)?
        .warnings(warnings);
    Ok(with_routing_validation(out, validation))
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

/// Arguments for a read-only plane fill diagnostic.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaneDiagnoseArgs {
    /// UUID of a copper plane from board_get.
    pub plane: String,
    pub board: Option<String>,
    pub expected_revision: Option<u64>,
    /// Zero-based fragment offset. Discarded islands are listed first.
    #[serde(default)]
    pub offset: usize,
    /// Number of fragments to return, from 1 to 64 (default 16).
    pub limit: Option<usize>,
    /// Also report which copper fragment contains this board point in mm.
    pub probe: Option<PointMm>,
}

pub struct PlaneDiagnosticJob {
    job: PlaneJob,
    plane: PlaneId,
    metadata: Value,
    offset: usize,
    limit: usize,
    probe: Option<Point>,
}

pub(super) fn plane_fragment_status(kind: PlaneFragmentStatus) -> (usize, &'static str) {
    match kind {
        PlaneFragmentStatus::IslandDiscarded => (0, "island_discarded"),
        PlaneFragmentStatus::IslandKept => (1, "island_kept"),
        PlaneFragmentStatus::Connected => (2, "connected"),
        PlaneFragmentStatus::Netless => (3, "netless"),
    }
}

pub fn prepare_plane_diagnostic(
    session: &Session,
    args: PlaneDiagnoseArgs,
) -> ToolResult<PlaneDiagnosticJob> {
    let project = session.project()?.project();
    super::write::check_revision(project, args.expected_revision)?;
    let (_, board, model) = resolve::board(project, args.board.as_deref())?;
    let plane = PlaneId(resolve::required_uuid(&args.plane, "plane")?);
    let model = model
        .plane(plane)
        .ok_or_else(|| ToolError::not_found("Plane was not found on this board."))?;
    let limit = args.limit.unwrap_or(16);
    if !(1..=64).contains(&limit) {
        return Err(ToolError::invalid("limit must be between 1 and 64."));
    }
    let metadata = json!({
        "project": project.uuid(),
        "base_revision": project.revision(),
        "board": board,
        "plane": plane,
        "layer": model.layer().id(),
        "net": model.net().map(|net| resolve::net_name(project, net)),
        "keep_islands": model.keep_islands(),
    });
    let job = project
        .plane_job(board, None)?
        .ok_or_else(|| ToolError::internal("The board has no planes to fill."))?;
    Ok(PlaneDiagnosticJob {
        job,
        plane,
        metadata,
        offset: args.offset,
        limit,
        probe: args.probe.map(PointMm::to_point).transpose()?,
    })
}

pub fn run_plane_diagnostic(work: PlaneDiagnosticJob) -> ToolResult<ToolOutput> {
    let PlaneDiagnosticJob {
        job,
        plane,
        mut metadata,
        offset,
        limit,
        probe,
    } = work;
    let mut fragments: Vec<_> = job
        .diagnose(plane)?
        .into_iter()
        .map(|fragment| (fragment.area_mm2(), fragment))
        .collect();
    let status = plane_fragment_status;
    fragments.sort_by_key(|(_, fragment)| status(fragment.status).0);
    let mut counts = [0; 4];
    let mut areas = [0.0; 4];
    for (area, fragment) in &fragments {
        let index = status(fragment.status).0;
        counts[index] += 1;
        areas[index] += area;
    }
    let describe = |(area, fragment): &(f64, PlaneFragmentDiagnostic)| {
        let vertices = fragment.outline.vertices();
        json!({
            "status": status(fragment.status).1,
            "area_mm2": area,
            "bounds": {
                "min": {
                    "x": vertices.iter().map(|v| mm(v.pos.x)).reduce(f64::min),
                    "y": vertices.iter().map(|v| mm(v.pos.y)).reduce(f64::min),
                },
                "max": {
                    "x": vertices.iter().map(|v| mm(v.pos.x)).reduce(f64::max),
                    "y": vertices.iter().map(|v| mm(v.pos.y)).reduce(f64::max),
                },
            },
        })
    };
    metadata["fragment_count"] = json!(fragments.len());
    metadata["totals"] = json!({
        "island_discarded": { "count": counts[0], "area_mm2": areas[0] },
        "island_kept": { "count": counts[1], "area_mm2": areas[1] },
        "connected": { "count": counts[2], "area_mm2": areas[2] },
        "netless": { "count": counts[3], "area_mm2": areas[3] },
    });
    metadata["fragments"] = json!(
        fragments
            .iter()
            .skip(offset)
            .take(limit)
            .map(describe)
            .collect::<Vec<_>>()
    );
    let next = offset.saturating_add(limit);
    metadata["next_offset"] = json!((next < fragments.len()).then_some(next));
    if let Some(point) = probe {
        let hit = fragments
            .iter()
            .enumerate()
            .find(|(_, (_, fragment))| fragment.contains(point));
        metadata["probe"] = json!({
            "point": views::point(point),
            "fragment_offset": hit.map(|(index, _)| index),
            "status": hit.map(|(_, (_, fragment))| status(fragment.status).1),
        });
    }
    ToolOutput::new(
        format!(
            "Inspected {} fragments, {} discarded islands.",
            fragments.len(),
            counts[0]
        ),
        metadata,
    )
}

pub fn finish_plane_diagnostic(session: &Session, report: ToolOutput) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    if report.result["project"] != json!(project.uuid()) {
        return Err(ToolError::new(
            ErrorKind::StaleRevision,
            "The open project changed during plane diagnosis.",
        ));
    }
    super::write::check_revision(project, report.result["base_revision"].as_u64())?;
    Ok(report)
}

/// `unrouted`.
pub fn unrouted(session: &Session, args: BoardArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (_, _, b) = resolve::board(p, args.board.as_deref())?;
    let fill = current_plane_fill(p, b)?;
    let air_wires = air_wires_json_with_fill(p, b, fill.as_ref());
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
