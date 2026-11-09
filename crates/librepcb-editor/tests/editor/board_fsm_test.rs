//! Headless scenarios of the board editor FSM on a project built with the
//! editor commands, with a `BoardScene` as view.

use librepcb_core::geometry::TraceAnchor;
use librepcb_core::project::board::Board;
use librepcb_core::project::{BoardId, ComponentInstanceId, NetSignalId, Project};
use librepcb_core::types::{
    Angle, CircuitIdentifier, ElementName, Layer, Length, Orientation, Point,
};
use librepcb_editor::ProjectEditor;
use librepcb_editor::commands::{
    AddBoard, AddComponent, AddDevice, AddHole, AddSchematic, AddTrace, ConnectPinToNet,
    ConnectPins, PadRef, PinRef, SetBoardOutline, SymbolPlacement, TraceEndpoint,
};
use librepcb_editor::fsm::board::{
    BoardContext, BoardEditorFsm, BoardEditorSettings, BoardFsmInput, BoardItemRef, BoardRequest,
    BoardTool, BoardView, ToolSetting, board_clipboard_mime_type,
};
use librepcb_editor::fsm::{Clipboard, Key, KeyEvent, MemoryClipboard, Modifiers, PointerEvent};
use librepcb_scene::librepcb_canvas::convert;
use librepcb_scene::librepcb_canvas::kurbo::Rect;
use librepcb_scene::{BoardObject, BoardScene, BoardSceneLayer, BoardSide, ColorScheme, SceneSync};

use crate::helpers::{create_editor, lib};

fn mm(x: f64, y: f64) -> Point {
    Point::new(Length::from_mm(x).unwrap(), Length::from_mm(y).unwrap())
}

fn name(s: &str) -> ElementName {
    ElementName::new(s).unwrap()
}

fn ident(s: &str) -> CircuitIdentifier {
    CircuitIdentifier::new(s).unwrap()
}

/// The view of the tests: a headless board scene.
struct SceneView {
    scene: BoardScene,
    sync: SceneSync,
}

fn to_ref(o: BoardObject) -> Option<BoardItemRef> {
    Some(match o {
        BoardObject::Device(c) => BoardItemRef::Device(c),
        BoardObject::FootprintPad(c, u) => BoardItemRef::FootprintPad(c, u),
        BoardObject::Pad(s, u) => BoardItemRef::Pad(s, u),
        BoardObject::Via(s, u) => BoardItemRef::Via(s, u),
        BoardObject::Trace(s, u) => BoardItemRef::Trace(s, u),
        BoardObject::Plane(p) => BoardItemRef::Plane(p),
        BoardObject::Zone(u) => BoardItemRef::Zone(u),
        BoardObject::Polygon(u) => BoardItemRef::Polygon(u),
        BoardObject::StrokeText(u) => BoardItemRef::StrokeText(u),
        BoardObject::Hole(u) => BoardItemRef::Hole(u),
        BoardObject::AirWire(_) => return None,
    })
}

impl SceneView {
    fn sync(&mut self, project: &Project) {
        self.sync.sync(project, &mut self.scene).unwrap();
    }
}

impl BoardView for SceneView {
    fn items_at(&self, pos: Point, tolerance: Length) -> Vec<BoardItemRef> {
        self.scene
            .scene()
            .items_at(convert::point(pos), tolerance.to_mm())
            .into_iter()
            .filter_map(|id| self.scene.object(id).and_then(to_ref))
            .collect()
    }

    fn items_in_rect(&self, p1: Point, p2: Point) -> Vec<BoardItemRef> {
        let rect = Rect::from_points(convert::point(p1), convert::point(p2));
        self.scene
            .scene()
            .items_in_rect(
                rect,
                librepcb_scene::librepcb_canvas::SelectionMode::Intersects,
            )
            .into_iter()
            .filter_map(|id| self.scene.object(id).and_then(to_ref))
            .collect()
    }

    fn tolerance(&self) -> Length {
        Length::from_mm(0.1).unwrap()
    }

    fn is_layer_visible(&self, layer: Layer) -> bool {
        self.scene
            .scene()
            .is_layer_visible(BoardSceneLayer::Board(layer).id())
    }
}

/// An editor with a board and the FSM.
struct Harness {
    _dir: tempfile::TempDir,
    editor: ProjectEditor,
    view: SceneView,
    fsm: BoardEditorFsm,
    board: BoardId,
    clipboard: MemoryClipboard,
    requests: Vec<BoardRequest>,
}

impl Harness {
    fn new(dir: tempfile::TempDir, mut editor: ProjectEditor, board: BoardId) -> Self {
        editor.rebuild_air_wires(board).unwrap();
        let scene = BoardScene::build(
            editor.project(),
            board,
            BoardSide::Top,
            &ColorScheme::BOARD_DARK,
        )
        .unwrap();
        let view = SceneView {
            scene,
            sync: SceneSync::new(editor.project()),
        };
        let fsm = BoardEditorFsm::new(board, BoardEditorSettings::default());
        Self {
            _dir: dir,
            editor,
            view,
            fsm,
            board,
            clipboard: MemoryClipboard::new(),
            requests: Vec::new(),
        }
    }

    fn input(&mut self, input: BoardFsmInput) -> bool {
        let mut ctx = BoardContext::new(&mut self.editor, &self.view, &mut self.clipboard);
        let handled = self.fsm.process(&mut ctx, input);
        // The application syncs its scene after each call.
        self.view.sync(self.editor.project());
        let requests = self.fsm.take_requests();
        for r in &requests {
            if let BoardRequest::ShowError(msg) = r {
                panic!("FSM error: {msg}");
            }
        }
        self.requests.extend(requests);
        handled
    }

    fn tool(&mut self, tool: BoardTool) {
        self.input(BoardFsmInput::Tool(tool));
    }

    fn move_to(&mut self, pos: Point) {
        self.input(BoardFsmInput::PointerMoved(PointerEvent::new(pos)));
    }

    /// Moves with the left button held (the FSM tracks the button).
    fn drag_to(&mut self, pos: Point) {
        self.move_to(pos);
    }

    fn press(&mut self, pos: Point) {
        self.input(BoardFsmInput::LeftPressed(PointerEvent::new(pos)));
    }

    fn release(&mut self, pos: Point) {
        self.input(BoardFsmInput::LeftReleased(PointerEvent::new(pos)));
    }

