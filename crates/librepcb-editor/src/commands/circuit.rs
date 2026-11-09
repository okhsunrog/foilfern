//! Net and net class commands: port of
//! libs/librepcb/editor/project/cmd/cmdnetclassadd, cmdnetclassedit,
//! cmdnetclassremove, cmdnetsignaladd, cmdnetsignaledit,
//! cmdcombinenetsignals, cmdchangenetsignalofschematicnetsegment and
//! cmdremoveunusednetsignalsandbuses (`.{h,cpp}`).

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::geometry::TraceAnchor;
use librepcb_core::project::circuit::{NetClass, NetSignal};
use librepcb_core::project::schematic::SchematicNetSegment;
use librepcb_core::project::{
    BoardMutation, BoardNetSegmentRef, ComponentSignalRef, Mutation, NetClassId, NetSegmentRef,
    NetSignalId, NetUse, PlaneRef, SchematicMutation,
};
use librepcb_core::types::{CircuitIdentifier, ElementName, PositiveLength, Uuid};
use librepcb_i18n::tr;

use super::board::{BoardSelection, remove_board_items};
use super::{NetRef, resolve};
use crate::editor::{Command, Transaction};
use crate::error::{Error, Result};

/// Returns the net class named "default", adding it if it does not exist
/// (upstream `SchematicEditorState_DrawWire::startPositioning()`).
pub(crate) fn default_net_class(tx: &mut Transaction<'_>) -> Result<NetClassId> {
    if let Some((id, _)) = tx.project().circuit().net_class_by_name("default") {
        return Ok(id);
    }
    let net_class = NetClass::new(Uuid::new_random(), ElementName::new("default")?);
    let id = NetClassId(net_class.uuid());
    tx.apply(Mutation::AddNetClass(net_class))?;
    Ok(id)
}

/// Adds a net signal (upstream `CmdNetSignalAdd`): without name, an
/// automatic name `N<n>` is generated and marked as auto name.
pub(crate) fn add_net(
    tx: &mut Transaction<'_>,
    name: Option<CircuitIdentifier>,
    net_class: NetClassId,
) -> Result<NetSignalId> {
    let (name, auto) = match name {
        Some(name) => (name, false),
        None => (
            CircuitIdentifier::new(tx.project().circuit().generate_auto_net_signal_name())?,
            true,
        ),
    };
    let net = NetSignal::new(Uuid::new_random(), net_class, name, auto);
    let id = NetSignalId(net.uuid());
    tx.apply(Mutation::AddNetSignal(net))?;
    Ok(id)
}

/// Returns a copy of a schematic net segment assigned to another net.
pub(crate) fn with_net(segment: &SchematicNetSegment, net: NetSignalId) -> SchematicNetSegment {
    let mut copy = SchematicNetSegment::new(segment.uuid(), net);
    for junction in segment.junctions().values() {
        copy.insert_junction(junction.clone());
    }
    for line in segment.lines().values() {
        copy.insert_line(line.clone());
    }
    for label in segment.labels().values() {
        copy.insert_label(label.clone());
    }
    copy
}

