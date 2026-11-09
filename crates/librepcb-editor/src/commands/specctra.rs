//! Specctra DSN export and session (SES) import: port of
//! libs/librepcb/editor/project/cmd/cmdboardspecctraimport.{h,cpp} (the DSN
//! export itself is the core's
//! [`BoardSpecctraExport`](librepcb_core::project::board::BoardSpecctraExport)).
//!
//! [`ImportSpecctraSession`] replaces all net segments of a board by the
//! routes of a Specctra session (e.g. from FreeRouting): traces, vias and
//! junctions are anchored to footprint pads, board pads and vias like
//! upstream; UUIDs of unchanged objects are kept. Device placements are
//! updated if the session moved or rotated them.
//!
//! The session is parsed with the core S-expression parser (permissive
//! mode), like upstream. The `topola_specctra` crate cannot be used: its
//! session model has no `base_design`, `placement` and `was_is` nodes and
//! rejects FreeRouting's session files (tested with FreeRouting 1.x and
//! 2.4.x output), and the placement is needed for the checks below.
//!
//! In addition to upstream, the import can be *strict*: with the
//! [`SpecctraExportManifest`] recorded at export time
//! ([`export_specctra_dsn()`]), the session is validated completely before
//! the board is modified: the project must not have changed since the
//! export, the placement must be unchanged (no moved, rotated, flipped,
//! unknown or missing components), and all nets and via padstacks must be
//! the exported ones. The FreeRouting integration always imports strictly.
//!
//! Differences to upstream:
//! - Messages are returned in [`SpecctraImportResult::messages`] (and logged
//!   with the `log` crate) instead of a `MessageLogger`.
//! - Net segments without net are identified by their exported dummy net
//!   name (`~anonymous~<segment UUID>`), so each keeps its own pads; upstream
//!   treats all of them as one net and adds the pads of all segments
//!   without net to each of them.
//! - Only footprint pads of this board are used as anchors (upstream also
//!   considers the pads of the same component on other boards).
//! - Via padstack names mangled by FreeRouting 2.x (which drops the
//!   fractional part of decimal numbers, e.g. `via-0.3:auto-0.7:auto-tht`
//!   becomes `via-0:auto-0:auto-tht`) are mapped back to the exported names,
//!   so the drill diameter, size and exposure encoded in them are restored
//!   (upstream falls back to automatic values).
//! - Traces are matched to old traces independently of the direction of the
//!   points; zero-length wire segments (both ends at the same anchor) are
//!   skipped instead of creating an invalid trace.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::{ComponentSide, Junction, Path, Trace, TraceAnchor, Vertex, Via};
use librepcb_core::project::board::{
    ANONYMOUS_NET_PREFIX, BoardNetSegment, BoardNetSegmentSplitter, BoardPadData,
    BoardSpecctraExport, SPECCTRA_HOST_CAD, SPECCTRA_HOST_VERSION, via_padstack_id,
};
use librepcb_core::project::{
    BoardId, BoardMutation, BoardNetSegmentRef, ComponentInstanceId, Mutation, NetSegmentId,
    NetSignalId, Project,
};
use librepcb_core::serialization::{Mode, SExpression};
use librepcb_core::types::{Angle, Layer, Length, MaskConfig, Point, PositiveLength, Uuid};
use librepcb_i18n::tr;

use super::{ComponentRef, MoveDevice, resolve};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

/// Translation context of the upstream command.
const CTX: &str = "librepcb::editor::CmdBoardSpecctraImport";

/// Level of an import message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MessageLevel {
    /// Debug information.
    Debug,
    /// Information.
    Info,
    /// Warning.
    Warning,
}

/// A message of the import (upstream `MessageLogger` entry).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportMessage {
    /// The level.
    pub level: MessageLevel,
    /// The (translated) message.
    pub message: String,
}

/// Placement of an exported component, see [`SpecctraExportManifest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExportedComponent {
    /// Position.
    pub position: Point,
    /// Rotation.
    pub rotation: Angle,
    /// Whether the device is on the bottom side.
    pub mirrored: bool,
    /// Number of footprint pads (routers drop components without pads).
    pub pads: usize,
}

/// What a Specctra DSN export contained, for the strict validation of the
/// resulting session (see the module documentation).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpecctraExportManifest {
    /// The exported board.
    pub board: BoardId,
    /// The project revision at export time.
    pub revision: u64,
    /// The components by designator.
    pub components: BTreeMap<String, ExportedComponent>,
    /// The net names (including the dummy nets of segments without net).
    pub nets: BTreeSet<String>,
    /// The via padstack IDs.
    pub via_padstacks: BTreeSet<String>,
}

/// Result of [`export_specctra_dsn()`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecctraExport {
    /// The DSN file content.
    pub dsn: Vec<u8>,
    /// What the file contains, for a strict import.
    pub manifest: SpecctraExportManifest,
}

/// Exports a board (default: the primary board) as Specctra DSN (upstream
/// "Export Specctra DSN") and records a manifest for the strict import of
/// the resulting session.
pub fn export_specctra_dsn(project: &Project, board: Option<BoardId>) -> Result<SpecctraExport> {
    let b = resolve::board(project, board)?;
    let export = BoardSpecctraExport::new(project, b.id()).map_err(Error::from_export)?;
    let dsn = export.generate().map_err(Error::from_export)?;
    let mut components = BTreeMap::new();
    for device in b.devices().values() {
        let name = resolve::component_name(project, device.component());
        let pads = device.pads(project.library(), project.circuit())?.len();
        components.insert(
            name,
            ExportedComponent {
                position: device.position(),
                rotation: device.rotation(),
                mirrored: device.mirrored(),
                pads,
            },
        );
    }
    let mut nets: BTreeSet<String> = project
        .circuit()
        .net_signals()
        .values()
        .map(|n| n.name().to_string())
        .collect();
    nets.extend(
        b.net_segments()
            .values()
            .filter(|s| s.net().is_none())
            .map(|s| librepcb_core::project::board::segment_net_name(project, s)),
    );
    Ok(SpecctraExport {
        dsn,
        manifest: SpecctraExportManifest {
            board: b.id(),
            revision: project.revision(),
            components,
            nets,
            via_padstacks: export.via_padstack_ids().into_iter().collect(),
        },
    })
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Front,
    Back,
}

