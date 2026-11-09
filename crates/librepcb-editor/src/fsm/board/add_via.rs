//! The via tool: port of
//! libs/librepcb/editor/project/board/fsm/boardeditorstate_addvia.{h,cpp}.
//!
//! The via follows the cursor in its own net segment (a live preview in
//! the undo group); a click connects it to the traces and junctions of its
//! net at the position ([`crate::commands::AddVia`] semantics), commits,
//! and starts the next via.

use librepcb_core::geometry::Via;
use librepcb_core::project::board::BoardNetSegment;
use librepcb_core::project::{BoardId, BoardMutation, Mutation, NetSegmentId, NetSignalId};
use librepcb_core::types::{Layer, MaskConfig, Point, PositiveLength, Uuid};
use librepcb_i18n::tr;

use super::context::Cx;
use super::output::{BoardToolData, ToolNet, ToolSetting};
use super::view::{BoardItemRef, FindFilter, FindFlags};
use super::{BoardFsmInput, State};
use crate::commands::board::{ViaResult, add_via_connected};
use crate::commands::{EditBoardSettings, EditNetClass};
use crate::editor::{Command, Transaction};
use crate::error::Result;
use crate::fsm::CursorShape;

/// Drill and size of new vias (upstream `mCurrentViaProperties`), shared
/// by the via and trace tools.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ViaSettings {
    /// Drill (`None`: automatic).
    pub drill: Option<PositiveLength>,
    /// Size (`None`: automatic).
    pub size: Option<PositiveLength>,
}

impl ViaSettings {
    /// The effective drill (upstream `getDrillDiameter()`).
    pub fn drill(&self, cx: &Cx<'_, '_>, net: Option<NetSignalId>) -> Option<PositiveLength> {
        if let Some(d) = self.drill {
            return Some(d);
        }
        let p = cx.project();
        if let Some(d) = net
            .and_then(|n| p.circuit().net_signal(n))
            .and_then(|n| p.circuit().net_class(n.net_class()))
            .and_then(|c| c.default_via_drill())
        {
            return Some(d);
        }
        cx.board()
            .ok()
            .map(|b| b.design_rules().default_via_drill_diameter())
    }

    /// The effective size (upstream `getSize()`).
    pub fn size(&self, cx: &Cx<'_, '_>, net: Option<NetSignalId>) -> Option<PositiveLength> {
        if let Some(s) = self.size {
            return Some(s);
        }
        let drill = self.drill(cx, net)?;
        let rules = cx.board().ok()?.design_rules().clone();
        Some(Via::calc_size_from_rules(drill, &rules.via_annular_ring()))
    }

    /// Upstream `setDrillDiameter()`.
    pub fn set_drill(
        &mut self,
        cx: &Cx<'_, '_>,
        net: Option<NetSignalId>,
        drill: Option<PositiveLength>,
    ) {
        // Avoid a via with auto drill but manual size.
        if drill.is_none() && self.size.is_some() {
            self.size = None;
        }
        // Avoid vias with a drill larger than the size.
        if let (Some(d), Some(s)) = (drill, self.size)
            && d > s
        {
            self.set_size(cx, net, Some(d));
        }
        self.drill = drill;
    }

    /// Upstream `setSize()`.
    pub fn set_size(
        &mut self,
        cx: &Cx<'_, '_>,
        net: Option<NetSignalId>,
        size: Option<PositiveLength>,
    ) {
        // Avoid a via with auto drill but manual size.
        if size.is_some() && self.drill.is_none() {
            self.drill = self.drill(cx, net);
        }
        // Avoid vias with a drill larger than the size.
        if let (Some(s), Some(d)) = (size, self.drill)
            && s < d
        {
            self.drill = Some(s);
        }
        self.size = size;
    }

    /// A via with these settings.
    pub fn via(&self, uuid: Uuid, pos: Point) -> Result<Via> {
        Via::new(
            uuid,
            Layer::TOP_COPPER,
            Layer::BOT_COPPER,
            pos,
            self.drill,
            self.size,
            MaskConfig::Off,
        )
        .map_err(|e| crate::error::Error::InvalidArgument(e.to_string()))
    }