/// Merges the net `to_remove` into `result` (upstream
/// `CmdCombineNetSignals`): all segments, planes and component signals of
/// `to_remove` are moved to `result`, then `to_remove` is removed.
pub(crate) fn combine_net_signals(
    tx: &mut Transaction<'_>,
    to_remove: NetSignalId,
    result: NetSignalId,
) -> Result<()> {
    if to_remove == result {
        return Ok(());
    }
    let p = tx.project();
    if p.circuit().net_signal(result).is_none() {
        return Err(Error::not_found("Net", result));
    }
    let uses: Vec<NetUse> = p.net_signal_uses(to_remove).copied().collect();
    let mut schematic_segments = Vec::new();
    let mut board_segments = Vec::new();
    let mut planes = Vec::new();
    let mut signals = Vec::new();
    for u in uses {
        match u {
            NetUse::SchematicSegment(schematic, segment) => {
                let value = p
                    .schematic(schematic)
                    .and_then(|s| s.net_segments().get(&segment))
                    .ok_or_else(|| Error::not_found("Net segment", segment))?;
                schematic_segments.push((schematic, value.clone()));
            }
            NetUse::BoardSegment(board, segment) => {
                let value = p
                    .board(board)
                    .and_then(|b| b.net_segment(segment))
                    .ok_or_else(|| Error::not_found("Net segment", segment))?;
                board_segments.push((board, value.clone()));
            }
            NetUse::Plane(board, plane) => {
                let value = p
                    .board(board)
                    .and_then(|b| b.plane(plane))
                    .ok_or_else(|| Error::not_found("Plane", plane))?;
                planes.push((board, value.clone()));
            }
            NetUse::ComponentSignal(signal) => signals.push(signal),
        }
    }
    // Remove all schematic net segments, board net segments and planes.
    for (schematic, segment) in &schematic_segments {
        tx.apply(Mutation::Schematic(SchematicMutation::RemoveNetSegment(
            NetSegmentRef {
                schematic: *schematic,
                segment: segment.id(),
            },
        )))?;
    }
    for (board, segment) in &board_segments {
        tx.apply(Mutation::Board(BoardMutation::RemoveNetSegment(
            BoardNetSegmentRef {
                board: *board,
                segment: segment.id(),
            },
        )))?;
    }
    for (board, plane) in &planes {
        tx.apply(Mutation::Board(BoardMutation::RemovePlane(PlaneRef {
            board: *board,
            plane: plane.id(),
        })))?;
    }
    // Change the net of all component signal instances.
    for signal in signals {
        tx.apply(Mutation::SetComponentSignalNet {
            signal,
            net: Some(result),
        })?;
    }
    // Re-add everything with the resulting net.
    for (board, mut segment) in board_segments {
        segment.set_net(Some(result));
        tx.apply(Mutation::Board(BoardMutation::AddNetSegment {
            board,
            segment,
        }))?;
    }
    for (board, mut plane) in planes {
        plane.set_net(Some(result));
        tx.apply(Mutation::Board(BoardMutation::AddPlane { board, plane }))?;
    }
    for (schematic, segment) in schematic_segments {
        tx.apply(Mutation::Schematic(SchematicMutation::AddNetSegment {
            schematic,
            segment: with_net(&segment, result),
        }))?;
    }
    tx.apply(Mutation::RemoveNetSignal(to_remove))
}

/// Connects a component signal to `net` after removing the traces attached
/// to its footprint pads in all boards (upstream
/// `CmdChangeNetSignalOfSchematicNetSegment::updateCompSigInstNetSignal()`
/// and `CmdRemoveSelectedSchematicItems::disconnectComponentSignalInstance()`).
pub(crate) fn set_component_signal_net(
    tx: &mut Transaction<'_>,
    signal: ComponentSignalRef,
    net: Option<NetSignalId>,
) -> Result<()> {
    let p = tx.project();
    let current = p
        .circuit()
        .component_instance(signal.component)
        .and_then(|c| c.signal(&signal.signal))
        .map(|s| s.net());
    if current == Some(net) {
        return Ok(());
    }
    let mut traces_per_board: BTreeMap<_, BoardSelection> = BTreeMap::new();
    for board in p.boards() {
        let Some(device) = board.device(signal.component) else {
            continue;
        };
        for pad in device.pads(p.library(), p.circuit())? {
            if pad.component_signal() != Some(signal) {
                continue;
            }
            let Some(segment_id) = board.footprint_pad_segment(signal.component, pad.uuid()) else {
                continue;
            };
            let Some(segment) = board.net_segment(segment_id) else {
                continue;
            };
            let anchor = TraceAnchor::FootprintPad {
                device: signal.component.0,
                pad: pad.uuid(),
            };
            let selection = traces_per_board.entry(board.id()).or_default();
            for trace in segment.traces_at(anchor) {
                selection.traces.push((segment_id, trace.uuid()));
            }
        }
    }
    for (board, selection) in traces_per_board {
        remove_board_items(tx, board, &selection, false)?;
    }
    tx.apply(Mutation::SetComponentSignalNet { signal, net })
}

/// Changes the net of a schematic net segment (upstream
/// `CmdChangeNetSignalOfSchematicNetSegment`): if it is the only schematic
/// segment of its net, the nets are combined; otherwise the segment and
/// the component signals of its pins move to the new net (removing traces
/// attached to their pads).
pub(crate) fn change_net_of_schematic_segment(
    tx: &mut Transaction<'_>,
    r: NetSegmentRef,
    net: NetSignalId,
) -> Result<()> {
    let p = tx.project();
    let schematic = p
        .schematic(r.schematic)
        .ok_or_else(|| Error::not_found("Schematic", r.schematic))?;
    let segment = schematic
        .net_segments()
        .get(&r.segment)
        .ok_or_else(|| Error::not_found("Net segment", r.segment))?
        .clone();
    let old = segment.net();
    if old == net {
        return Ok(());
    }
    if resolve::schematic_segments_of_net(p, old).len() == 1 {
        return combine_net_signals(tx, old, net);
    }
    // Memorize the component signals of the connected pins.
    let mut signals = BTreeSet::new();
    for (symbol, pin) in segment.connected_pins() {
        let Some(symbol) = schematic.symbols().get(&symbol) else {
            continue;
        };
        if let Some(view) = symbol.pin(p.view(), pin)? {
            signals.insert(view.signal());
        }
    }
    tx.apply(Mutation::Schematic(SchematicMutation::RemoveNetSegment(r)))?;
    for signal in signals {
        set_component_signal_net(tx, signal, Some(net))?;
    }
    tx.apply(Mutation::Schematic(SchematicMutation::AddNetSegment {
        schematic: r.schematic,
        segment: with_net(&segment, net),
    }))
}