    fn click(&mut self, pos: Point) {
        self.move_to(pos);
        self.press(pos);
        self.release(pos);
    }

    fn project(&self) -> &Project {
        self.editor.project()
    }

    fn board(&self) -> &Board {
        self.project().board(self.board).unwrap()
    }

    fn component(&self, name: &str) -> ComponentInstanceId {
        self.project()
            .circuit()
            .component_instance_by_name(name)
            .unwrap()
            .0
    }

    fn net(&self, name: &str) -> NetSignalId {
        self.project().circuit().net_signal_by_name(name).unwrap().0
    }

    /// Position of a footprint pad by component and pad name.
    fn pad_pos(&self, component: &str, pad: &str) -> Point {
        let c = self.component(component);
        let p = self.project();
        let dev = self.board().device(c).unwrap();
        dev.pads(p.library(), p.circuit())
            .unwrap()
            .into_iter()
            .find(|v| v.package_pad().is_some_and(|pp| pp.name().as_str() == pad))
            .unwrap()
            .position()
    }

    fn traces(&self) -> Vec<librepcb_core::geometry::Trace> {
        self.board()
            .net_segments()
            .values()
            .flat_map(|s| s.traces().values().cloned())
            .collect()
    }

    fn vias(&self) -> Vec<librepcb_core::geometry::Via> {
        self.board()
            .net_segments()
            .values()
            .flat_map(|s| s.vias().values().cloned())
            .collect()
    }

    fn air_wire_count(&self, net: NetSignalId) -> usize {
        self.board()
            .derived()
            .air_wires()
            .get(&Some(net))
            .map_or(0, Vec::len)
    }

    fn undo_text(&self) -> Option<String> {
        self.editor.undo_stack().undo_text().map(str::to_owned)
    }
}

/// R1 and R2 in series between VCC and GND (middle net MID), C1 between
/// VCC and GND, placed on a 40 x 20 mm board; `with_trace` adds a trace
/// R1.2 → R2.1.
fn build(with_trace: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut editor = create_editor(dir.path());
    let schematic = editor
        .execute(AddSchematic {
            name: name("Main"),
            index: None,
        })
        .unwrap();
    let board = editor
        .execute(AddBoard::new(name("default")))
        .unwrap()
        .board;
    let place = |x, y, rotation| SymbolPlacement {
        schematic: Some(schematic),
        position: mm(x, y),
        rotation,
        mirrored: false,
        gate_offset: None,
    };
    for (device, component, x, y, rot) in [
        (lib::r0805(), lib::resistor(), 20.32, 50.8, Angle::DEG0),
        (lib::r0805(), lib::resistor(), 40.64, 50.8, Angle::DEG0),
        (lib::c0805(), lib::capacitor(), 60.96, 40.64, Angle::DEG90),
    ] {
        editor
            .execute(AddComponent {
                device: Some(device),
                place: Some(place(x, y, rot)),
                ..AddComponent::new(component)
            })
            .unwrap();
    }
    editor
        .execute(ConnectPins {
            a: PinRef::new("R1", "2"),
            b: PinRef::new("R2", "1"),
            mode: Default::default(),
            net: Some(ident("MID")),
        })
        .unwrap();
    for (component, pin, net) in [
        ("R1", "1", "VCC"),
        ("C1", "1", "VCC"),
        ("R2", "2", "GND"),
        ("C1", "2", "GND"),
    ] {
        editor
            .execute(ConnectPinToNet {
                pin: PinRef::new(component, pin),
                net: ident(net),
                label: true,
                stub_length: None,
            })
            .unwrap();
    }
    for (component, x) in [("R1", 10.0), ("R2", 20.0), ("C1", 30.0)] {
        editor
            .execute(AddDevice {
                component: component.into(),
                board: Some(board),
                device: None,
                footprint: None,
                position: mm(x, 10.0),
                rotation: Angle::DEG0,
                mirrored: false,
            })
            .unwrap();
    }
    editor
        .execute(SetBoardOutline::rect(
            Some(board),
            mm(0.0, 0.0),
            mm(40.0, 20.0),
        ))
        .unwrap();
    if with_trace {
        editor
            .execute(AddTrace {
                board: Some(board),
                start: TraceEndpoint::Pad(PadRef::new("R1", "2")),
                end: TraceEndpoint::Pad(PadRef::new("R2", "1")),
                points: vec![],
                layer: None,
                width: None,
                net: None,
            })
            .unwrap();
    }
    Harness::new(dir, editor, board)
}

#[test]
fn move_device_traces_follow() {
    let mut h = build(true);
    let r1 = h.component("R1");
    let before = h.board().device(r1).unwrap().position();
    let pad_before = h.pad_pos("R1", "2");
    let grab = h.pad_pos("R1", "1");
    let undo_len = h.editor.undo_stack().index();

    // Press on the free pad of R1 (selects the device or the pad; on pad 2
    // the trace would be selected, like upstream), drag, release.
    h.move_to(grab);
    h.press(grab);
    assert!(
        h.fsm.selection().contains(BoardItemRef::Device(r1))
            || h.fsm
                .selection()
                .items()
                .iter()
                .any(|i| matches!(i, BoardItemRef::FootprintPad(c, _) if *c == r1))
    );
    h.drag_to(grab + mm(0.0, 2.54));
    // Live preview: the device moved already, inside an active group.
    assert!(h.editor.undo_stack().is_group_active());
    assert_eq!(
        h.board().device(r1).unwrap().position(),
        before + mm(0.0, 2.54)
    );
    h.drag_to(grab + mm(1.27, 5.08));
    h.release(grab + mm(1.27, 5.08));
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.editor.undo_stack().index(), undo_len + 1);
    assert_eq!(h.undo_text().as_deref(), Some("Drag Board Elements"));
    let after = h.board().device(r1).unwrap().position();
    assert_eq!(after, before + mm(1.27, 5.08));
    assert!(h.fsm.selection().contains(BoardItemRef::Device(r1)));

    // The trace follows: its anchor at R1.2 is at the new pad position.
    let pad_after = h.pad_pos("R1", "2");
    assert_eq!(pad_after, pad_before + mm(1.27, 5.08));
    let traces = h.traces();
    assert_eq!(traces.len(), 1);
    let seg = h.board().net_segments().values().next().unwrap();
    let p = h.project();
    let positions: Vec<Point> = [traces[0].p1(), traces[0].p2()]
        .into_iter()
        .map(|a| {
            h.board()
                .anchor_position(seg, a, p.library(), p.circuit())
                .unwrap()
        })
        .collect();
    assert!(positions.contains(&pad_after));

    // Undo restores the position, redo applies it again.
    assert!(h.editor.undo().unwrap());
    assert_eq!(h.board().device(r1).unwrap().position(), before);
    assert!(h.editor.redo().unwrap());
    assert_eq!(h.board().device(r1).unwrap().position(), after);
}