    /// Stores the drill as board default (upstream
    /// `saveDrillDiameterInBoard()`).
    pub fn save_drill_in_board(&self, cx: &mut Cx<'_, '_>, net: Option<NetSignalId>) {
        let Some(drill) = self.drill(cx, net) else {
            return;
        };
        let Ok(board) = cx.board() else {
            return;
        };
        let mut rules = board.design_rules().clone();
        rules.set_default_via_drill_diameter(drill);
        let id = cx.board_id();
        if let Err(e) = cx.exec(EditBoardSettings {
            board: Some(id),
            design_rules: Some(rules),
            ..Default::default()
        }) {
            cx.error(e);
        }
    }

    /// Stores the drill as net class default (upstream
    /// `saveDrillDiameterInNetClass()`).
    pub fn save_drill_in_net_class(&self, cx: &mut Cx<'_, '_>, net: Option<NetSignalId>) {
        let Some(class) = net
            .and_then(|n| cx.project().circuit().net_signal(n))
            .map(|n| n.net_class())
        else {
            return;
        };
        if let Err(e) = cx.exec(EditNetClass {
            net_class: class,
            name: None,
            default_trace_width: None,
            default_via_drill: self.drill,
            inherit_defaults: false,
        }) {
            cx.error(e);
        }
    }
}

/// Places a via connected to the traces and junctions of its net at its
/// position (upstream `fixPosition()`).
struct PlaceVia {
    board: BoardId,
    via: Via,
    net: Option<NetSignalId>,
}

impl Command for PlaceVia {
    type Output = ViaResult;

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::BoardEditorState_AddVia",
            "Add via to board"
        )
    }

    fn execute(self, tx: &mut Transaction<'_>) -> Result<ViaResult> {
        add_via_connected(tx, self.board, self.via, self.net)
    }
}

/// The via being placed.
#[derive(Debug, Clone, Copy)]
struct Placing {
    segment: NetSegmentId,
    via: Uuid,
    pos: Point,
}

/// The via tool (upstream `BoardEditorState_AddVia`).
#[derive(Debug)]
pub(super) struct AddViaState {
    settings: ViaSettings,
    auto_net: bool,
    net: Option<NetSignalId>,
    placing: Option<Placing>,
}

impl Default for AddViaState {
    fn default() -> Self {
        Self {
            settings: ViaSettings::default(),
            auto_net: true,
            net: None,
            placing: None,
        }
    }
}

impl AddViaState {
    /// Upstream `addVia()`.
    fn add_via(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        if let Err(e) = cx.begin(tr!(
            "librepcb::editor::BoardEditorState_AddVia",
            "Add via to board"
        )) {
            cx.error(e);
            return false;
        }
        self.placing = Some(Placing {
            segment: NetSegmentId(Uuid::new_random()),
            via: Uuid::new_random(),
            pos,
        });
        if let Err(e) = self.update_preview(cx) {
            cx.error(e);
            self.abort(cx);
            return false;
        }
        cx.highlight_nets(self.net);
        true
    }

    fn update_preview(&mut self, cx: &mut Cx<'_, '_>) -> Result<()> {
        let Some(placing) = self.placing else {
            return Ok(());
        };
        let via = self.settings.via(placing.via, placing.pos)?;
        let segment = BoardNetSegment::with_elements(
            placing.segment.0,
            self.net,
            vec![],
            vec![via],
            vec![],
            vec![],
        );
        let board = cx.board_id();
        cx.preview(
            0,
            vec![Mutation::Board(BoardMutation::AddNetSegment {
                board,
                segment,
            })],
        )
    }

    /// Upstream `updateClosestNetSignal()`.
    fn update_closest_net(&mut self, cx: &mut Cx<'_, '_>, pos: Point) {
        let except = self
            .placing
            .map(|p| vec![BoardItemRef::Via(p.segment, p.via)])
            .unwrap_or_default();
        let item = cx.find_item(
            pos,
            FindFlags::VIAS
                | FindFlags::FOOTPRINT_PADS
                | FindFlags::TRACES
                | FindFlags::ACCEPT_NEXT_GRID_MATCH,
            &FindFilter {
                except: &except,
                ..Default::default()
            },
        );
        let board = cx.board().ok();
        let p = cx.project();
        let net = match item {
            Some(BoardItemRef::Trace(s, _) | BoardItemRef::Via(s, _)) => {
                board.and_then(|b| b.net_segment(s)).and_then(|s| s.net())
            }
            Some(BoardItemRef::FootprintPad(c, u)) => board
                .and_then(|b| b.device(c))
                .and_then(|d| d.pad(&u, p.library(), p.circuit()).ok().flatten())
                .and_then(|pad| pad.net()),
            _ if self.net.is_none() => p.net_signal_with_most_elements(),
            _ => self.net,
        };
        self.net = net;
    }

