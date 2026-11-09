//! The standalone pad tools: port of
//! libs/librepcb/editor/project/board/fsm/boardeditorstate_addpad.{h,cpp}
//! (and of the pad parts of `Board2dTab::fsmToolEnter()`).
//!
//! The pad follows the cursor (live preview in the group "Add Pad to
//! Board": a new net segment containing the pad); a click commits it and
//! starts the next one. Tool bar changes (net, shape, size, drill, ...)
//! update the pad being placed and are kept for the next pads, like
//! upstream.
//!
//! Differences to upstream: changing the net re-creates the preview
//! segment with the new net (upstream removes, edits and re-adds it).

use librepcb_core::geometry::{
    ComponentSide, NonEmptyPath, Pad, PadFunction, PadHole, PadHoleList, PadShape, Path,
};
use librepcb_core::project::board::{BoardNetSegment, BoardPadData};
use librepcb_core::project::{BoardMutation, Mutation, NetSignalId};
use librepcb_core::types::{
    Angle, Length, MaskConfig, Point, PositiveLength, Ratio, UnsignedLength, UnsignedLimitedRatio,
    Uuid,
};
use librepcb_i18n::tr;

use super::context::Cx;
use super::output::{BoardToolData, ToolNet, ToolPadShape, ToolSetting};
use super::{BoardFsmInput, State};
use crate::fsm::CursorShape;

fn positive(nm: i64) -> PositiveLength {
    // Invariant: only called with positive constants.
    PositiveLength::new(Length::new(nm)).expect("positive constant")
}

fn ratio(percent: i32) -> UnsignedLimitedRatio {
    // Invariant: only called with 0..=100 %.
    UnsignedLimitedRatio::new(Ratio::from_percent(percent)).expect("0..100 %")
}

/// The pad tool (upstream `BoardEditorState_AddPad`), one instance per pad
/// kind (THT, SMT per function).
#[derive(Debug)]
pub(super) struct AddPadState {
    /// Current properties (position and UUIDs are set when placing).
    props: Pad,
    net: Option<NetSignalId>,
    /// The pad being placed: segment UUID and pad UUID.
    placing: Option<(Uuid, Uuid)>,
}

impl AddPadState {
    /// A THT pad tool (upstream `PadType::THT`, standard pad).
    pub fn tht() -> Self {
        let mut s = Self::new(PadFunction::StandardPad);
        let mut holes = PadHoleList::new();
        holes.push(PadHole::new(
            Uuid::new_random(),
            positive(800_000),
            NonEmptyPath::from_point(Point::ORIGIN),
        ));
        s.props.set_holes(holes);
        s.apply_recommended_radius();
        s
    }

    /// An SMT pad tool with the defaults of the given function (upstream
    /// `PadType::SMT`).
    pub fn smt(function: PadFunction) -> Self {
        let mut s = Self::new(function);
        let p = &mut s.props;
        p.set_radius(ratio(50));
        p.set_width(positive(1_500_000));
        p.set_height(positive(700_000));
        p.set_solder_paste_config(MaskConfig::Automatic);
        match function {
            PadFunction::ThermalPad => {
                p.set_radius(ratio(0));
                p.set_width(positive(2_000_000));
                p.set_height(positive(2_000_000));
            }
            PadFunction::BgaPad => {
                p.set_radius(ratio(100));
                p.set_width(positive(300_000));
                p.set_height(positive(300_000));
            }
            PadFunction::EdgeConnectorPad => {
                p.set_radius(ratio(0));
                p.set_solder_paste_config(MaskConfig::Off);
            }
            PadFunction::TestPad => {
                p.set_radius(ratio(100));
                p.set_width(positive(700_000));
                p.set_height(positive(700_000));
                p.set_solder_paste_config(MaskConfig::Off);
            }
            PadFunction::LocalFiducial | PadFunction::GlobalFiducial => {
                p.set_radius(ratio(100));
                p.set_width(positive(1_000_000));
                p.set_height(positive(1_000_000));
                let clearance = UnsignedLength::new(Length::new(500_000)).unwrap_or_default();
                p.set_copper_clearance(clearance);
                p.set_stop_mask_config(MaskConfig::Manual(*clearance));
                p.set_solder_paste_config(MaskConfig::Off);
            }
            _ => {}
        }
        s.apply_recommended_radius();
        s
    }

    fn new(function: PadFunction) -> Self {
        Self {
            props: Pad::new(
                Uuid::new_random(),
                Point::ORIGIN,
                Angle::DEG0,
                PadShape::RoundedRect,
                positive(2_500_000),
                positive(1_300_000),
                ratio(100),
                Path::default(),
                MaskConfig::Automatic,
                MaskConfig::Off,
                UnsignedLength::default(),
                ComponentSide::Top,
                function,
                PadHoleList::new(),
            ),
            net: None,
            placing: None,
        }
    }