#[test]
fn rotate_flip_and_keyboard_move() {
    let mut h = build(false);
    let c1 = h.component("C1");
    let pos = h.board().device(c1).unwrap().position();
    h.click(pos);
    assert!(h.fsm.selection().contains(BoardItemRef::Device(c1)));
    h.input(BoardFsmInput::Rotate(Angle::DEG90));
    assert_eq!(h.board().device(c1).unwrap().rotation(), Angle::DEG90);
    h.input(BoardFsmInput::Move(mm(2.54, 0.0)));
    assert_eq!(
        h.board().device(c1).unwrap().position(),
        pos + mm(2.54, 0.0)
    );
    h.input(BoardFsmInput::Flip(Orientation::Horizontal));
    let dev = h.board().device(c1).unwrap();
    assert!(dev.mirrored());
    assert_eq!(dev.rotation(), -Angle::DEG90);
    // Device texts are on the bottom side now.
    assert!(dev.stroke_texts().values().all(|t| t.layer().is_bottom()));
    // Undo all three actions.
    for _ in 0..3 {
        assert!(h.editor.undo().unwrap());
    }
    let dev = h.board().device(c1).unwrap();
    assert_eq!(
        (dev.position(), dev.rotation(), dev.mirrored()),
        (pos, Angle::DEG0, false)
    );
}

#[test]
fn draw_trace_pad_to_pad() {
    let mut h = build(false);
    let mid = h.net("MID");
    assert_eq!(h.air_wire_count(mid), 1);
    let a = h.pad_pos("R1", "2");
    let b = h.pad_pos("R2", "1");
    let undo_len = h.editor.undo_stack().index();
    h.tool(BoardTool::DrawTrace);
    assert_eq!(h.fsm.tool(), BoardTool::DrawTrace);
    h.click(a);
    // Positioning: a live preview is in the model, the net is highlighted.
    assert!(h.editor.undo_stack().is_group_active());
    assert!(h.fsm.highlighted_nets().contains(&mid));
    h.move_to(b);
    h.press(b);
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.editor.undo_stack().index(), undo_len + 1);
    assert_eq!(h.undo_text().as_deref(), Some("Draw Board Trace"));

    // One segment of MID connecting both pads.
    let segs: Vec<_> = h.board().net_segments().values().collect();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].net(), Some(mid));
    let pads: Vec<TraceAnchor> = segs[0]
        .traces()
        .values()
        .flat_map(|t| [t.p1(), t.p2()])
        .filter(|a| matches!(a, TraceAnchor::FootprintPad { .. }))
        .collect();
    assert_eq!(pads.len(), 2, "{:?}", segs[0]);
    assert!(
        segs[0]
            .traces()
            .values()
            .all(|t| t.layer() == Layer::TOP_COPPER)
    );
    // The air wire disappeared.
    assert_eq!(h.air_wire_count(mid), 0);

    // Undo removes the trace (and the air wire comes back).
    assert!(h.editor.undo().unwrap());
    assert!(h.traces().is_empty());
    h.editor.rebuild_air_wires(h.board).unwrap();
    assert_eq!(h.air_wire_count(mid), 1);
    assert!(h.editor.redo().unwrap());
    assert!(!h.traces().is_empty());
}

#[test]
fn draw_trace_with_corner_and_abort() {
    let mut h = build(false);
    let a = h.pad_pos("R1", "1");
    h.tool(BoardTool::DrawTrace);
    h.click(a);
    // HV mode: corner at (x of target, y of start).
    let corner = mm(5.08, 15.24);
    h.move_to(corner);
    h.press(corner);
    // Continues from the new junction.
    assert!(h.editor.undo_stack().is_group_active());
    h.input(BoardFsmInput::Abort);
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.fsm.tool(), BoardTool::DrawTrace);
    // Two traces (pad → middle → corner) were committed.
    assert_eq!(h.traces().len(), 2);
    let seg = h.board().net_segments().values().next().unwrap();
    assert!(seg.junctions().values().any(|j| j.position() == corner));
    // A second abort leaves the tool.
    h.input(BoardFsmInput::Abort);
    assert_eq!(h.fsm.tool(), BoardTool::Select);
}

#[test]
fn draw_trace_layer_change_inserts_via() {
    let mut h = build(false);
    let a = h.pad_pos("R1", "2");
    let corner = mm(12.7, 15.24);
    let end = mm(17.78, 15.24);
    h.tool(BoardTool::DrawTrace);
    h.click(a);
    h.move_to(corner);
    h.press(corner);
    h.move_to(corner + mm(2.54, 0.0));
    // Change the layer while positioning: a via is added at the end.
    h.input(BoardFsmInput::ToolSetting(ToolSetting::Layer(
        Layer::BOT_COPPER,
    )));
    assert_eq!(h.fsm.tool_data().layer, Some(Layer::BOT_COPPER));
    assert_eq!(h.vias().len(), 1, "via preview");
    let via_pos = corner + mm(2.54, 0.0);
    h.press(via_pos);
    // Continue on the bottom layer from the via.
    h.move_to(end);
    h.press(end);
    h.input(BoardFsmInput::Abort);
    assert!(!h.editor.undo_stack().is_group_active());

    let vias = h.vias();
    assert_eq!(vias.len(), 1);
    assert_eq!(vias[0].position(), via_pos);
    let segs: Vec<_> = h.board().net_segments().values().collect();
    assert_eq!(segs.len(), 1, "all in one segment");
    let seg = segs[0];
    let via = TraceAnchor::Via(vias[0].uuid());
    let layers: Vec<Layer> = seg.traces_at(via).map(|t| t.layer()).collect();
    assert!(layers.contains(&Layer::TOP_COPPER), "{layers:?}");
    assert!(layers.contains(&Layer::BOT_COPPER), "{layers:?}");
    assert!(
        seg.traces()
            .values()
            .any(|t| t.layer() == Layer::BOT_COPPER)
    );
}