#[derive(Debug, Clone)]
struct ComponentOut {
    name: String,
    pos: Point,
    side: Side,
    rot: Angle,
}

#[derive(Debug, Clone, Copy)]
struct PadStackOut {
    start_layer: Layer,
    end_layer: Layer,
    diameter: Length,
}

#[derive(Debug, Clone)]
struct ViaOut {
    pad_stack_id: String,
    pos: Point,
}

#[derive(Debug, Clone)]
struct WireOut {
    layer: Layer,
    width: Length,
    path: Path,
}

#[derive(Debug, Clone)]
struct NetOut {
    net_name: String,
    vias: Vec<ViaOut>,
    wires: Vec<WireOut>,
}

/// A parsed session (upstream: the members of `CmdBoardSpecctraImport`).
#[derive(Debug, Clone)]
struct Session {
    components: Option<Vec<ComponentOut>>,
    pad_stacks: BTreeMap<String, PadStackOut>,
    nets: Vec<NetOut>,
}

fn runtime_error(msg: impl Into<String>) -> Error {
    Error::InvalidArgument(msg.into())
}

/// Allow some percentual deviation due to floating point inaccuracy:
/// 25nm <= 0.001% <= 1um (upstream `fuzzyCompare()`).
fn fuzzy_point(exact: Point, imported: Point) -> bool {
    let max_dim = imported.x.max(imported.y);
    let epsilon = (max_dim / 100_000).clamp(Length::new(25), Length::new(1000));
    *(exact - imported).length() < epsilon
}

fn fuzzy_angle(a: Angle, b: Angle) -> bool {
    (a - b).mapped_to_180deg().abs() < Angle::new(100) // 100 micro degrees.
}

fn node_value(node: &SExpression, path: &str) -> Result<String> {
    Ok(node.required_child(path)?.value()?.to_owned())
}

fn parse_double(value: &str) -> Result<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| runtime_error(format!("Invalid number: {value}")))
}

fn parse_length(node: &SExpression, resolution: f64) -> Result<Length> {
    let value = parse_double(node.value()?)?;
    Length::from_mm(value / resolution).map_err(|e| runtime_error(e.to_string()))
}

fn parse_side(node: &SExpression) -> Result<Side> {
    match node.value()? {
        "front" => Ok(Side::Front),
        "back" => Ok(Side::Back),
        other => Err(runtime_error(format!("Invalid board side: {other}"))),
    }
}

fn parse_angle(node: &SExpression) -> Result<Angle> {
    let mut angle =
        Angle::from_deg(parse_double(node.value()?)?).map_err(|e| runtime_error(e.to_string()))?;
    let multiple = Angle::new(1_000_000);
    if fuzzy_angle(Angle::DEG0, angle % multiple) {
        angle = angle.rounded(multiple);
    }
    Ok(angle)
}

fn parse_layer(node: &SExpression) -> Result<Layer> {
    let id = node.value()?;
    Layer::from_id(id)
        .filter(|l| l.is_copper())
        .ok_or_else(|| runtime_error(format!("Invalid layer: \"{id}\"")))
}

/// Returns the resolution (units per millimeter) and its log text
/// (upstream `getResolution()`).
fn resolution(node: &SExpression) -> Result<(f64, String)> {
    let value = node_value(node, "resolution/@1")?;
    let mut resolution = parse_double(&value)?;
    let unit = node_value(node, "resolution/@0")?;
    if unit == "um" {
        resolution /= 1000.0;
    } else if unit != "mm" {
        return Err(runtime_error(format!("Unsupported unit: '{unit}'")));
    }
    if resolution <= 0.0 {
        return Err(runtime_error(format!("Invalid number: {value}")));
    }
    Ok((resolution, format!("1/{value} {unit}")))
}

fn lists(node: &SExpression) -> impl Iterator<Item = &SExpression> {
    node.children().iter().filter(|c| c.is_list())
}