    fn is_tht(&self) -> bool {
        self.props.is_tht()
    }

    /// Upstream `applyRecommendedRoundedRectRadius()`.
    fn apply_recommended_radius(&mut self) {
        let r = self.props.radius().get();
        if r > Ratio::from_percent(0) && r < Ratio::from_percent(100) {
            self.props.set_radius(Pad::recommended_radius(
                self.props.width(),
                self.props.height(),
            ));
        }
    }

    fn drill(&self) -> Option<PositiveLength> {
        self.props.holes().first().map(|h| h.diameter())
    }

    /// Upstream `setWidth()`.
    fn set_width(&mut self, width: PositiveLength) {
        if self.props.width() != width {
            self.props.set_width(width);
            self.apply_recommended_radius();
        }
        // Avoid creating pads with a drill larger than width or height.
        if self.drill().is_some_and(|d| d > width) {
            self.set_drill(width);
        }
    }

    /// Upstream `setHeight()`.
    fn set_height(&mut self, height: PositiveLength) {
        if self.props.height() != height {
            self.props.set_height(height);
            self.apply_recommended_radius();
        }
        if self.drill().is_some_and(|d| d > height) {
            self.set_drill(height);
        }
    }

    /// Upstream `setDrillDiameter()`.
    fn set_drill(&mut self, diameter: PositiveLength) {
        let mut holes = self.props.holes().clone();
        let Some(hole) = holes.get_mut(0) else {
            return;
        };
        hole.set_diameter(diameter);
        self.props.set_holes(holes);
        if diameter > self.props.width() {
            self.set_width(diameter);
        }
        if diameter > self.props.height() {
            self.set_height(diameter);
        }
    }

    /// Upstream `Board2dTab::shapeRequested`.
    fn set_shape(&mut self, shape: ToolPadShape) {
        match shape {
            ToolPadShape::Round => {
                self.props.set_shape(PadShape::RoundedRect);
                self.props.set_radius(ratio(100));
            }
            ToolPadShape::RoundedRect => {
                self.props.set_shape(PadShape::RoundedRect);
                self.props.set_radius(Pad::recommended_radius(
                    self.props.width(),
                    self.props.height(),
                ));
            }
            ToolPadShape::Rect => {
                self.props.set_shape(PadShape::RoundedRect);
                self.props.set_radius(ratio(0));
            }
            ToolPadShape::Octagon => {
                self.props.set_shape(PadShape::RoundedOctagon);
                self.props.set_radius(ratio(0));
            }
        }
    }

    /// The shape as shown in the tool bar (upstream `getCurrentShape()`).
    fn tool_shape(&self) -> ToolPadShape {
        let r = self.props.radius().get();
        if self.props.shape() != PadShape::RoundedRect {
            ToolPadShape::Octagon
        } else if r == Ratio::from_percent(0) {
            ToolPadShape::Rect
        } else if r == Ratio::from_percent(100) {
            ToolPadShape::Round
        } else {
            ToolPadShape::RoundedRect
        }
    }

    /// Replaces the preview by a segment with the pad at its current
    /// position.
    fn preview(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        let Some((segment, pad)) = self.placing else {
            return false;
        };
        let board = cx.board_id();
        let data = BoardPadData::new(self.props.with_uuid(pad), false);
        let segment = BoardNetSegment::with_elements(segment, self.net, [data], [], [], []);
        match cx.preview(
            0,
            vec![Mutation::Board(BoardMutation::AddNetSegment {
                board,
                segment,
            })],
        ) {
            Ok(()) => true,
            Err(e) => {
                cx.error(e);
                self.abort(cx);
                false
            }
        }
    }

    /// Upstream `start()`.
    fn start(&mut self, cx: &mut Cx<'_, '_>, pos: Point) -> bool {
        // Assign new UUIDs to all holes.
        let mut holes = self.props.holes().clone();
        for hole in holes.iter_mut() {
            *hole = PadHole::new(Uuid::new_random(), hole.diameter(), hole.path().clone());
        }
        self.props.set_holes(holes);
        if let Err(e) = cx.begin(tr!(
            "librepcb::editor::BoardEditorState_AddPad",
            "Add Pad to Board"
        )) {
            cx.error(e);
            return false;
        }
        self.props.set_position(pos);
        self.placing = Some((Uuid::new_random(), Uuid::new_random()));
        cx.highlight_nets(self.net);
        self.preview(cx)
    }