#[test]
fn draw_trace_shift_and_wire_mode() {
    let mut h = build(false);
    let a = h.pad_pos("R1", "1");
    h.tool(BoardTool::DrawTrace);
    h.click(a);
    // Right click cycles the wire mode.
    let before = h.fsm.tool_data().wire_mode;
    h.input(BoardFsmInput::RightReleased(PointerEvent::new(a)));
    assert_ne!(h.fsm.tool_data().wire_mode, before);
    assert_eq!(h.fsm.tool(), BoardTool::DrawTrace);
    h.input(BoardFsmInput::KeyPressed(KeyEvent {
        key: Key::Shift,
        modifiers: Modifiers::SHIFT,
    }));
    h.input(BoardFsmInput::KeyReleased(KeyEvent::new(Key::Shift)));
    h.input(BoardFsmInput::Abort);
    assert!(h.traces().is_empty());
    assert!(!h.editor.undo_stack().is_group_active());
}

#[test]
fn rubber_band_and_delete() {
    let mut h = build(true);
    let r1 = h.component("R1");
    let r2 = h.component("R2");
    // Rubber band around R1 only.
    // (Above the trace at y = 10 mm, touching the pads of R1.)
    h.press(mm(7.0, 10.4));
    assert!(h.fsm.selection().is_empty());
    h.drag_to(mm(12.0, 12.0));
    assert!(h.fsm.view_state().rubber_band.is_some());
    h.release(mm(12.0, 12.0));
    assert!(h.fsm.view_state().rubber_band.is_none());
    assert!(h.fsm.selection().contains(BoardItemRef::Device(r1)));
    assert!(!h.fsm.selection().contains(BoardItemRef::Device(r2)));
    assert!(!h.fsm.view_state().info_box.is_empty());
    h.input(BoardFsmInput::Remove);
    assert!(h.board().device(r1).is_none());
    assert!(h.board().device(r2).is_some());
    assert_eq!(h.undo_text().as_deref(), Some("Remove Board Items"));
    // The trace at R1's pad ends in a junction now.
    let traces = h.traces();
    assert_eq!(traces.len(), 1);
    assert!(
        [traces[0].p1(), traces[0].p2()]
            .iter()
            .any(|a| matches!(a, TraceAnchor::Junction(_)))
    );
    assert!(h.editor.undo().unwrap());
    assert!(h.board().device(r1).is_some());
}

#[test]
fn select_all_and_delete_traces_by_click() {
    let mut h = build(true);
    // Click on the middle of the trace selects it.
    let a = h.pad_pos("R1", "2");
    let b = h.pad_pos("R2", "1");
    let middle = (a + b) / 2;
    h.click(middle);
    assert!(
        h.fsm
            .selection()
            .items()
            .iter()
            .any(|i| matches!(i, BoardItemRef::Trace(..))),
        "{:?}",
        h.fsm.selection()
    );
    h.input(BoardFsmInput::Remove);
    assert!(h.traces().is_empty());
    h.input(BoardFsmInput::SelectAll);
    assert!(h.fsm.selection().items().len() >= 4);
    h.input(BoardFsmInput::Abort);
    assert!(h.fsm.selection().is_empty());
}

#[test]
fn add_via() {
    let mut h = build(true);
    let a = h.pad_pos("R1", "2");
    let b = h.pad_pos("R2", "1");
    let middle = ((a + b) / 2).mapped_to_grid(h.board().settings().grid_interval);
    h.move_to(mm(30.0, 5.0));
    h.tool(BoardTool::AddVia);
    assert!(h.editor.undo_stack().is_group_active());
    assert_eq!(h.vias().len(), 1, "preview via");
    // Place a via in free space.
    h.move_to(mm(25.4, 2.54));
    h.press(mm(25.4, 2.54));
    // Place a via on the trace: it is split and connected.
    h.move_to(middle);
    h.press(middle);
    h.input(BoardFsmInput::Abort);
    assert_eq!(h.fsm.tool(), BoardTool::Select);
    assert!(!h.editor.undo_stack().is_group_active());
    let vias = h.vias();
    assert_eq!(vias.len(), 2);
    let mid = h.net("MID");
    let seg = h
        .board()
        .net_segments()
        .values()
        .find(|s| s.vias().values().any(|v| v.position() == middle))
        .unwrap();
    assert_eq!(seg.net(), Some(mid));
    assert_eq!(seg.traces().len(), 2);
    assert_eq!(h.undo_text().as_deref(), Some("Add via to board"));
}

#[test]
fn copy_paste() {
    let mut h = build(true);
    let board = h.board;
    h.editor
        .execute(AddHole {
            board: Some(board),
            position: mm(5.08, 5.08),
            diameter: librepcb_core::types::PositiveLength::new(Length::from_mm(1.0).unwrap())
                .unwrap(),
            stop_mask: None,
        })
        .unwrap();
    h.view.sync(h.editor.project());
    // Select the hole and the trace with a rubber band from the hole to
    // the trace's middle.
    let a = h.pad_pos("R1", "2");
    let b = h.pad_pos("R2", "1");
    let middle = (a + b) / 2;
    h.press(mm(4.0, 4.0));
    h.drag_to(mm(15.5, 10.5));
    h.release(mm(15.5, 10.5));
    let sel = h.fsm.selection().items().clone();
    assert!(
        sel.iter().any(|i| matches!(i, BoardItemRef::Hole(_))),
        "{sel:?}"
    );
    assert!(
        sel.iter().any(|i| matches!(i, BoardItemRef::Trace(..))),
        "{sel:?}"
    );
    h.move_to(mm(5.08, 5.08));
    h.input(BoardFsmInput::Copy);
    let mime = board_clipboard_mime_type(&BoardEditorSettings::default().app_version);
    let zip = h.clipboard.get(&mime).expect("board data in the clipboard");
    let data = librepcb_editor::fsm::board::BoardClipboardData::from_zip(&zip).unwrap();
    assert_eq!(data.holes.len(), 1);
    assert_eq!(data.net_segments.len(), 1);
    let text = data
        .to_sexpression()
        .to_string_with_mode(librepcb_core::serialization::Mode::LibrePcb)
        .unwrap();
    assert!(text.starts_with("(librepcb_clipboard_board"), "{text}");
    let _ = middle;

    // Paste 10 mm to the right: the items follow the cursor until the
    // click places them.
    let traces_before = h.traces().len();
    let target = mm(15.24, 5.08);
    h.move_to(target);
    assert!(h.input(BoardFsmInput::Paste));
    assert!(h.editor.undo_stack().is_group_active());
    h.move_to(target + mm(0.0, 2.54));
    h.press(target + mm(0.0, 2.54));
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.undo_text().as_deref(), Some("Paste board elements"));
    let holes: Vec<Point> = h
        .board()
        .holes()
        .values()
        .map(|h| h.path().first().pos)
        .collect();
    assert_eq!(holes.len(), 2);
    assert!(holes.contains(&(target + mm(0.0, 2.54))), "{holes:?}");
    // The pasted trace ends in junctions (the pads were not copied).
    assert_eq!(h.traces().len(), traces_before + 1);
    assert_eq!(h.board().net_segments().len(), 2);
    // Undo removes all pasted items.
    assert!(h.editor.undo().unwrap());
    assert_eq!(h.board().holes().len(), 1);
    assert_eq!(h.traces().len(), traces_before);
}