/// Removes nets without component signals and schematic segments (with
/// their board segments and planes) and unused buses (upstream
/// `CmdRemoveUnusedNetSignalsAndBuses`). Returns the removed nets.
pub(crate) fn remove_unused_nets(tx: &mut Transaction<'_>) -> Result<Vec<NetSignalId>> {
    let p = tx.project();
    let mut removed = Vec::new();
    let mut steps = Vec::new();
    for id in p.circuit().net_signals().keys() {
        let uses: Vec<NetUse> = p.net_signal_uses(*id).copied().collect();
        let used = uses
            .iter()
            .any(|u| matches!(u, NetUse::ComponentSignal(_) | NetUse::SchematicSegment(..)));
        if used {
            continue;
        }
        for u in uses {
            match u {
                NetUse::BoardSegment(board, segment) => steps.push(Mutation::Board(
                    BoardMutation::RemoveNetSegment(BoardNetSegmentRef { board, segment }),
                )),
                NetUse::Plane(board, plane) => {
                    steps.push(Mutation::Board(BoardMutation::RemovePlane(PlaneRef {
                        board,
                        plane,
                    })))
                }
                _ => {}
            }
        }
        steps.push(Mutation::RemoveNetSignal(*id));
        removed.push(*id);
    }
    for id in p.circuit().buses().keys() {
        if p.bus_uses(*id).next().is_none() {
            steps.push(Mutation::RemoveBus(*id));
        }
    }
    tx.apply_all(steps)?;
    Ok(removed)
}

/// Adds a net class (upstream `CmdNetClassAdd`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddNetClass {
    /// The name.
    pub name: ElementName,
    /// Default trace width (default: inherit from the board).
    #[serde(default)]
    pub default_trace_width: Option<PositiveLength>,
    /// Default via drill diameter (default: inherit from the board).
    #[serde(default)]
    pub default_via_drill: Option<PositiveLength>,
}

impl Command for AddNetClass {
    type Output = NetClassId;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdNetClassAdd", "Add netclass")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<NetClassId> {
        let mut net_class = NetClass::new(Uuid::new_random(), self.name);
        net_class.set_default_trace_width(self.default_trace_width);
        net_class.set_default_via_drill(self.default_via_drill);
        let id = NetClassId(net_class.uuid());
        tx.apply(Mutation::AddNetClass(net_class))?;
        Ok(id)
    }
}

/// Modifies a net class (upstream `CmdNetClassEdit`); `None` fields are
/// kept.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EditNetClass {
    /// The net class.
    pub net_class: NetClassId,
    /// The new name.
    #[serde(default)]
    pub name: Option<ElementName>,
    /// The new default trace width.
    #[serde(default)]
    pub default_trace_width: Option<PositiveLength>,
    /// The new default via drill diameter.
    #[serde(default)]
    pub default_via_drill: Option<PositiveLength>,
    /// Whether the default trace width and via drill are reset to "inherit
    /// from the board" (applied before the values above).
    #[serde(default)]
    pub inherit_defaults: bool,
}

impl Command for EditNetClass {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdNetClassEdit", "Edit Net Class")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        let mut net_class = tx
            .project()
            .circuit()
            .net_class(self.net_class)
            .ok_or_else(|| Error::not_found("Net class", self.net_class))?
            .clone();
        if let Some(name) = self.name {
            net_class.set_name(name);
        }
        if self.inherit_defaults {
            net_class.set_default_trace_width(None);
            net_class.set_default_via_drill(None);
        }
        if let Some(width) = self.default_trace_width {
            net_class.set_default_trace_width(Some(width));
        }
        if let Some(drill) = self.default_via_drill {
            net_class.set_default_via_drill(Some(drill));
        }
        tx.apply(Mutation::UpdateNetClass(net_class))
    }
}

/// Removes an unused net class (upstream `CmdNetClassRemove`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoveNetClass {
    /// The net class.
    pub net_class: NetClassId,
}

impl Command for RemoveNetClass {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdNetClassRemove", "Remove netclass")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<()> {
        tx.apply(Mutation::RemoveNetClass(self.net_class))
    }
}