impl Session {
    /// Parses a session (upstream constructor of `CmdBoardSpecctraImport`).
    fn parse(root: &SExpression, board_copper: &BTreeSet<Layer>, log: &mut Log) -> Result<Self> {
        // Check file type.
        if root.name().ok() != Some("session") {
            return Err(runtime_error(tr!(
                CTX,
                "The specified file is not a Specctra session (SES)."
            )));
        }

        // Check parser.
        let host_cad = root
            .child("routes/parser/host_cad/@0")
            .and_then(|c| c.value().ok())
            .unwrap_or_default();
        let host_version = root
            .child("routes/parser/host_version/@0")
            .and_then(|c| c.value().ok())
            .unwrap_or_default();
        if host_cad.is_empty() {
            log.warning("Specctra session doesn't specify host CAD, compatibility is unknown.");
        } else if (host_cad != SPECCTRA_HOST_CAD) || (host_version != SPECCTRA_HOST_VERSION) {
            log.warning(format!(
                "Specctra session originates from {host_cad} {host_version}, compatibility is unknown."
            ));
        }

        // Parse placement.
        let components = match root.child("placement") {
            Some(child) => {
                let (resolution, log_res) = resolution(child)?;
                log.debug(format!("Placement resolution: {log_res}"));
                let mut components = Vec::new();
                for cmp in child.children_named("component") {
                    let places: Vec<_> = cmp.children_named("place").collect();
                    let [node] = places.as_slice() else {
                        return Err(runtime_error("Unexpected component placement count."));
                    };
                    components.push(ComponentOut {
                        name: node_value(node, "@0")?,
                        pos: Point::new(
                            parse_length(node.required_child("@1")?, resolution)?,
                            parse_length(node.required_child("@2")?, resolution)?,
                        ),
                        side: parse_side(node.required_child("@3")?)?,
                        rot: parse_angle(node.required_child("@4")?)?,
                    });
                }
                Some(components)
            }
            None => {
                log.warning("Specctra session doesn't contain component placement data.");
                None
            }
        };

        let routes = root.required_child("routes")?;
        let (resolution, log_res) = resolution(routes)?;
        log.debug(format!("Routing resolution: {log_res}"));

        // Parse pad stacks.
        let mut pad_stacks = BTreeMap::new();
        if let Some(library) = routes.child("library_out") {
            for node in library.children_named("padstack") {
                let name = node_value(node, "@0")?;
                if pad_stacks.contains_key(&name) {
                    return Err(runtime_error(format!(
                        "Pad stack '{name}' defined multiple times."
                    )));
                }
                let mut diameters = BTreeSet::new();
                let mut layers = Vec::new();
                for shape in lists(node) {
                    for child in lists(shape) {
                        if child.name()? != "circle" {
                            return Err(runtime_error(format!(
                                "Unsupported pad stack shape '{}'.",
                                shape.name()?
                            )));
                        }
                        layers.push(parse_layer(child.required_child("@0")?)?);
                        diameters.insert(parse_length(child.required_child("@1")?, resolution)?);
                    }
                }
                if diameters.len() != 1 {
                    return Err(runtime_error("Unsupported complex pad stack."));
                }
                let diameter = diameters.first().copied().unwrap_or_default();
                if layers.len() < 2 {
                    return Err(runtime_error("Too few layers in pad stack."));
                }
                layers.sort_by_key(|l| l.copper_number());
                let (first, last) = (layers[0], layers[layers.len() - 1]);
                for layer in board_copper {
                    if (layer.copper_number() > first.copper_number())
                        && (layer.copper_number() < last.copper_number())
                        && !layers.contains(layer)
                    {
                        return Err(runtime_error("Missing layers in pad stack."));
                    }
                }
                pad_stacks.insert(
                    name,
                    PadStackOut {
                        start_layer: first,
                        end_layer: last,
                        diameter,
                    },
                );
            }
        }

        // Parse networks.
        let mut nets = Vec::new();
        if let Some(network) = routes.child("network_out") {
            for net_node in network.children_named("net") {
                let mut net = NetOut {
                    net_name: node_value(net_node, "@0")?,
                    vias: Vec::new(),
                    wires: Vec::new(),
                };
                for via in net_node.children_named("via") {
                    let pad_stack_id = node_value(via, "@0")?;
                    if !pad_stacks.contains_key(&pad_stack_id) {
                        return Err(runtime_error(format!(
                            "Pad stack '{pad_stack_id}' not found."
                        )));
                    }
                    let pos = Point::new(
                        parse_length(via.required_child("@1")?, resolution)?,
                        parse_length(via.required_child("@2")?, resolution)?,
                    );
                    net.vias.push(ViaOut { pad_stack_id, pos });
                }
                for wire in net_node.children_named("wire") {
                    for path in wire.children_named("path") {
                        let children: Vec<&SExpression> = path
                            .children()
                            .iter()
                            .filter(|c| c.is_token() || c.is_string())
                            .collect();
                        if (children.len() < 2) || !children.len().is_multiple_of(2) {
                            return Err(runtime_error(format!(
                                "Unexpected number of vertices ({}) in wire path of net '{}'.",
                                children.len(),
                                net.net_name
                            )));
                        }
                        let mut wire = WireOut {
                            layer: parse_layer(children[0])?,
                            width: parse_length(children[1], resolution)?,
                            path: Path::default(),
                        };
                        if wire.width <= Length::ZERO {
                            log.warning("Skippted wire with zero width.");
                            continue;
                        }
                        for xy in children[2..].chunks(2) {
                            let x = parse_length(xy[0], resolution)?;
                            let y = parse_length(xy[1], resolution)?;
                            wire.path.vertices_mut().push(Vertex::at(Point::new(x, y)));
                        }
                        if wire.path.vertices().len() < 2 {
                            return Err(runtime_error("Path contains too few vertices."));
                        }
                        net.wires.push(wire);
                    }
                }
                nets.push(net);
            }
        }

        log.debug(tr!(CTX, "Specctra session file parsed successfully."));
        Ok(Self {
            components,
            pad_stacks,
            nets,
        })
    }

    /// Restores via padstack names mangled by the router (see
    /// [`mangled_pad_stack_name()`]): a padstack name which is not one of
    /// the `known` (exported) names is replaced by the known name with the
    /// same mangled form and the same size; if several match, the default
    /// via is preferred, else the first one in sort order.
    fn restore_pad_stack_names(&mut self, known: &BTreeSet<String>, default: &str, log: &mut Log) {
        let mut renamed = BTreeMap::new();
        for (name, pad_stack) in &self.pad_stacks {
            if known.contains(name) {
                continue;
            }
            let candidates: Vec<&String> = known
                .iter()
                .filter(|k| mangled_pad_stack_name(k) == *name)
                .filter(|k| {
                    extract_via_dimension(k, 2).is_none_or(|(_, size)| *size == pad_stack.diameter)
                })
                .collect();
            let chosen = candidates
                .iter()
                .find(|c| c.as_str() == default)
                .or(candidates.first());
            if let Some(chosen) = chosen {
                log.debug(format!(
                    "Pad stack '{name}' from Specctra session restored as '{chosen}'."
                ));
                renamed.insert(name.clone(), (*chosen).clone());
            }
        }
        for (from, to) in renamed {
            if self.pad_stacks.contains_key(&to) {
                continue; // Keep both, the original name wins.
            }
            if let Some(pad_stack) = self.pad_stacks.remove(&from) {
                self.pad_stacks.insert(to.clone(), pad_stack);
            }
            for net in &mut self.nets {
                for via in &mut net.vias {
                    if via.pad_stack_id == from {
                        via.pad_stack_id = to.clone();
                    }
                }
            }
        }
    }