#[test]
fn add_hole_text_polygon_zone_plane() {
    let mut h = build(false);
    let polygons = h.board().polygons().len();
    // Hole.
    h.move_to(mm(2.54, 2.54));
    h.tool(BoardTool::AddHole);
    h.click(mm(5.08, 2.54));
    h.input(BoardFsmInput::Abort);
    assert_eq!(h.board().holes().len(), 1);
    // Text.
    h.tool(BoardTool::AddStrokeText);
    h.input(BoardFsmInput::ToolSetting(ToolSetting::Text(
        "Hello".into(),
    )));
    h.input(BoardFsmInput::Rotate(Angle::DEG90));
    h.click(mm(7.62, 2.54));
    h.input(BoardFsmInput::Abort);
    let texts: Vec<_> = h.board().stroke_texts().values().collect();
    assert_eq!(texts.len(), 1);
    assert_eq!(texts[0].text(), "Hello");
    assert_eq!(texts[0].rotation(), Angle::DEG90);
    // Polygon: three segments, finished by clicking the last point again.
    h.tool(BoardTool::DrawPolygon);
    h.input(BoardFsmInput::ToolSetting(ToolSetting::Layer(
        Layer::TOP_LEGEND,
    )));
    for p in [
        mm(1.27, 1.27),
        mm(3.81, 1.27),
        mm(3.81, 3.81),
        mm(3.81, 3.81),
    ] {
        h.click(p);
    }
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.board().polygons().len(), polygons + 1);
    let poly = h
        .board()
        .polygons()
        .values()
        .find(|p| p.layer() == Layer::TOP_LEGEND)
        .unwrap();
    assert_eq!(poly.path().vertices().len(), 3);
    h.input(BoardFsmInput::Abort);
    // Zone: a triangle.
    h.tool(BoardTool::DrawZone);
    for p in [
        mm(20.32, 2.54),
        mm(25.4, 2.54),
        mm(25.4, 7.62),
        mm(25.4, 7.62),
    ] {
        h.click(p);
    }
    h.input(BoardFsmInput::Abort);
    assert_eq!(h.board().zones().len(), 1);
    // Plane: automatic outline.
    h.tool(BoardTool::DrawPlane);
    h.input(BoardFsmInput::AutoAddPlane);
    assert_eq!(h.board().planes().len(), 1);
    assert_eq!(h.undo_text().as_deref(), Some("Draw Board Plane"));
    h.input(BoardFsmInput::Abort);
    assert_eq!(h.fsm.tool(), BoardTool::Select);
}

#[test]
fn add_device_from_unplaced_components() {
    let dir = tempfile::tempdir().unwrap();
    let mut editor = create_editor(dir.path());
    editor
        .execute(AddSchematic {
            name: name("Main"),
            index: None,
        })
        .unwrap();
    let board = editor
        .execute(AddBoard::new(name("default")))
        .unwrap()
        .board;
    let added = editor
        .execute(AddComponent {
            device: Some(lib::r0805()),
            ..AddComponent::new(lib::resistor())
        })
        .unwrap();
    let mut h = Harness::new(dir, editor, board);
    let c = added.component;
    h.move_to(mm(10.0, 10.0));
    assert!(h.input(BoardFsmInput::AddDevice {
        component: c,
        device: lib::r0805(),
        footprint: None,
    }));
    assert_eq!(h.fsm.tool(), BoardTool::AddDevice);
    assert!(h.board().device(c).is_some());
    h.move_to(mm(12.7, 7.62));
    h.input(BoardFsmInput::Rotate(Angle::DEG90));
    h.press(mm(12.7, 7.62));
    // Placed: the tool is left, one undo step.
    assert_eq!(h.fsm.tool(), BoardTool::Select);
    let dev = h.board().device(c).unwrap();
    assert_eq!(dev.position(), mm(12.7, 7.62));
    assert_eq!(dev.rotation(), Angle::DEG90);
    assert_eq!(h.undo_text().as_deref(), Some("Add device to board"));
    assert!(h.editor.undo().unwrap());
    assert!(h.board().device(c).is_none());
}

#[test]
fn measure_tool() {
    let mut h = build(false);
    h.tool(BoardTool::Measure);
    assert!(h.fsm.view_state().gray_out);
    h.click(mm(0.0, 0.0));
    h.move_to(mm(3.0, 4.0));
    h.press(mm(3.0, 4.0));
    let (start, end) = h.fsm.view_state().ruler.unwrap();
    assert_eq!(start, mm(0.0, 0.0));
    assert!(h.fsm.view_state().info_box.contains("Δ"));
    let _ = end;
    assert!(h.input(BoardFsmInput::Copy));
    assert!(h.clipboard.get("text/plain").is_some());
    h.tool(BoardTool::Select);
    assert!(!h.fsm.view_state().gray_out);
}

#[test]
fn right_click_opens_context_menu_and_properties() {
    let mut h = build(true);
    let c1 = h.component("C1");
    let pos = h.board().device(c1).unwrap().position();
    h.input(BoardFsmInput::RightReleased(PointerEvent::new(pos)));
    let menu = h
        .requests
        .iter()
        .find_map(|e| match e {
            BoardRequest::ContextMenu { items, .. } => Some(items.clone()),
            _ => None,
        })
        .unwrap();
    assert!(menu.iter().any(|i| i.default));
    h.input(BoardFsmInput::ContextMenu(
        librepcb_editor::fsm::board::ContextAction::Lock(true),
    ));
    assert!(h.board().device(c1).unwrap().locked());
    // A locked device is not dragged.
    let before = h.board().device(c1).unwrap().position();
    h.press(pos);
    h.drag_to(pos + mm(2.54, 0.0));
    h.release(pos + mm(2.54, 0.0));
    assert_eq!(h.board().device(c1).unwrap().position(), before);
    // Double click requests the properties dialog.
    h.input(BoardFsmInput::LeftDoubleClicked(PointerEvent::new(pos)));
    assert!(
        h.requests
            .iter()
            .any(|e| matches!(e, BoardRequest::Properties(BoardItemRef::Device(c)) if *c == c1))
    );
}