    /// Upstream `fixPosition()`.
    fn fix_position(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        let Some(placing) = self.placing.take() else {
            return false;
        };
        let result = (|| -> Result<()> {
            let via = self.settings.via(placing.via, pos)?;
            cx.rollback_to(0);
            let board = cx.board_id();
            cx.exec(PlaceVia {
                board,
                via,
                net: self.net,
            })?;
            cx.commit()?;
            Ok(())
        })();
        if let Err(e) = result {
            cx.error(e);
            self.abort(cx);
            return false;
        }
        true
    }

    fn abort(&mut self, cx: &mut Cx<'_, '_>) {
        cx.highlight_nets([]);
        self.placing = None;
        if cx.is_group_active() {
            cx.abort();
        }
    }

    fn net_for_settings(&self) -> Option<NetSignalId> {
        self.net
    }
}

impl State for AddViaState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let pos = cx.cursor_pos.mapped_to_grid(cx.grid());
        if !self.add_via(cx, pos) {
            return false;
        }
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.abort(cx);
        cx.set_cursor(None);
        true
    }

    fn process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        match input {
            BoardFsmInput::PointerMoved(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                let Some(placing) = self.placing.as_mut() else {
                    return false;
                };
                placing.pos = pos;
                if self.auto_net {
                    self.update_closest_net(cx, pos);
                    cx.highlight_nets(self.net);
                }
                if let Err(e) = self.update_preview(cx) {
                    cx.error(e);
                }
                true
            }
            BoardFsmInput::LeftPressed(e) | BoardFsmInput::LeftDoubleClicked(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                self.fix_position(cx, pos);
                self.add_via(cx, pos);
                true
            }
            BoardFsmInput::ToolSetting(setting) => {
                let net = self.net_for_settings();
                match setting {
                    ToolSetting::ViaDrill(d) => {
                        let mut s = self.settings;
                        s.set_drill(cx, net, *d);
                        self.settings = s;
                    }
                    ToolSetting::ViaSize(size) => {
                        let mut s = self.settings;
                        s.set_size(cx, net, *size);
                        self.settings = s;
                    }
                    ToolSetting::Net(ToolNet { auto, net }) => {
                        self.auto_net = *auto;
                        if !*auto {
                            self.net = *net;
                        }
                        cx.highlight_nets(self.net);
                    }
                    ToolSetting::SaveViaDrillInBoard => {
                        self.settings.save_drill_in_board(cx, net);
                        return true;
                    }
                    ToolSetting::SaveViaDrillInNetClass => {
                        self.settings.save_drill_in_net_class(cx, net);
                        return true;
                    }
                    _ => return false,
                }
                if let Err(e) = self.update_preview(cx) {
                    cx.error(e);
                }
                true
            }
            _ => false,
        }
    }

    fn tool_data(&self, cx: &Cx<'_, '_>) -> BoardToolData {
        let p = cx.project();
        BoardToolData {
            nets: sorted_nets(cx),
            net: ToolNet {
                auto: self.auto_net,
                net: self.net,
            },
            net_class_name: self
                .net
                .and_then(|n| p.circuit().net_signal(n))
                .and_then(|n| p.circuit().net_class(n.net_class()))
                .map(|c| c.name().to_string())
                .unwrap_or_default(),
            drill: self.settings.drill(cx, self.net),
            auto_drill: self.settings.drill.is_none(),
            size: self.settings.size(cx, self.net),
            auto_size: self.settings.size.is_none(),
            ..Default::default()
        }
    }
}

/// All nets sorted naturally by name (upstream `getAvailableNets()`).
pub(super) fn sorted_nets(cx: &Cx<'_, '_>) -> Vec<(NetSignalId, String)> {
    let mut nets: Vec<(NetSignalId, String)> = cx
        .project()
        .circuit()
        .net_signals()
        .iter()
        .map(|(id, n)| (*id, n.name().to_string()))
        .collect();
    nets.sort_by(|a, b| librepcb_core::utils::toolbox::compare_numeric(&a.1, &b.1));
    nets
}