    /// The strict checks against the export manifest (see the module
    /// documentation), done before anything is modified.
    fn validate(
        &self,
        p: &Project,
        board: BoardId,
        manifest: &SpecctraExportManifest,
    ) -> Result<()> {
        if manifest.board != board {
            return Err(runtime_error(
                "The Specctra session belongs to another board than the export.",
            ));
        }
        if manifest.revision != p.revision() {
            return Err(runtime_error(
                "The project has been modified since the Specctra export, the session cannot be imported safely.",
            ));
        }
        let components = self.components.as_ref().ok_or_else(|| {
            runtime_error("Specctra session doesn't contain component placement data.")
        })?;
        let mut seen = BTreeSet::new();
        for item in components {
            if item.name == "BOARD" {
                continue;
            }
            let exported = manifest.components.get(&item.name).ok_or_else(|| {
                runtime_error(format!(
                    "The Specctra session contains the unknown component '{}'.",
                    item.name
                ))
            })?;
            if !seen.insert(item.name.as_str()) {
                return Err(runtime_error(format!(
                    "The Specctra session contains the component '{}' multiple times.",
                    item.name
                )));
            }
            let side = if exported.mirrored {
                Side::Back
            } else {
                Side::Front
            };
            if (item.side != side)
                || !fuzzy_point(exported.position, item.pos)
                || !fuzzy_angle(exported.rotation, item.rot)
            {
                return Err(runtime_error(format!(
                    "The Specctra session changes the placement of component '{}'.",
                    item.name
                )));
            }
        }
        let missing: Vec<&str> = manifest
            .components
            .iter()
            .filter(|(name, c)| (c.pads > 0) && !seen.contains(name.as_str()))
            .map(|(name, _)| name.as_str())
            .collect();
        if !missing.is_empty() {
            return Err(runtime_error(format!(
                "The Specctra session does not contain the component(s) {}.",
                missing.join(", ")
            )));
        }
        for name in self.pad_stacks.keys() {
            if !manifest.via_padstacks.contains(name) {
                return Err(runtime_error(format!(
                    "The Specctra session contains the unknown via pad stack '{name}'."
                )));
            }
        }
        for net in &self.nets {
            if !manifest.nets.contains(&net.net_name) {
                return Err(runtime_error(format!(
                    "The Specctra session contains the unknown net '{}'.",
                    net.net_name
                )));
            }
        }
        Ok(())
    }
}