/// Moving everything on the boards of the upstream test projects and
/// undoing it restores the boards exactly.
#[test]
fn upstream_projects_move_all_and_undo() {
    for dir in crate::helpers::test_data_dir()
        .join("projects")
        .read_dir()
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join(".librepcb-project").exists())
    {
        let project = open_upstream(&dir);
        let boards: Vec<BoardId> = project.boards().iter().map(|b| b.id()).collect();
        let mut editor = ProjectEditor::new(project);
        for board in boards {
            let original = editor.project().board(board).unwrap().clone();
            let tmp = tempfile::tempdir().unwrap();
            let mut h = Harness::new(tmp, editor, board);
            h.fsm.set_settings(BoardEditorSettings {
                ignore_locks: true,
                ..Default::default()
            });
            let start = h.editor.undo_stack().index();
            h.input(BoardFsmInput::SelectAll);
            h.input(BoardFsmInput::Move(mm(2.54, 0.0)));
            h.input(BoardFsmInput::Rotate(Angle::DEG90));
            let has_items = !h.board().devices().is_empty() || !h.board().net_segments().is_empty();
            assert!(
                !has_items || h.editor.undo_stack().index() == start + 2,
                "{}: nothing moved",
                dir.display()
            );
            if has_items {
                assert!(*h.board() != original, "{}: board unchanged", dir.display());
            }
            for _ in start..h.editor.undo_stack().index() {
                h.editor.undo().unwrap();
            }
            let after = h.board().clone();
            assert!(after == original, "{}: board not restored", dir.display());
            h.input(BoardFsmInput::Abort);
            editor = h.editor;
        }
    }
}

fn open_upstream(dir: &std::path::Path) -> Project {
    use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
    let lpp = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".lpp"))
        .unwrap();
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(dir).unwrap()).unwrap();
    let directory = TransactionalDirectory::new(std::sync::Arc::new(fs), "");
    librepcb_core::project::ProjectLoader::new()
        .open(directory, &lpp)
        .unwrap()
}

#[test]
fn draw_trace_simplifies_collinear_traces() {
    use librepcb_editor::fsm::board::WireMode;
    let mut h = build(false);
    h.tool(BoardTool::DrawTrace);
    h.input(BoardFsmInput::ToolSetting(ToolSetting::WireMode(
        WireMode::Straight,
    )));
    // Three collinear points in free space.
    h.click(mm(2.54, 15.24));
    h.move_to(mm(5.08, 15.24));
    h.press(mm(5.08, 15.24));
    h.move_to(mm(7.62, 15.24));
    h.press(mm(7.62, 15.24));
    // Finishing the trace simplifies the segment: one straight trace.
    h.input(BoardFsmInput::Abort);
    assert!(!h.editor.undo_stack().is_group_active());
    let traces = h.traces();
    assert_eq!(traces.len(), 1, "{traces:?}");
    assert_eq!(
        h.undo_text().as_deref(),
        Some("Simplify Board Net Segments")
    );
    let seg = h.board().net_segments().values().next().unwrap();
    assert_eq!(seg.junctions().len(), 2);
    // Undo restores the two traces.
    assert!(h.editor.undo().unwrap());
    assert_eq!(h.traces().len(), 2);
}

#[test]
fn remove_trace_simplifies_segment() {
    use librepcb_editor::fsm::board::WireMode;
    let mut h = build(false);
    h.tool(BoardTool::DrawTrace);
    h.input(BoardFsmInput::ToolSetting(ToolSetting::WireMode(
        WireMode::Straight,
    )));
    // A T junction: a line with a stub at its middle.
    h.click(mm(2.54, 15.24));
    h.move_to(mm(5.08, 15.24));
    h.press(mm(5.08, 15.24));
    h.move_to(mm(7.62, 15.24));
    h.press(mm(7.62, 15.24));
    h.input(BoardFsmInput::Abort);
    h.click(mm(5.08, 15.24));
    h.move_to(mm(5.08, 17.78));
    h.press(mm(5.08, 17.78));
    h.input(BoardFsmInput::Abort);
    h.tool(BoardTool::Select);
    assert_eq!(h.traces().len(), 3);
    // Remove the stub: the remaining two collinear traces are merged.
    h.click(mm(5.08, 16.5));
    assert!(
        h.fsm
            .selection()
            .items()
            .iter()
            .any(|i| matches!(i, BoardItemRef::Trace(..)))
    );
    h.input(BoardFsmInput::Remove);
    assert_eq!(h.traces().len(), 1, "{:?}", h.traces());
    assert_eq!(
        h.undo_text().as_deref(),
        Some("Simplify Board Net Segments")
    );
}