    /// Upstream `finish()`.
    fn finish(&mut self, cx: &mut Cx<'_, '_>, pos: Point) {
        self.props.set_position(pos);
        if self.preview(cx) {
            self.placing = None;
            if let Err(e) = cx.commit() {
                cx.error(e);
            }
        }
    }

    /// Upstream `abortCommand()`.
    fn abort(&mut self, cx: &mut Cx<'_, '_>) {
        cx.highlight_nets([]);
        self.placing = None;
        if cx.is_group_active() {
            cx.abort();
        }
    }

    /// Applies a tool bar change; returns whether it was handled.
    fn setting(&mut self, cx: &mut Cx<'_, '_>, setting: &ToolSetting) -> bool {
        match setting {
            ToolSetting::Net(ToolNet { net, .. }) => {
                self.net = *net;
                cx.highlight_nets(self.net);
            }
            ToolSetting::ComponentSide(side) if !self.is_tht() => {
                self.props.set_component_side(*side);
            }
            ToolSetting::PadShape(shape) => self.set_shape(*shape),
            ToolSetting::PadWidth(width) => {
                self.set_width(*width);
                if self.props.function_is_fiducial() {
                    self.set_height(*width);
                }
            }
            ToolSetting::PadHeight(height) => self.set_height(*height),
            ToolSetting::PadRadius(radius) => {
                self.props.set_radius(*radius);
            }
            ToolSetting::HoleDiameter(d) if self.is_tht() => self.set_drill(*d),
            ToolSetting::PressFit(press_fit) if self.is_tht() => {
                self.props.set_function(if *press_fit {
                    PadFunction::PressFitPad
                } else {
                    PadFunction::StandardPad
                });
            }
            ToolSetting::FiducialClearance(clearance) if self.props.function_is_fiducial() => {
                self.props.set_copper_clearance(*clearance);
                self.props
                    .set_stop_mask_config(MaskConfig::Manual(**clearance));
            }
            _ => return false,
        }
        self.preview(cx);
        true
    }
}

impl State for AddPadState {
    fn entry(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        // Drop nets which no longer exist.
        if self
            .net
            .is_some_and(|n| cx.project().circuit().net_signal(n).is_none())
        {
            self.net = None;
        }
        let pos = cx.cursor_pos.mapped_to_grid(cx.grid());
        if !self.start(cx, pos) {
            return false;
        }
        cx.out.view.features = crate::fsm::Features {
            rotate: true,
            ..Default::default()
        };
        cx.set_cursor(Some(CursorShape::Cross));
        true
    }

    fn exit(&mut self, cx: &mut Cx<'_, '_>) -> bool {
        self.abort(cx);
        cx.set_cursor(None);
        cx.out.view.features = Default::default();
        true
    }

    fn process(&mut self, cx: &mut Cx<'_, '_>, input: &BoardFsmInput) -> bool {
        match input {
            BoardFsmInput::PointerMoved(e) => {
                if self.placing.is_none() {
                    return false;
                }
                self.props.set_position(e.pos.mapped_to_grid(cx.grid()));
                self.preview(cx)
            }
            BoardFsmInput::LeftPressed(e) | BoardFsmInput::LeftDoubleClicked(e) => {
                let pos = e.pos.mapped_to_grid(cx.grid());
                if self.placing.is_some() {
                    self.finish(cx, pos);
                }
                self.start(cx, pos);
                true
            }
            BoardFsmInput::RightReleased(_) => self.rotate(cx, Angle::DEG90),
            BoardFsmInput::Rotate(angle) => self.rotate(cx, *angle),
            BoardFsmInput::ToolSetting(setting) => self.setting(cx, setting),
            _ => false,
        }
    }

    fn tool_data(&self, cx: &Cx<'_, '_>) -> BoardToolData {
        let fiducial = self.props.function_is_fiducial();
        BoardToolData {
            nets: super::add_via::sorted_nets(cx),
            net: ToolNet {
                auto: false,
                net: self.net,
            },
            line_width: Some(UnsignedLength::from(self.props.width())),
            size: (!fiducial).then_some(self.props.height()),
            drill: self.drill(),
            pad_shape: Some(self.tool_shape()),
            pad_radius: Some(self.props.radius()),
            component_side: (!self.is_tht()).then_some(self.props.component_side()),
            fiducial,
            press_fit: self
                .is_tht()
                .then_some(self.props.function() == PadFunction::PressFitPad),
            copper_clearance: fiducial.then_some(self.props.copper_clearance()),
            ..Default::default()
        }
    }
}

impl AddPadState {
    /// Upstream `processRotate()`.
    fn rotate(&mut self, cx: &mut Cx<'_, '_>, angle: Angle) -> bool {
        if self.placing.is_none() {
            return false;
        }
        self.props.set_rotation(self.props.rotation() + angle);
        self.preview(cx)
    }
}