/// Returns a via padstack name as FreeRouting 2.x writes it into the
/// session: the fractional part of every decimal number is dropped (e.g.
/// `via-0.3:auto-0.7:auto-tht` becomes `via-0:auto-0:auto-tht`).
fn mangled_pad_stack_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let decimal_point = (c == '.')
            && (i > 0)
            && chars[i - 1].is_ascii_digit()
            && chars.get(i + 1).is_some_and(char::is_ascii_digit);
        if decimal_point {
            i += 1;
            while chars.get(i).is_some_and(char::is_ascii_digit) {
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Parses the drill diameter or size from a via padstack ID (upstream
/// `extractViaDimension()`); returns whether it is automatic.
fn extract_via_dimension(pad_stack_id: &str, index: usize) -> Option<(bool, PositiveLength)> {
    // Note: Keep in sync with `via_padstack_id()` of the export.
    let tokens: Vec<&str> = pad_stack_id.split('-').collect();
    if (tokens.len() >= 4) && (tokens[0] == "via") {
        let token = tokens[index];
        let auto = token.contains("auto");
        let value = token.split(':').next().unwrap_or_default();
        let length = value.parse::<Length>().ok()?;
        return PositiveLength::new(length).ok().map(|l| (auto, l));
    }
    None
}

/// Parses the exposure configuration from a via padstack ID (upstream
/// `extractViaExposureConfig()`).
fn extract_via_exposure_config(pad_stack_id: &str) -> Option<MaskConfig> {
    let tokens: Vec<&str> = pad_stack_id.split('-').collect();
    if (tokens.len() >= 4) && (tokens[0] == "via") {
        let exposure = tokens[tokens.len() - 1];
        if exposure == "exposed" {
            return Some(MaskConfig::Automatic);
        } else if let Some(offset) = exposure.strip_prefix("exposed:") {
            return offset.parse::<Length>().ok().map(MaskConfig::Manual);
        } else {
            return Some(MaskConfig::Off);
        }
    }
    None
}

/// Collects the messages of an import.
#[derive(Debug, Default)]
struct Log {
    messages: Vec<ImportMessage>,
}

impl Log {
    fn add(&mut self, level: MessageLevel, message: impl Into<String>) {
        let message = message.into();
        match level {
            MessageLevel::Debug => log::debug!("{message}"),
            MessageLevel::Info => log::info!("{message}"),
            MessageLevel::Warning => log::warn!("{message}"),
        }
        self.messages.push(ImportMessage { level, message });
    }

    fn debug(&mut self, message: impl Into<String>) {
        self.add(MessageLevel::Debug, message);
    }

    fn info(&mut self, message: impl Into<String>) {
        self.add(MessageLevel::Info, message);
    }

    fn warning(&mut self, message: impl Into<String>) {
        self.add(MessageLevel::Warning, message);
    }
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// The net a session net is imported into: a net signal, or (for the dummy
/// nets of segments without net) the old segment without net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NetKey {
    Net(NetSignalId),
    Anonymous(Option<Uuid>),
}

impl NetKey {
    fn net(self) -> Option<NetSignalId> {
        match self {
            Self::Net(n) => Some(n),
            Self::Anonymous(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct OldJunction {
    uuid: Uuid,
    pos: Point,
    layer: Option<Layer>,
}

#[derive(Debug, Clone, Copy)]
struct OldTrace {
    uuid: Uuid,
    p1: Point,
    p2: Point,
    layer: Layer,
    width: PositiveLength,
}

#[derive(Debug, Clone)]
struct OldSegment {
    uuid: Uuid,
    key: NetKey,
    junctions: Vec<OldJunction>,
    traces: Vec<OldTrace>,
    vias: Vec<Via>,
    pads: Vec<BoardPadData>,
}

impl OldSegment {
    fn contains_any(&self, refs: &BTreeSet<Uuid>) -> bool {
        self.junctions.iter().any(|j| refs.contains(&j.uuid))
            || self.traces.iter().any(|t| refs.contains(&t.uuid))
            || self.vias.iter().any(|v| refs.contains(&v.uuid()))
            || self.pads.iter().any(|p| refs.contains(&p.uuid()))
    }
}

/// Finds old objects for new objects to keep their UUIDs (upstream helper
/// lambdas of `performExecute()`).
struct Matcher<'a> {
    old: &'a [OldSegment],
    reused: BTreeSet<Uuid>,
    new_uuids: usize,
}

impl Matcher<'_> {
    fn segments(&self, key: NetKey) -> impl Iterator<Item = &OldSegment> {
        self.old.iter().filter(move |s| s.key == key)
    }

    fn net_point(&mut self, key: NetKey, pos: Point, layer: Layer) -> Option<OldJunction> {
        let found = self
            .segments(key)
            .flat_map(|s| s.junctions.iter())
            .find(|j| {
                fuzzy_point(j.pos, pos)
                    && (j.layer == Some(layer))
                    && !self.reused.contains(&j.uuid)
            })
            .copied();
        match found {
            Some(j) => {
                self.reused.insert(j.uuid);
                Some(j)
            }
            None => {
                self.new_uuids += 1;
                None
            }
        }
    }

    fn net_line_impl(
        &mut self,
        key: NetKey,
        p1: Point,
        p2: Point,
        layer: Layer,
        width: Option<Length>,
    ) -> Option<OldTrace> {
        let found = self
            .segments(key)
            .flat_map(|s| s.traces.iter())
            .find(|t| {
                let same = (fuzzy_point(t.p1, p1) && fuzzy_point(t.p2, p2))
                    || (fuzzy_point(t.p1, p2) && fuzzy_point(t.p2, p1));
                same && (t.layer == layer)
                    && width.is_none_or(|w| *t.width == w)
                    && !self.reused.contains(&t.uuid)
            })
            .copied();
        if let Some(t) = found {
            self.reused.insert(t.uuid);
        }
        found
    }

    fn net_line(
        &mut self,
        key: NetKey,
        p1: Point,
        p2: Point,
        layer: Layer,
        width: Length,
    ) -> Option<OldTrace> {
        // First try to match including trace width, then ignore it because it
        // might have been changed during the DSN -> SES roundtrip.
        let found = self
            .net_line_impl(key, p1, p2, layer, Some(width))
            .or_else(|| self.net_line_impl(key, p1, p2, layer, None));
        if found.is_none() {
            self.new_uuids += 1;
        }
        found
    }

    fn via(&mut self, key: NetKey, pos: Point, start: Layer, end: Layer) -> Option<Via> {
        let found = self
            .segments(key)
            .flat_map(|s| s.vias.iter())
            .find(|v| {
                fuzzy_point(v.position(), pos)
                    && (v.start_layer() == start)
                    && (v.end_layer() == end)
                    && !self.reused.contains(&v.uuid())
            })
            .cloned();
        match found {
            Some(v) => {
                self.reused.insert(v.uuid());
                Some(v)
            }
            None => {
                self.new_uuids += 1;
                None
            }
        }
    }

    fn net_segment(&mut self, key: NetKey, refs: &BTreeSet<Uuid>) -> Option<Uuid> {
        let found = self
            .segments(key)
            .find(|s| s.contains_any(refs) && !self.reused.contains(&s.uuid))
            .map(|s| s.uuid);
        match found {
            Some(uuid) => {
                self.reused.insert(uuid);
                Some(uuid)
            }
            None => {
                self.new_uuids += 1;
                None
            }
        }
    }
}

/// An anchor for new traces (upstream `AnchorData`).
#[derive(Debug, Clone, Copy)]
struct AnchorData {
    pos: Point,
    start_layer: Layer,
    end_layer: Layer,
    anchor: TraceAnchor,
}

/// A footprint pad of the board which may be an anchor.
#[derive(Debug, Clone, Copy)]
struct PadInfo {
    component: ComponentInstanceId,
    uuid: Uuid,
    net: Option<NetSignalId>,
    pos: Point,
    tht: bool,
    solder_layer: Layer,
}

/// Result of [`ImportSpecctraSession`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpecctraImportResult {
    /// The board.
    pub board: BoardId,
    /// Components moved or rotated by the session.
    pub updated_components: usize,
    /// Components of the session which were not modified.
    pub unmodified_components: usize,
    /// Number of new net objects (segments, junctions, traces, vias).
    pub new_objects: usize,
    /// Number of net objects whose UUID was kept.
    pub reused_objects: usize,
    /// The net segments of the board after the import.
    pub segments: Vec<NetSegmentId>,
    /// Messages (warnings about skipped data, statistics).
    pub messages: Vec<ImportMessage>,
}

/// Imports a Specctra session (SES), e.g. from FreeRouting (upstream
/// `CmdBoardSpecctraImport`, see the module documentation): all net
/// segments of the board are replaced by the routes of the session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportSpecctraSession {
    /// The board (default: the primary board).
    #[serde(default)]
    pub board: Option<BoardId>,
    /// The content of the session file.
    pub session: String,
    /// The manifest of the export for a strict import (default: upstream's
    /// lenient import, which also applies placement changes).
    #[serde(default)]
    pub manifest: Option<SpecctraExportManifest>,
}

impl Command for ImportSpecctraSession {
    type Output = SpecctraImportResult;

    fn text(&self) -> String {
        tr!(CTX, "Import From Specctra Session")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<SpecctraImportResult> {
        let mut log = Log::default();
        let p = tx.project();
        let b = resolve::board(p, self.board)?;
        let board = b.id();
        let root = SExpression::parse(self.session.as_bytes(), None, Mode::Permissive)?;
        let mut session = Session::parse(&root, &b.copper_layers(), &mut log)?;
        let known_pad_stacks = match &self.manifest {
            Some(manifest) => manifest.via_padstacks.clone(),
            None => BoardSpecctraExport::new(p, board)
                .map_err(Error::from_export)?
                .via_padstack_ids()
                .into_iter()
                .collect(),
        };
        let rules = b.design_rules();
        let drill = rules.default_via_drill_diameter();
        let default_pad_stack = via_padstack_id(
            drill,
            true,
            Via::calc_size_from_rules(drill, &rules.via_annular_ring()),
            true,
            Layer::TOP_COPPER,
            Layer::BOT_COPPER,
            MaskConfig::Off,
        );
        session.restore_pad_stack_names(&known_pad_stacks, &default_pad_stack, &mut log);
        if let Some(manifest) = &self.manifest {
            session.validate(p, board, manifest)?;
        }

        // Memorize the current board to allow reusing their properties.
        let mut old_segments = Vec::new();
        for seg in b.net_segments().values() {
            let key = match seg.net() {
                Some(n) => NetKey::Net(n),
                None => NetKey::Anonymous(Some(seg.uuid())),
            };
            let pos = |a| {
                b.anchor_position(seg, a, p.library(), p.circuit())
                    .ok_or_else(|| Error::not_found("Trace anchor", seg.uuid()))
            };
            let mut traces = Vec::new();
            for t in seg.traces().values() {
                traces.push(OldTrace {
                    uuid: t.uuid(),
                    p1: pos(t.p1())?,
                    p2: pos(t.p2())?,
                    layer: t.layer(),
                    width: t.width(),
                });
            }
            old_segments.push(OldSegment {
                uuid: seg.uuid(),
                key,
                junctions: seg
                    .junctions()
                    .values()
                    .map(|j| OldJunction {
                        uuid: j.uuid(),
                        pos: j.position(),
                        layer: seg.junction_layer(&j.uuid()),
                    })
                    .collect(),
                traces,
                vias: seg.vias().values().cloned().collect(),
                pads: seg.pads().values().cloned().collect(),
            });
        }

        // The footprint pads of the board.
        let mut pads = Vec::new();
        for device in b.devices().values() {
            for pad in device.pads(p.library(), p.circuit())? {
                pads.push(PadInfo {
                    component: device.component(),
                    uuid: pad.uuid(),
                    net: pad.net(),
                    pos: pad.position(),
                    tht: pad.properties().is_tht(),
                    solder_layer: pad.solder_layer(),
                });
            }
        }

        // Delete all net segments, they will be re-created from scratch
        // below.
        let segment_ids: Vec<NetSegmentId> = b.net_segments().keys().copied().collect();
        for segment in segment_ids {
            tx.apply(Mutation::Board(BoardMutation::RemoveNetSegment(
                BoardNetSegmentRef { board, segment },
            )))?;
        }

        // Update devices placement.
        let mut imported_components = BTreeSet::new();
        let mut updated_components = BTreeSet::new();
        if let Some(components) = &session.components {
            for item in components {
                if item.name == "BOARD" {
                    continue; // Currently board-level pads cannot be edited.
                }
                imported_components.insert(item.name.clone());
                let p = tx.project();
                let cmp = p.circuit().component_instance_by_name(&item.name);
                let dev = cmp
                    .and_then(|(id, _)| p.board(board).and_then(|b| b.device(id)).map(|d| (id, d)));
                let Some((component, dev)) = dev else {
                    log.warning(tr!(
                        CTX,
                        "Component '{0}' from Specctra session does not exist in this board.",
                        item.name
                    ));
                    continue;
                };
                if (item.side == Side::Front) == dev.mirrored() {
                    log.warning(tr!(
                        CTX,
                        "Component '{0}' has been flipped, which is not supported yet.",
                        item.name
                    ));
                    continue;
                }
                let position = (!fuzzy_point(dev.position(), item.pos)).then_some(item.pos);
                let rotation = (!fuzzy_angle(dev.rotation(), item.rot)).then_some(item.rot);
                if position.is_some() || rotation.is_some() {
                    updated_components.insert(component);
                    tx.run(MoveDevice {
                        component: ComponentRef::Id(component),
                        board: Some(board),
                        position,
                        rotation,
                        mirrored: None,
                        locked: None,
                    })?;
                }
            }

            // Warn about missing components.
            let p = tx.project();
            if let Some(b) = p.board(board) {
                for dev in b.devices().values() {
                    let name = resolve::component_name(p, dev.component());
                    // Footprints without pads are discarded by Freerouting as
                    // they are not relevant, thus ignore them.
                    let has_pads = pads.iter().any(|pad| pad.component == dev.component());
                    if !imported_components.contains(&name) && has_pads {
                        log.warning(tr!(
                            CTX,
                            "The component '{0}' does not exist in the Specctra session.",
                            name
                        ));
                    }
                }
            }
        }
        // Positions of the footprint pads after moving devices.
        {
            let p = tx.project();
            if let Some(b) = p.board(board) {
                for pad in &mut pads {
                    if let Some(dev) = b.device(pad.component)
                        && let Some(view) = dev.pad(&pad.uuid, p.library(), p.circuit())?
                    {
                        pad.pos = view.position();
                    }
                }
            }
        }

        // Find the corresponding net signal for each imported net. In
        // addition, explicitly add any net that has board-level pads in it
        // if they haven't been imported anymore. The external router may
        // have removed such nets if they didn't contain traces, which would
        // cause board-level pads to be accidentally removed.
        let mut imported_nets = BTreeSet::new();
        let mut nets: Vec<(NetKey, Vec<ViaOut>, Vec<WireOut>)> = Vec::new();
        for net in &session.nets {
            let key = match tx.project().circuit().net_signal_by_name(&net.net_name) {
                Some((id, _)) => NetKey::Net(id),
                // ATTENTION: The ~anonymous~ comes from our own Specctra
                // export!
                None => match net.net_name.strip_prefix(ANONYMOUS_NET_PREFIX) {
                    Some(uuid) => NetKey::Anonymous(uuid.parse::<Uuid>().ok()),
                    None => {
                        log.warning(tr!(
                            CTX,
                            "The net '{0}' from Specctra session does not exist in this project, skipping it.",
                            net.net_name
                        ));
                        continue;
                    }
                },
            };
            nets.push((key, net.vias.clone(), net.wires.clone()));
            imported_nets.insert(key);
        }
        for seg in &old_segments {
            if !imported_nets.contains(&seg.key) && !seg.pads.is_empty() {
                nets.push((seg.key, Vec::new(), Vec::new()));
                imported_nets.insert(seg.key);
            }
        }

        // Import nets.
        let rules = tx
            .project()
            .board(board)
            .ok_or_else(|| Error::not_found("Board", board))?
            .design_rules()
            .clone();
        let mut matcher = Matcher {
            old: &old_segments,
            reused: BTreeSet::new(),
            new_uuids: 0,
        };
        let mut new_segments = Vec::new();
        for (key, vias, wires) in &nets {
            let key = *key;
            let mut anchors: Vec<AnchorData> = Vec::new();

            // Add anchors for each pad corresponding to imported wire
            // coordinates.
            let mut wire_coordinates: Vec<Point> = Vec::new();
            let mut wire_coordinates_per_layer: BTreeMap<Layer, Vec<Point>> = BTreeMap::new();
            for wire in wires {
                for vertex in wire.path.vertices() {
                    wire_coordinates.push(vertex.pos);
                    wire_coordinates_per_layer
                        .entry(wire.layer)
                        .or_default()
                        .push(vertex.pos);
                }
            }
            let mut add_pad_anchor =
                |anchors: &mut Vec<AnchorData>, anchor, pos: Point, tht: bool, solder_layer| {
                    let coordinates = if tht {
                        &wire_coordinates
                    } else {
                        wire_coordinates_per_layer.entry(solder_layer).or_default()
                    };
                    let mut pos = pos;
                    if !coordinates.contains(&pos) {
                        // Find another coordinate which is very close
                        // (rounding errors). In some tests, errors were up
                        // to 70 nm!
                        let nearest = coordinates
                            .iter()
                            .min_by_key(|c| *(**c - pos).length())
                            .copied();
                        match nearest {
                            Some(c) if fuzzy_point(pos, c) => pos = c,
                            _ => return,
                        }
                    }
                    let (start_layer, end_layer) = if tht {
                        (Layer::TOP_COPPER, Layer::BOT_COPPER)
                    } else {
                        (solder_layer, solder_layer)
                    };
                    anchors.push(AnchorData {
                        pos,
                        start_layer,
                        end_layer,
                        anchor,
                    });
                };
            if let Some(net) = key.net() {
                for pad in pads.iter().filter(|pad| pad.net == Some(net)) {
                    add_pad_anchor(
                        &mut anchors,
                        TraceAnchor::FootprintPad {
                            device: pad.component.0,
                            pad: pad.uuid,
                        },
                        pad.pos,
                        pad.tht,
                        pad.solder_layer,
                    );
                }
            }

            // Define net segments with the splitter.
            let mut splitter = BoardNetSegmentSplitter::new();
            for seg in old_segments.iter().filter(|s| s.key == key) {
                for pad in &seg.pads {
                    splitter.add_pad(pad.clone(), false);
                    let solder_layer = if pad.pad().component_side() == ComponentSide::Top {
                        Layer::TOP_COPPER
                    } else {
                        Layer::BOT_COPPER
                    };
                    add_pad_anchor(
                        &mut anchors,
                        TraceAnchor::Pad(pad.uuid()),
                        pad.pad().position(),
                        pad.pad().is_tht(),
                        solder_layer,
                    );
                }
            }
            for via in vias {
                let pad_stack = session.pad_stacks[&via.pad_stack_id];
                let old_via = matcher.via(key, via.pos, pad_stack.start_layer, pad_stack.end_layer);
                let uuid = old_via.as_ref().map_or_else(Uuid::new_random, Via::uuid);
                // Note: How can we know the drill diameter??? Use this logic
                // for now (like upstream):
                //  - If position & size not modified, keep original drill
                //    diameter too
                //  - Try to extract drill diameter from pad stack ID
                //  - If this didn't work, use automatic drill diameter as
                //    fallback
                let drill = match &old_via {
                    Some(v) => v.drill_diameter(),
                    None => extract_via_dimension(&via.pad_stack_id, 1)
                        .and_then(|(auto, dia)| (!auto).then_some(dia)),
                };
                // Automatic drill with manual size is not allowed!
                let mut size = match drill {
                    Some(_) => Some(
                        PositiveLength::new(pad_stack.diameter)
                            .map_err(|e| runtime_error(e.to_string()))?,
                    ),
                    None => None,
                };
                if let (Some(d), Some(s)) = (drill, size) {
                    let auto_size = Via::calc_size_from_rules(d, &rules.via_annular_ring());
                    let from_id = extract_via_dimension(&via.pad_stack_id, 2);
                    if from_id.is_some_and(|(auto, _)| auto) && (s == auto_size) {
                        size = None; // Automatic size is correct, let's take it.
                    }
                }
                // For the exposure config, use a similar mechanism like for
                // the drill.
                let exposure = match &old_via {
                    Some(v) => v.exposure_config(),
                    None => extract_via_exposure_config(&via.pad_stack_id)
                        .unwrap_or(MaskConfig::Automatic),
                };
                let new_via = Via::new(
                    uuid,
                    pad_stack.start_layer,
                    pad_stack.end_layer,
                    old_via.as_ref().map_or(via.pos, Via::position),
                    drill,
                    size,
                    exposure,
                )
                .map_err(|e| runtime_error(e.to_string()))?;
                splitter.add_via(new_via, false);
                anchors.push(AnchorData {
                    pos: via.pos,
                    start_layer: pad_stack.start_layer,
                    end_layer: pad_stack.end_layer,
                    anchor: TraceAnchor::Via(uuid),
                });
            }
            let get_or_create_anchor = |anchors: &mut Vec<AnchorData>,
                                        splitter: &mut BoardNetSegmentSplitter,
                                        matcher: &mut Matcher<'_>,
                                        pos: Point,
                                        layer: Layer| {
                let number = layer.copper_number();
                if let Some(a) = anchors.iter().find(|a| {
                    (a.pos == pos)
                        && (number >= a.start_layer.copper_number())
                        && (number <= a.end_layer.copper_number())
                }) {
                    return a.anchor;
                }
                // Create new junction.
                let old = matcher.net_point(key, pos, layer);
                let uuid = old.map_or_else(Uuid::new_random, |j| j.uuid);
                splitter.add_junction(Junction::new(uuid, old.map_or(pos, |j| j.pos)));
                let anchor = TraceAnchor::Junction(uuid);
                anchors.push(AnchorData {
                    pos,
                    start_layer: layer,
                    end_layer: layer,
                    anchor,
                });
                anchor
            };
            for wire in wires {
                let width =
                    PositiveLength::new(wire.width).map_err(|e| runtime_error(e.to_string()))?;
                for pair in wire.path.vertices().windows(2) {
                    let (mut p0, mut p1) = (pair[0].pos, pair[1].pos);
                    let old = matcher.net_line(key, p0, p1, wire.layer, wire.width);
                    if old.is_some_and(|t| !fuzzy_point(t.p1, p0)) {
                        std::mem::swap(&mut p0, &mut p1); // Avoid change in file format.
                    }
                    let a0 = get_or_create_anchor(
                        &mut anchors,
                        &mut splitter,
                        &mut matcher,
                        p0,
                        wire.layer,
                    );
                    let a1 = get_or_create_anchor(
                        &mut anchors,
                        &mut splitter,
                        &mut matcher,
                        p1,
                        wire.layer,
                    );
                    if a0 == a1 {
                        continue; // Zero-length segment.
                    }
                    let uuid = old.map_or_else(Uuid::new_random, |t| t.uuid);
                    splitter.add_trace(&Trace::new(uuid, wire.layer, width, a0, a1), || {
                        Uuid::new_random()
                    });
                }
            }

            // Add net segments with pads, vias, junctions and traces.
            for part in splitter.split() {
                let refs: BTreeSet<Uuid> = part
                    .junctions
                    .iter()
                    .map(Junction::uuid)
                    .chain(part.traces.iter().map(Trace::uuid))
                    .chain(part.vias.iter().map(Via::uuid))
                    .chain(part.pads.iter().map(BoardPadData::uuid))
                    .collect();
                let uuid = matcher
                    .net_segment(key, &refs)
                    .unwrap_or_else(Uuid::new_random);
                let segment = BoardNetSegment::with_elements(
                    uuid,
                    key.net(),
                    part.pads,
                    part.vias,
                    part.junctions,
                    part.traces,
                );
                new_segments.push(segment.id());
                tx.apply(Mutation::Board(BoardMutation::AddNetSegment {
                    board,
                    segment,
                }))?;
            }
        }

        // Print some statistics.
        let updated = updated_components.len();
        let unmodified = imported_components.len().saturating_sub(updated);
        log.info(tr!(
            CTX,
            "Updated {0} components ({1} unmodified components skipped).",
            updated,
            unmodified
        ));
        let (new_objects, reused_objects) = (matcher.new_uuids, matcher.reused.len());
        log.info(tr!(
            CTX,
            "Updated {0} net objects ({1} unmodified objects skipped).",
            new_objects,
            reused_objects
        ));
        new_segments.sort();
        Ok(SpecctraImportResult {
            board,
            updated_components: updated,
            unmodified_components: unmodified,
            new_objects,
            reused_objects,
            segments: new_segments,
            messages: log.messages,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn via_dimensions_from_padstack_id() {
        let mm = |v: &str| PositiveLength::new(v.parse::<Length>().unwrap()).unwrap();
        assert_eq!(
            extract_via_dimension("via-0.3:auto-0.7:auto-tht", 1),
            Some((true, mm("0.3")))
        );
        assert_eq!(
            extract_via_dimension("via-0.3:auto-0.7-tht-exposed", 2),
            Some((false, mm("0.7")))
        );
        assert_eq!(extract_via_dimension("foo-0.3-0.7-tht", 1), None);
        assert_eq!(
            extract_via_exposure_config("via-0.3-0.7-tht-exposed:0.2"),
            Some(MaskConfig::Manual("0.2".parse().unwrap()))
        );
        assert_eq!(
            extract_via_exposure_config("via-0.3-0.7-tht-exposed"),
            Some(MaskConfig::Automatic)
        );
        assert_eq!(
            extract_via_exposure_config("via-0.3-0.7-tht"),
            Some(MaskConfig::Off)
        );
    }

    #[test]
    fn mangled_pad_stack_names() {
        assert_eq!(
            mangled_pad_stack_name("via-0.3:auto-0.7:auto-tht"),
            "via-0:auto-0:auto-tht"
        );
        assert_eq!(
            mangled_pad_stack_name("via-0.4-0.8:auto-tht-exposed:0.2"),
            "via-0-0:auto-tht-exposed:0"
        );
        assert_eq!(mangled_pad_stack_name("via-1.0-2.0-tht"), "via-1-2-tht");
        assert_eq!(mangled_pad_stack_name("via-1-2-tht"), "via-1-2-tht");
    }

    #[test]
    fn fuzzy_compare() {
        let p = |x, y| Point::from_nm(x, y);
        assert!(fuzzy_point(p(0, 0), p(24, 0)));
        assert!(!fuzzy_point(p(0, 0), p(25, 0)));
        assert!(fuzzy_point(p(100_000_000, 0), p(100_000_999, 0)));
        assert!(!fuzzy_point(p(100_000_000, 0), p(100_001_000, 0)));
        assert!(fuzzy_angle(Angle::new(359_999_950), Angle::DEG0));
    }
}