#[test]
fn add_tht_and_smt_pads() {
    use librepcb_core::geometry::{ComponentSide, PadFunction};
    use librepcb_editor::fsm::board::{ToolNet, ToolPadShape};
    let mut h = build(false);
    let vcc = h.net("VCC");
    h.move_to(mm(2.54, 2.54));
    h.tool(BoardTool::AddThtPad);
    assert_eq!(h.fsm.tool(), BoardTool::AddThtPad);
    assert!(h.editor.undo_stack().is_group_active());
    let pads = |h: &Harness| -> Vec<librepcb_core::project::board::BoardPadData> {
        h.board()
            .net_segments()
            .values()
            .flat_map(|s| s.pads().values().cloned())
            .collect()
    };
    assert_eq!(pads(&h).len(), 1, "preview pad");
    let data = h.fsm.tool_data().clone();
    assert_eq!(data.drill.unwrap().to_mm(), 0.8);
    assert_eq!(data.press_fit, Some(false));
    assert!(data.component_side.is_none());
    // Tool bar: net, shape, larger drill (grows the pad).
    h.input(BoardFsmInput::ToolSetting(ToolSetting::Net(ToolNet {
        auto: false,
        net: Some(vcc),
    })));
    h.input(BoardFsmInput::ToolSetting(ToolSetting::PadShape(
        ToolPadShape::Octagon,
    )));
    h.input(BoardFsmInput::ToolSetting(ToolSetting::HoleDiameter(
        librepcb_core::types::PositiveLength::new(Length::from_mm(1.5).unwrap()).unwrap(),
    )));
    assert!(h.fsm.highlighted_nets().contains(&vcc));
    h.move_to(mm(5.08, 5.08));
    h.input(BoardFsmInput::Rotate(Angle::DEG90));
    h.press(mm(5.08, 5.08));
    // The next pad follows the cursor.
    assert!(h.editor.undo_stack().is_group_active());
    h.input(BoardFsmInput::Abort);
    assert_eq!(h.undo_text().as_deref(), Some("Add Pad to Board"));
    assert_eq!(h.fsm.tool(), BoardTool::Select);
    let placed = pads(&h);
    assert_eq!(placed.len(), 1);
    let pad = placed[0].pad();
    assert_eq!(pad.position(), mm(5.08, 5.08));
    assert_eq!(pad.rotation(), Angle::DEG90);
    assert!(pad.is_tht());
    assert_eq!(pad.holes().first().unwrap().diameter().to_mm(), 1.5);
    assert!(pad.height().to_mm() >= 1.5);
    let seg = h
        .board()
        .net_segments()
        .values()
        .find(|s| !s.pads().is_empty())
        .unwrap();
    assert_eq!(seg.net(), Some(vcc));

    // SMT fiducial on the bottom side.
    h.tool(BoardTool::AddSmtPad(PadFunction::GlobalFiducial));
    assert!(h.fsm.tool_data().fiducial);
    h.input(BoardFsmInput::ToolSetting(ToolSetting::ComponentSide(
        ComponentSide::Bottom,
    )));
    h.move_to(mm(10.16, 2.54));
    h.press(mm(10.16, 2.54));
    h.input(BoardFsmInput::Abort);
    let placed = pads(&h);
    assert_eq!(placed.len(), 2);
    let fid = placed
        .iter()
        .find(|p| p.pad().function() == PadFunction::GlobalFiducial)
        .unwrap();
    assert!(!fid.pad().is_tht());
    assert_eq!(fid.pad().component_side(), ComponentSide::Bottom);
    assert_eq!(fid.pad().width().to_mm(), 1.0);
    // Undo removes the fiducial.
    assert!(h.editor.undo().unwrap());
    assert_eq!(pads(&h).len(), 1);
}

#[test]
fn change_device_from_context_menu() {
    use librepcb_editor::fsm::board::ContextAction;
    let mut h = build(true);
    let r1 = h.component("R1");
    assert_eq!(h.board().device(r1).unwrap().lib_device(), lib::r0805());
    let pos = h.board().device(r1).unwrap().position();
    let pad = h.pad_pos("R1", "1");
    h.input(BoardFsmInput::RightReleased(PointerEvent::new(pad)));
    let menu = h
        .requests
        .iter()
        .find_map(|e| match e {
            BoardRequest::ContextMenu { items, .. } => Some(items.clone()),
            _ => None,
        })
        .unwrap();
    // The devices of the resistor component from the library source: the
    // current one is checked and disabled, the others can be chosen.
    let current = menu
        .iter()
        .find(|i| i.action == Some(ContextAction::ChangeDevice(lib::r0805())))
        .expect("current device listed");
    assert_eq!(current.checked, Some(true));
    assert!(!current.enabled);
    let other = menu
        .iter()
        .find(|i| i.action == Some(ContextAction::ChangeDevice(lib::r0603())))
        .expect("other device listed");
    assert!(other.enabled);
    assert!(other.text.contains('['), "{}", other.text);
    h.input(BoardFsmInput::ContextMenu(ContextAction::ChangeDevice(
        lib::r0603(),
    )));
    let dev = h.board().device(r1).unwrap();
    assert_eq!(dev.lib_device(), lib::r0603());
    assert_eq!(dev.position(), pos);
    assert_eq!(h.undo_text().as_deref(), Some("Change Device"));
    assert!(h.editor.undo().unwrap());
    assert_eq!(h.board().device(r1).unwrap().lib_device(), lib::r0805());
}

#[test]
fn find_components_and_nets() {
    use librepcb_editor::fsm::find::{FindCandidate, FindKind};
    let mut h = build(true);
    let r1 = h.component("R1");
    let vcc = h.net("VCC");
    let mut ctx = BoardContext::new(&mut h.editor, &h.view, &mut h.clipboard);
    h.fsm.refresh_find_suggestions(&ctx);
    // Components on the board and nets with labels (MID has none).
    let names: Vec<(FindKind, String)> = h
        .fsm
        .search()
        .suggestions()
        .iter()
        .map(|c| (c.kind, c.name.clone()))
        .collect();
    assert!(names.contains(&(FindKind::Component, "R1".into())));
    assert!(names.contains(&(FindKind::Net, "VCC".into())));
    assert!(!names.iter().any(|(_, n)| n == "MID"), "{names:?}");
    h.fsm.set_find_term("r1");
    assert_eq!(
        h.fsm.search().suggestions(),
        &[FindCandidate::component("R1")]
    );
    let result = h.fsm.find_next(&mut ctx);
    assert_eq!(result.components, vec![r1]);
    let (p1, p2) = result.zoom_rect.unwrap();
    let pos = h
        .editor
        .project()
        .board(h.board)
        .unwrap()
        .device(r1)
        .unwrap()
        .position();
    assert!(p1.x < pos.x && p1.y < pos.y && p2.x > pos.x && p2.y > pos.y);
    assert!(h.fsm.selection().contains(BoardItemRef::Device(r1)));

    let mut ctx = BoardContext::new(&mut h.editor, &h.view, &mut h.clipboard);
    h.fsm.set_find_term("VCC");
    let result = h.fsm.find_next(&mut ctx);
    assert_eq!(result.nets, vec![vcc]);
    assert!(h.fsm.highlighted_nets().contains(&vcc));
    assert!(
        h.fsm
            .selection()
            .items()
            .iter()
            .any(|i| matches!(i, BoardItemRef::FootprintPad(c, _) if *c == r1))
    );
    assert!(!h.fsm.selection().contains(BoardItemRef::Device(r1)));
}