/// Adds a net signal without connections (upstream `CmdNetSignalAdd`).
/// Unused nets are removed by most removal commands, like upstream.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddNet {
    /// The name (default: automatic name `N<n>`).
    #[serde(default)]
    pub name: Option<CircuitIdentifier>,
    /// The net class (default: "default").
    #[serde(default)]
    pub net_class: Option<NetClassId>,
}

impl Command for AddNet {
    type Output = NetSignalId;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdNetSignalAdd", "Add netsignal")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<NetSignalId> {
        let net_class = match self.net_class {
            Some(id) => id,
            None => default_net_class(tx)?,
        };
        add_net(tx, self.name, net_class)
    }
}

/// Renames a net and/or changes its net class (upstream
/// `CmdNetSignalEdit`; a new name is no longer an auto name). If another
/// net already has the name and `merge` is set, the net is merged into
/// that one (upstream `CmdCombineNetSignals`, as the net label rename
/// dialog does), otherwise the name conflict is an error.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EditNet {
    /// The net.
    pub net: NetRef,
    /// The new name.
    #[serde(default)]
    pub name: Option<CircuitIdentifier>,
    /// The new net class.
    #[serde(default)]
    pub net_class: Option<NetClassId>,
    /// Whether to merge into an existing net with the new name.
    #[serde(default)]
    pub merge: bool,
}

impl Command for EditNet {
    /// The resulting net (the other one if merged).
    type Output = NetSignalId;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdNetSignalEdit", "Edit netsignal")
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<NetSignalId> {
        let id = resolve::required_net(tx.project(), &self.net)?;
        let mut net = tx
            .project()
            .circuit()
            .net_signal(id)
            .ok_or_else(|| Error::not_found("Net", id))?
            .clone();
        if let Some(class) = self.net_class {
            net.set_net_class(class);
        }
        if let Some(name) = self.name {
            let existing = tx
                .project()
                .circuit()
                .net_signal_by_name(name.as_str())
                .map(|(other, _)| other)
                .filter(|other| *other != id);
            match existing {
                Some(other) if self.merge => {
                    if net != *tx.project().circuit().net_signal(id).expect("exists") {
                        tx.apply(Mutation::UpdateNetSignal(net))?;
                    }
                    combine_net_signals(tx, id, other)?;
                    return Ok(other);
                }
                _ => {
                    net.set_name(name, false);
                }
            }
        }
        tx.apply(Mutation::UpdateNetSignal(net))?;
        Ok(id)
    }
}

/// Merges one net into another (upstream `CmdCombineNetSignals`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeNets {
    /// The net which is merged and removed.
    pub net: NetRef,
    /// The resulting net.
    pub into: NetRef,
}

impl Command for MergeNets {
    type Output = NetSignalId;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdCombineNetSignals",
            "Combine Net Signals"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<NetSignalId> {
        let from = resolve::required_net(tx.project(), &self.net)?;
        let into = resolve::required_net(tx.project(), &self.into)?;
        combine_net_signals(tx, from, into)?;
        Ok(into)
    }
}

/// Removes unused nets and buses (upstream
/// `CmdRemoveUnusedNetSignalsAndBuses`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoveUnusedNets {}

impl Command for RemoveUnusedNets {
    /// The removed nets.
    type Output = Vec<NetSignalId>;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdRemoveUnusedNetSignalsAndBuses",
            "Remove Unused Nets & Buses"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<Self::Output> {
        remove_unused_nets(tx)
    }
}

/// Moves a schematic net segment to another net, adding the net if no net
/// with that name exists (upstream `RenameNetSegmentDialog` with "rename
/// only this net segment": `CmdNetSignalAdd` with the net class of the
/// current net, then `CmdChangeNetSignalOfSchematicNetSegment`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeNetOfSchematicSegment {
    /// The net segment.
    pub segment: NetSegmentRef,
    /// The name of the (new or existing) net.
    pub net: CircuitIdentifier,
}

impl Command for ChangeNetOfSchematicSegment {
    /// The net of the segment.
    type Output = NetSignalId;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::RenameNetSegmentDialog",
            "Change net of net segment"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<NetSignalId> {
        let old = tx
            .project()
            .schematic(self.segment.schematic)
            .and_then(|s| s.net_segments().get(&self.segment.segment))
            .ok_or_else(|| Error::not_found("Net segment", self.segment.segment))?
            .net();
        let existing = tx
            .project()
            .circuit()
            .net_signal_by_name(self.net.as_str())
            .map(|(id, _)| id);
        let net = match existing {
            Some(id) => id,
            None => {
                let class = tx
                    .project()
                    .circuit()
                    .net_signal(old)
                    .ok_or_else(|| Error::not_found("Net", old))?
                    .net_class();
                add_net(tx, Some(self.net), class)?
            }
        };
        change_net_of_schematic_segment(tx, self.segment, net)?;
        Ok(net)
    }
}