fn dxf_settings(
    dir: &std::path::Path,
    placement: Option<Point>,
) -> librepcb_editor::fsm::board::DxfImportSettings {
    let file = dir.join("import.dxf");
    std::fs::write(
        &file,
        "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n\
         0\nSECTION\n2\nENTITIES\n\
         0\nLWPOLYLINE\n90\n4\n70\n1\n10\n0.0\n20\n0.0\n10\n10.0\n20\n0.0\n\
         10\n10.0\n20\n5.0\n10\n0.0\n20\n5.0\n\
         0\nCIRCLE\n10\n5.0\n20\n2.5\n40\n0.5\n\
         0\nENDSEC\n0\nEOF\n",
    )
    .unwrap();
    librepcb_editor::fsm::board::DxfImportSettings {
        file: librepcb_core::fileio::FilePath::new(&file).unwrap(),
        layer: Layer::BOARD_OUTLINES,
        line_width: librepcb_core::types::UnsignedLength::default(),
        scale_factor: 1.0,
        join_tangent_polylines: true,
        circles_as_drills: true,
        placement,
    }
}

#[test]
fn import_dxf_fixed_and_interactive() {
    let mut h = build(false);
    let tmp = tempfile::tempdir().unwrap();
    let polygons_before = h.board().polygons().len();
    let holes_before = h.board().holes().len();
    // Fixed placement: pasted and committed at once.
    let settings = dxf_settings(tmp.path(), Some(mm(20.0, 5.0)));
    assert!(h.input(BoardFsmInput::ImportDxf(settings)));
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.undo_text().as_deref(), Some("Paste board elements"));
    assert_eq!(h.board().polygons().len(), polygons_before + 1);
    assert_eq!(h.board().holes().len(), holes_before + 1);
    let hole = h
        .board()
        .holes()
        .values()
        .find(|hole| hole.path().vertices()[0].pos == mm(25.0, 7.5))
        .expect("hole at the circle center plus offset");
    assert_eq!(hole.diameter().to_mm(), 1.0);
    let polygon = h.board().polygons().values().find(|p| {
        p.layer() == Layer::BOARD_OUTLINES && p.path().vertices()[0].pos == mm(20.0, 5.0)
    });
    assert!(polygon.is_some());

    // Interactive placement: follows the cursor, a click places it.
    let settings = dxf_settings(tmp.path(), None);
    h.move_to(mm(0.0, 0.0));
    assert!(h.input(BoardFsmInput::ImportDxf(settings)));
    assert!(h.editor.undo_stack().is_group_active());
    h.move_to(mm(2.54, 2.54));
    h.press(mm(2.54, 2.54));
    h.release(mm(2.54, 2.54));
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.board().holes().len(), holes_before + 2);
    assert!(
        h.board()
            .holes()
            .values()
            .any(|hole| hole.path().vertices()[0].pos == mm(5.0 + 2.54, 2.5 + 2.54))
    );

    // A file without objects is an error.
    let empty = tmp.path().join("empty.dxf");
    std::fs::write(&empty, "").unwrap();
    let mut settings = dxf_settings(tmp.path(), None);
    settings.file = librepcb_core::fileio::FilePath::new(&empty).unwrap();
    let mut ctx = BoardContext::new(&mut h.editor, &h.view, &mut h.clipboard);
    assert!(!h.fsm.import_dxf(&mut ctx, settings));
    assert!(
        h.fsm
            .take_requests()
            .iter()
            .any(|r| matches!(r, BoardRequest::ShowError(_)))
    );
}

#[test]
fn paste_graphics_from_package_editor() {
    use librepcb_core::geometry::{Hole, NonEmptyPath, Path, Polygon};
    use librepcb_core::library::pkg::PackagePadList;
    use librepcb_core::types::{MaskConfig, PositiveLength, UnsignedLength, Uuid};
    use librepcb_editor::library_editor::commands::{
        FootprintClipboardData, footprint_clipboard_mime_type,
    };
    let mut h = build(false);
    let polygons = h.board().polygons().len();
    let holes = h.board().holes().len();
    let mut data =
        FootprintClipboardData::new(Uuid::new_random(), PackagePadList::new(), Point::ORIGIN);
    data.polygons.push(Polygon::new(
        Uuid::new_random(),
        Layer::TOP_LEGEND,
        UnsignedLength::new(Length::new(200_000)).unwrap(),
        false,
        false,
        Path::rect(mm(0.0, 0.0), mm(2.54, 2.54)),
    ));
    data.holes.push(Hole::new(
        Uuid::new_random(),
        PositiveLength::new(Length::new(1_000_000)).unwrap(),
        NonEmptyPath::from_point(mm(1.27, 1.27)),
        MaskConfig::Automatic,
    ));
    let version = BoardEditorSettings::default().app_version;
    h.clipboard.set(
        &footprint_clipboard_mime_type(&version),
        data.to_bytes().unwrap(),
    );
    let target = mm(20.32, 20.32);
    h.move_to(target);
    assert!(h.input(BoardFsmInput::Paste));
    assert!(h.editor.undo_stack().is_group_active());
    h.press(target);
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.board().polygons().len(), polygons + 1);
    assert_eq!(h.board().holes().len(), holes + 1);
    assert!(
        h.board()
            .holes()
            .values()
            .any(|hole| hole.path().first().pos == target + mm(1.27, 1.27))
    );
    assert!(h.editor.undo().unwrap());
    assert_eq!(h.board().polygons().len(), polygons);
    assert_eq!(h.board().holes().len(), holes);
}

#[test]
fn selection_is_cross_probed() {
    let mut h = build(true);
    let r1 = h.component("R1");
    // Clicking a pad of a device selects the device.
    let pos = h.pad_pos("R1", "1");
    h.click(pos);
    assert_eq!(h.fsm.cross_probe().components, [r1].into_iter().collect());
    assert!(h.fsm.cross_probe().nets.is_empty());
    // A trace: its net.
    let middle = (h.pad_pos("R1", "2") + h.pad_pos("R2", "1")) / 2;
    h.click(middle);
    assert!(
        h.fsm
            .selection()
            .items()
            .iter()
            .any(|i| matches!(i, BoardItemRef::Trace(..)))
    );
    let mid = h.net("MID");
    assert_eq!(h.fsm.cross_probe().nets, [mid].into_iter().collect());
    assert!(h.fsm.cross_probe().components.is_empty());
    assert!(h.fsm.highlighted_nets().contains(&mid));
    // Nothing selected: nothing to cross-probe.
    h.click(mm(35.0, 18.0));
    assert!(h.fsm.cross_probe().is_empty());
}
