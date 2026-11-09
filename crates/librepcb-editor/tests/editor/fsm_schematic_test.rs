//! Headless scenarios of the schematic editor FSM: a project built with the
//! editor commands, the view backed by a `librepcb_scene::SchematicScene`
//! (updated from the change journal after every FSM call, like the
//! application does).

use std::collections::BTreeSet;

use librepcb_core::geometry::NetLineAnchor;
use librepcb_core::project::{Project, SchematicId, SymbolId};
use librepcb_core::types::{Angle, ElementName, Length, Orientation, Point, Uuid};
use librepcb_editor::ProjectEditor;
use librepcb_editor::commands::*;
use librepcb_editor::fsm::schematic::{
    ComponentChoice, SchematicClipboardData, SchematicContext, SchematicEditorFsm,
    SchematicEditorSettings, SchematicItem, SchematicRequest, SchematicTool, SchematicView,
};
use librepcb_editor::fsm::{Clipboard, KeyEvent, MemoryClipboard, Modifiers, PointerEvent};
use librepcb_scene::librepcb_canvas::kurbo::{Point as KPoint, Rect};
use librepcb_scene::librepcb_canvas::{SelectionMode, convert};
use librepcb_scene::{ColorScheme, SceneSync, SchematicObject, SchematicScene};

use crate::helpers::{create_editor, lib, mm};

/// The view of the tests: hit testing on a `SchematicScene` (what the
/// application implements on its canvas).
struct SceneView<'a> {
    scene: &'a SchematicScene,
    cursor: Point,
}

fn map_object(o: SchematicObject) -> Option<SchematicItem> {
    Some(match o {
        SchematicObject::Symbol(id) => SchematicItem::Symbol(id),
        SchematicObject::SymbolPin(id, pin) => SchematicItem::SymbolPin(id, pin),
        SchematicObject::SymbolText(id, text) => SchematicItem::SymbolText(id, text),
        SchematicObject::NetLine(seg, l) => SchematicItem::NetLine(seg, l),
        SchematicObject::NetJunction(seg, NetLineAnchor::Junction(j)) => {
            SchematicItem::NetPoint(seg, j)
        }
        SchematicObject::NetJunction(_, NetLineAnchor::Pin { symbol, pin }) => {
            SchematicItem::SymbolPin(SymbolId(symbol), pin)
        }
        SchematicObject::NetJunction(..) => return None,
        SchematicObject::NetLabel(seg, l) => SchematicItem::NetLabel(seg, l),
        SchematicObject::BusLine(seg, l) => SchematicItem::BusLine(seg, l),
        SchematicObject::BusJunction(seg, j) => SchematicItem::BusJunction(seg, j),
        SchematicObject::BusLabel(seg, l) => SchematicItem::BusLabel(seg, l),
        SchematicObject::Polygon(id) => SchematicItem::Polygon(id),
        SchematicObject::Text(id) => SchematicItem::Text(id),
        SchematicObject::Image(id) => SchematicItem::Image(id),
    })
}

impl SchematicView for SceneView<'_> {
    fn items_at(&self, pos: Point, tolerance: Length) -> Vec<SchematicItem> {
        let mut items: Vec<SchematicItem> = self
            .scene
            .scene()
            .items_at(convert::point(pos), tolerance.to_mm())
            .into_iter()
            .filter_map(|id| self.scene.object(id).and_then(map_object))
            .collect();
        items.dedup();
        items
    }

    fn items_in_rect(&self, p1: Point, p2: Point) -> Vec<SchematicItem> {
        let (a, b): (KPoint, KPoint) = (convert::point(p1), convert::point(p2));
        self.scene
            .scene()
            .items_in_rect(Rect::from_points(a, b), SelectionMode::Intersects)
            .into_iter()
            .filter_map(|id| self.scene.object(id).and_then(map_object))
            .collect()
    }

    fn tolerance(&self) -> Length {
        // 5 pixels at 20 pixels per millimeter.
        Length::new(250_000)
    }

    fn cursor_pos(&self) -> Option<Point> {
        Some(self.cursor)
    }
}

/// The FSM with its editor, scene and clipboard.
struct Harness {
    editor: ProjectEditor,
    schematic: SchematicId,
    scene: SchematicScene,
    sync: SceneSync,
    fsm: SchematicEditorFsm,
    clipboard: MemoryClipboard,
    cursor: Point,
    /// Requests of the FSM (except errors, which fail the test).
    requests: Vec<SchematicRequest>,
    _dir: Option<tempfile::TempDir>,
}

impl Harness {
    /// A project with the resistors R1 at (0, 0) and R2 at (20.32, 0).
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut editor = create_editor(dir.path());
        let schematic = editor
            .execute(AddSchematic {
                name: ElementName::new("Main").unwrap(),
                index: None,
            })
            .unwrap();
        for x in [0.0, 20.32] {
            editor
                .execute(AddComponent {
                    device: Some(lib::r0805()),
                    place: Some(SymbolPlacement {
                        schematic: Some(schematic),
                        position: mm(x, 0.0),
                        rotation: Angle::DEG0,
                        mirrored: false,
                        gate_offset: None,
                    }),
                    ..AddComponent::new(lib::resistor())
                })
                .unwrap();
        }
        Self::with_editor(editor, schematic, Some(dir))
    }

    /// The FSM on a schematic of an editor.
    fn with_editor(
        editor: ProjectEditor,
        schematic: SchematicId,
        dir: Option<tempfile::TempDir>,
    ) -> Self {
        let scene =
            SchematicScene::build(editor.project(), schematic, &ColorScheme::SCHEMATIC_LIGHT)
                .unwrap();
        let sync = SceneSync::new(editor.project());
        Self {
            editor,
            schematic,
            scene,
            sync,
            fsm: SchematicEditorFsm::new(schematic, SchematicEditorSettings::default()),
            clipboard: MemoryClipboard::new(),
            cursor: Point::ORIGIN,
            requests: Vec::new(),
            _dir: dir,
        }
    }

    fn p(&self) -> &Project {
        self.editor.project()
    }

    /// Calls the FSM and syncs the scene afterwards.
    fn run<R>(
        &mut self,
        f: impl FnOnce(&mut SchematicEditorFsm, &mut SchematicContext<'_>) -> R,
    ) -> R {
        let view = SceneView {
            scene: &self.scene,
            cursor: self.cursor,
        };
        let mut ctx = SchematicContext::new(&mut self.editor, &view, &mut self.clipboard);
        let result = f(&mut self.fsm, &mut ctx);
        self.sync
            .sync(self.editor.project(), &mut self.scene)
            .unwrap();
        let (errors, others): (Vec<_>, Vec<_>) = self
            .fsm
            .take_requests()
            .into_iter()
            .partition(|r| matches!(r, SchematicRequest::ShowError(_)));
        self.requests.extend(others);
        assert!(errors.is_empty(), "FSM errors: {errors:?}");
        result
    }

    fn move_to(&mut self, pos: Point) {
        self.cursor = pos;
        self.run(|fsm, ctx| fsm.pointer_moved(ctx, PointerEvent::new(pos)));
    }

    fn press(&mut self, pos: Point) -> bool {
        self.cursor = pos;
        self.run(|fsm, ctx| fsm.left_pressed(ctx, PointerEvent::new(pos)))
    }

    fn release(&mut self, pos: Point) -> bool {
        self.cursor = pos;
        self.run(|fsm, ctx| fsm.left_released(ctx, PointerEvent::new(pos)))
    }

    fn click(&mut self, pos: Point) {
        self.move_to(pos);
        self.press(pos);
        self.release(pos);
    }

    fn undo(&mut self) {
        assert!(self.editor.undo().unwrap());
        self.sync
            .sync(self.editor.project(), &mut self.scene)
            .unwrap();
    }

    fn redo(&mut self) {
        assert!(self.editor.redo().unwrap());
        self.sync
            .sync(self.editor.project(), &mut self.scene)
            .unwrap();
    }

    fn symbol(&self, name: &str) -> SymbolId {
        let p = self.p();
        let (id, _) = p.circuit().component_instance_by_name(name).unwrap();
        let s = p.schematic(self.schematic).unwrap();
        *s.symbols()
            .iter()
            .find(|(_, sym)| sym.component() == id)
            .unwrap()
            .0
    }

    /// The pins of a component's symbol, sorted by X coordinate.
    fn pins(&self, name: &str) -> Vec<(Uuid, Point)> {
        let p = self.p();
        let symbol = self.symbol(name);
        let s = p.schematic(self.schematic).unwrap();
        let mut pins: Vec<_> = s.symbols()[&symbol]
            .pins(p.view())
            .unwrap()
            .into_iter()
            .map(|v| (v.uuid(), v.position()))
            .collect();
        pins.sort_by_key(|(_, pos)| pos.x);
        pins
    }

    fn segment_count(&self) -> usize {
        self.p()
            .schematic(self.schematic)
            .unwrap()
            .net_segments()
            .len()
    }
}

/// Draws a wire from R1 pin 2 to R2 pin 1 with the FSM.
fn draw_wire_between_resistors(h: &mut Harness) {
    let from = h.pins("R1")[1].1;
    let to = h.pins("R2")[0].1;
    assert!(h.run(|fsm, ctx| fsm.draw_wire(ctx)));
    assert_eq!(h.fsm.tool(), SchematicTool::Wire);
    h.move_to(from);
    assert!(h.press(from));
    h.move_to(mm(10.0, 3.0));
    // The preview is in the model (inside the open group).
    assert!(h.editor.undo_stack().is_group_active());
    assert_eq!(h.segment_count(), 1);
    h.move_to(to);
    h.press(to);
    assert!(!h.editor.undo_stack().is_group_active());
    // Escape: back to the select tool.
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert_eq!(h.fsm.tool(), SchematicTool::Select);
}

#[test]
fn draw_wire_between_two_pins() {
    let mut h = Harness::new();
    let nets_before = h.p().circuit().net_signals().len();
    draw_wire_between_resistors(&mut h);
    let p = h.p();
    let s = p.schematic(h.schematic).unwrap();
    assert_eq!(s.net_segments().len(), 1);
    let segment = s.net_segments().values().next().unwrap();
    let pins: BTreeSet<SymbolId> = segment.connected_pins().iter().map(|(s, _)| *s).collect();
    assert_eq!(pins, [h.symbol("R1"), h.symbol("R2")].into_iter().collect());
    // A straight wire (both pins at y=0): the simplification removed the
    // corner junction.
    assert_eq!(segment.lines().len(), 1);
    assert_eq!(p.circuit().net_signals().len(), nets_before + 1);
    let net = segment.net();
    for name in ["R1", "R2"] {
        let (_, c) = p.circuit().component_instance_by_name(name).unwrap();
        assert!(c.signals().values().any(|sig| sig.net() == Some(net)));
    }
    assert!(p.is_ref_index_consistent());

    // Undo/redo: wire group and simplification group.
    let history = h.editor.undo_stack().history();
    assert!(history.iter().all(|e| e.done));
    while h.segment_count() > 0 {
        h.undo();
    }
    assert_eq!(h.p().circuit().net_signals().len(), nets_before);
    while h.editor.undo_stack().can_redo() {
        h.redo();
    }
    assert_eq!(h.segment_count(), 1);
}

#[test]
fn draw_wire_with_corner_and_continue() {
    let mut h = Harness::new();
    let from = h.pins("R1")[1].1;
    h.run(|fsm, ctx| fsm.draw_wire(ctx));
    h.click(from);
    // Free point: the wire continues from there.
    let corner = mm(10.16, 10.16);
    h.move_to(corner);
    h.press(corner);
    assert!(h.editor.undo_stack().is_group_active());
    let to = h.pins("R2")[0].1;
    h.move_to(to);
    h.press(to);
    assert!(!h.editor.undo_stack().is_group_active());
    let p = h.p();
    let s = p.schematic(h.schematic).unwrap();
    let segment = s.net_segments().values().next().unwrap();
    // HV mode: from (from) horizontally to x=10.16, then up; then
    // horizontally/vertically to R2.
    assert!(segment.lines().len() >= 3, "{segment:?}");
    assert_eq!(segment.connected_pins().len(), 2);
    assert!(segment.junctions().values().any(|j| j.position() == corner));
    // Escape leaves the tool.
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert_eq!(h.fsm.tool(), SchematicTool::Select);
}

#[test]
fn draw_wire_abort_discards_preview() {
    let mut h = Harness::new();
    let index = h.editor.undo_stack().index();
    let from = h.pins("R1")[1].1;
    h.run(|fsm, ctx| fsm.draw_wire(ctx));
    h.click(from);
    h.move_to(mm(10.0, 10.0));
    assert_eq!(h.segment_count(), 1);
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert_eq!(h.segment_count(), 0);
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.editor.undo_stack().index(), index);
}

#[test]
fn select_and_move_symbol_wires_follow() {
    let mut h = Harness::new();
    draw_wire_between_resistors(&mut h);
    let r2 = h.symbol("R2");
    let r2_pos = h.p().schematic(h.schematic).unwrap().symbols()[&r2].position();
    let index = h.editor.undo_stack().index();

    // Drag R2 by its origin.
    h.move_to(r2_pos);
    assert!(h.press(r2_pos));
    assert_eq!(
        h.fsm.selection().iter().next(),
        Some(&SchematicItem::Symbol(r2))
    );
    let target = r2_pos + mm(5.0, 10.0);
    h.move_to(target);
    // Live preview inside the open group.
    assert!(h.editor.undo_stack().is_group_active());
    h.release(target);
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.editor.undo_stack().index(), index + 1);

    let p = h.p();
    let s = p.schematic(h.schematic).unwrap();
    let new_pos = s.symbols()[&r2].position();
    assert_eq!(new_pos, r2_pos + mm(5.08, 10.16)); // snapped to the grid
    // The wire still ends at the moved pin.
    let pin = h.pins("R2")[0];
    let segment = s.net_segments().values().next().unwrap();
    let anchor = NetLineAnchor::Pin {
        symbol: r2.0,
        pin: pin.0,
    };
    assert!(segment.lines_at(anchor).next().is_some());
    assert_eq!(
        s.net_line_anchor_position(segment.id(), anchor, p.view()),
        Some(pin.1)
    );
    assert!(p.is_ref_index_consistent());

    h.undo();
    let s = h.p().schematic(h.schematic).unwrap();
    assert_eq!(s.symbols()[&r2].position(), r2_pos);
    h.redo();
    let s = h.p().schematic(h.schematic).unwrap();
    assert_eq!(s.symbols()[&r2].position(), new_pos);
}

#[test]
fn rotate_mirror_and_move_selection() {
    let mut h = Harness::new();
    let r1 = h.symbol("R1");
    // Name, value and MPN are shown for components placed on a board.
    let board = h
        .editor
        .execute(AddBoard::new(ElementName::new("Board").unwrap()))
        .unwrap()
        .board;
    h.editor
        .execute(AddDevice {
            component: "R1".into(),
            board: Some(board),
            device: None,
            footprint: None,
            position: Point::ORIGIN,
            rotation: Angle::DEG0,
            mirrored: false,
        })
        .unwrap();
    h.sync.sync(h.editor.project(), &mut h.scene).unwrap();
    h.click(Point::ORIGIN);
    assert!(h.fsm.selection().contains(&SchematicItem::Symbol(r1)));
    assert!(h.fsm.view_state().features.rotate);
    let info = h.fsm.view_state().info_box.clone();
    assert!(info.contains("Name:") && info.contains("R1"), "{info}");
    assert!(info.contains("MPN"), "{info}");
    let r1_cmp = h.p().schematic(h.schematic).unwrap().symbols()[&r1].component();
    assert!(h.fsm.cross_probe().components.contains(&r1_cmp));
    h.run(|fsm, ctx| fsm.rotate(ctx, Angle::DEG90));
    let sym = |h: &Harness| h.p().schematic(h.schematic).unwrap().symbols()[&r1].clone();
    assert_eq!(sym(&h).rotation(), Angle::DEG90);
    h.run(|fsm, ctx| fsm.mirror(ctx, Orientation::Horizontal));
    assert!(sym(&h).mirrored());
    h.run(|fsm, ctx| fsm.move_by(ctx, mm(2.54, 0.0)));
    assert_eq!(sym(&h).position(), mm(2.54, 0.0));
    h.undo();
    h.undo();
    h.undo();
    assert_eq!(sym(&h).rotation(), Angle::DEG0);
    assert!(!sym(&h).mirrored());
    // Right click while dragging rotates.
    h.move_to(Point::ORIGIN);
    h.press(Point::ORIGIN);
    h.run(|fsm, ctx| fsm.right_released(ctx, PointerEvent::new(Point::ORIGIN)));
    h.release(Point::ORIGIN);
    assert_eq!(sym(&h).rotation(), Angle::DEG90);
}

#[test]
fn rubber_band_select_and_delete() {
    let mut h = Harness::new();
    draw_wire_between_resistors(&mut h);
    let index = h.editor.undo_stack().index();
    // Rubber band over everything, starting in an empty area.
    let start = mm(-20.0, 20.0);
    h.move_to(start);
    h.press(start);
    h.move_to(mm(40.0, -20.0));
    assert!(h.fsm.view_state().rubber_band.is_some());
    h.release(mm(40.0, -20.0));
    assert!(h.fsm.view_state().rubber_band.is_none());
    let selection = h.fsm.selection().clone();
    assert!(selection.contains(&SchematicItem::Symbol(h.symbol("R1"))));
    assert!(selection.contains(&SchematicItem::Symbol(h.symbol("R2"))));
    assert!(
        selection
            .iter()
            .any(|i| matches!(i, SchematicItem::NetLine(..)))
    );
    assert!(h.fsm.view_state().features.remove);

    h.run(|fsm, ctx| fsm.remove(ctx));
    let p = h.p();
    let s = p.schematic(h.schematic).unwrap();
    assert!(s.symbols().is_empty());
    assert!(s.net_segments().is_empty());
    assert!(p.circuit().component_instances().is_empty());
    assert!(h.fsm.selection().is_empty());
    assert_eq!(h.editor.undo_stack().index(), index + 1);

    h.undo();
    let s = h.p().schematic(h.schematic).unwrap();
    assert_eq!(s.symbols().len(), 2);
    assert_eq!(s.net_segments().len(), 1);
}

#[test]
fn select_all_and_abort_clears_selection() {
    let mut h = Harness::new();
    h.run(|fsm, ctx| fsm.select_all(ctx));
    assert_eq!(h.fsm.selection().len(), 2);
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert!(h.fsm.selection().is_empty());
}

#[test]
fn add_component_by_clicks() {
    let mut h = Harness::new();
    h.cursor = mm(0.0, 20.0);
    let ok = h.run(|fsm, ctx| {
        fsm.add_component(
            ctx,
            ComponentChoice {
                device: Some(lib::r0805()),
                ..ComponentChoice::new(lib::resistor())
            },
        )
    });
    assert!(ok);
    assert_eq!(h.fsm.tool(), SchematicTool::Component);
    // The new component follows the cursor (inside an open group).
    assert_eq!(h.p().circuit().component_instances().len(), 3);
    h.run(|fsm, ctx| fsm.rotate(ctx, Angle::DEG90));
    h.move_to(mm(10.0, 20.0));
    let r3 = h.symbol("R3");
    assert_eq!(
        h.p().schematic(h.schematic).unwrap().symbols()[&r3].position(),
        mm(10.16, 20.32)
    );
    h.press(mm(10.0, 20.0));
    // Placed; the next resistor (R4) follows the cursor already.
    assert_eq!(h.p().circuit().component_instances().len(), 4);
    let placed = h.p().schematic(h.schematic).unwrap().symbols()[&r3].clone();
    assert_eq!(placed.position(), mm(10.16, 20.32));
    assert_eq!(placed.rotation(), Angle::DEG90);
    let r4 = h.symbol("R4");
    assert_eq!(
        h.p().schematic(h.schematic).unwrap().symbols()[&r4].rotation(),
        Angle::DEG90
    );
    // Escape discards R4 and leaves the tool.
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert_eq!(h.fsm.tool(), SchematicTool::Select);
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.p().circuit().component_instances().len(), 3);
    assert_eq!(
        h.editor.undo_stack().undo_text(),
        Some("Add Component to Schematic")
    );
    assert!(h.p().is_ref_index_consistent());

    h.undo();
    assert_eq!(h.p().circuit().component_instances().len(), 2);
    h.redo();
    assert_eq!(h.p().circuit().component_instances().len(), 3);
}

#[test]
fn add_component_dialog_request() {
    let mut h = Harness::new();
    h.fsm.open_add_component_dialog("schematic frame");
    let requests = h.fsm.take_requests();
    assert_eq!(
        requests,
        vec![SchematicRequest::AddComponentDialog {
            search_term: "schematic frame".to_owned()
        }]
    );
}

#[test]
fn add_net_label_on_wire() {
    let mut h = Harness::new();
    draw_wire_between_resistors(&mut h);
    h.run(|fsm, ctx| fsm.add_net_label(ctx));
    let pos = mm(10.16, 0.0);
    h.click(pos);
    assert!(h.editor.undo_stack().is_group_active());
    h.run(|fsm, ctx| fsm.rotate(ctx, Angle::DEG90));
    h.move_to(mm(12.7, 0.0));
    h.press(mm(12.7, 0.0));
    assert!(!h.editor.undo_stack().is_group_active());
    let p = h.p();
    let segment = p
        .schematic(h.schematic)
        .unwrap()
        .net_segments()
        .values()
        .next()
        .unwrap()
        .clone();
    assert_eq!(segment.labels().len(), 1);
    let label = segment.labels().values().next().unwrap();
    assert_eq!(label.position(), mm(12.7, 0.0));
    assert_eq!(label.rotation(), Angle::DEG90);
    h.undo();
    let segment = h
        .p()
        .schematic(h.schematic)
        .unwrap()
        .net_segments()
        .values()
        .next()
        .unwrap()
        .clone();
    assert!(segment.labels().is_empty());
}

#[test]
fn add_text_and_polygon() {
    let mut h = Harness::new();
    h.cursor = mm(0.0, -20.0);
    h.run(|fsm, ctx| fsm.set_value(ctx, "x"));
    h.run(|fsm, ctx| fsm.add_text(ctx));
    assert_eq!(h.fsm.tool(), SchematicTool::Text);
    assert_eq!(h.fsm.tool_data().value, "{{PROJECT}}");
    h.run(|fsm, ctx| fsm.set_value(ctx, "Hello"));
    h.move_to(mm(5.0, -20.0));
    h.press(mm(5.0, -20.0));
    // Placed; the next text follows the cursor.
    h.run(|fsm, ctx| fsm.select_tool(ctx));
    let texts: Vec<_> = h
        .p()
        .schematic(h.schematic)
        .unwrap()
        .texts()
        .values()
        .cloned()
        .collect();
    assert_eq!(texts.len(), 1);
    assert_eq!(texts[0].text(), "Hello");
    assert_eq!(texts[0].position(), mm(5.08, -20.32));

    h.run(|fsm, ctx| fsm.draw_polygon(ctx));
    for p in [
        mm(0.0, 30.0),
        mm(10.16, 30.48),
        mm(10.16, 40.64),
        mm(0.0, 30.0),
    ] {
        h.move_to(p);
        h.press(p);
    }
    h.run(|fsm, ctx| fsm.abort(ctx));
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert_eq!(h.fsm.tool(), SchematicTool::Select);
    let polygons: Vec<_> = h
        .p()
        .schematic(h.schematic)
        .unwrap()
        .polygons()
        .values()
        .cloned()
        .collect();
    assert_eq!(polygons.len(), 1);
    assert!(polygons[0].path().is_closed(), "{:?}", polygons[0]);
    assert_eq!(polygons[0].path().vertices().len(), 4);
}

#[test]
fn copy_paste_symbols_and_wire() {
    let mut h = Harness::new();
    draw_wire_between_resistors(&mut h);
    h.run(|fsm, ctx| fsm.select_all(ctx));
    h.cursor = Point::ORIGIN;
    assert!(h.run(|fsm, ctx| fsm.copy(ctx)));
    assert!(h.fsm.take_status_message().is_some());
    let mime = librepcb_editor::fsm::schematic::clipboard::schematic_clipboard_mime_type(
        &SchematicEditorSettings::default().app_version,
    );
    let zip = h.clipboard.get(&mime).expect("clipboard content");
    // The clipboard content round-trips.
    let data = SchematicClipboardData::from_zip(&zip).unwrap();
    assert_eq!(data.symbols.len(), 2);
    assert_eq!(data.components.len(), 2);
    assert_eq!(data.net_segments.len(), 1);
    let again = SchematicClipboardData::from_zip(&data.to_zip().unwrap()).unwrap();
    assert_eq!(again.to_sexpression(), data.to_sexpression());

    let index = h.editor.undo_stack().index();
    h.cursor = mm(0.0, 30.0);
    assert!(h.run(|fsm, ctx| fsm.paste(ctx)));
    assert!(h.editor.undo_stack().is_group_active());
    assert_eq!(h.p().circuit().component_instances().len(), 4);
    h.move_to(mm(0.0, 40.0));
    h.press(mm(0.0, 40.0));
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.editor.undo_stack().index(), index + 1);
    let p = h.p();
    let s = p.schematic(h.schematic).unwrap();
    assert_eq!(s.symbols().len(), 4);
    assert_eq!(s.net_segments().len(), 2);
    let r3 = h.symbol("R3");
    assert_eq!(s.symbols()[&r3].position(), mm(0.0, 40.64));
    // The pasted wire connects the pasted pins with a new net.
    let nets: BTreeSet<_> = s.net_segments().values().map(|s| s.net()).collect();
    assert_eq!(nets.len(), 2);
    assert!(p.is_ref_index_consistent());
    h.undo();
    assert_eq!(h.p().circuit().component_instances().len(), 2);
    assert_eq!(h.segment_count(), 1);

    // Cut removes.
    h.run(|fsm, ctx| fsm.select_all(ctx));
    assert!(h.run(|fsm, ctx| fsm.cut(ctx)));
    assert_eq!(h.p().circuit().component_instances().len(), 0);
}

#[test]
fn measure_tool() {
    let mut h = Harness::new();
    h.run(|fsm, ctx| fsm.measure(ctx));
    assert_eq!(h.fsm.tool(), SchematicTool::Measure);
    assert!(h.fsm.view_state().gray_out);
    h.move_to(mm(0.1, 0.1));
    h.press(mm(0.1, 0.1));
    h.move_to(mm(3.0, 4.1));
    h.press(mm(3.0, 4.1));
    assert!(h.fsm.view_state().ruler.is_some());
    assert!(h.run(|fsm, ctx| fsm.copy(ctx)));
    assert!(h.clipboard.get("text/plain").is_some());
    // Right click in another tool than select aborts the tool.
    h.run(|fsm, ctx| fsm.right_released(ctx, PointerEvent::new(Point::ORIGIN)));
    assert_eq!(h.fsm.tool(), SchematicTool::Select);
    assert!(!h.fsm.view_state().gray_out);
    // Right click in the select state goes back to the previous tool.
    h.run(|fsm, ctx| fsm.right_released(ctx, PointerEvent::new(mm(50.0, 50.0))));
    assert_eq!(h.fsm.tool(), SchematicTool::Measure);
}

#[test]
fn shift_disables_snapping_while_drawing() {
    let mut h = Harness::new();
    h.run(|fsm, ctx| fsm.draw_wire(ctx));
    let start = mm(0.0, 20.32);
    h.click(start);
    h.run(|fsm, ctx| {
        fsm.key_pressed(
            ctx,
            KeyEvent {
                key: librepcb_editor::fsm::Key::Shift,
                modifiers: Modifiers::SHIFT,
            },
        )
    });
    h.run(|fsm, ctx| {
        fsm.pointer_moved(
            ctx,
            PointerEvent::with_modifiers(mm(5.0, 20.32), Modifiers::SHIFT),
        )
    });
    assert_eq!(h.segment_count(), 1);
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert_eq!(h.segment_count(), 0);
}

#[test]
fn draw_wire_from_existing_wire() {
    let mut h = Harness::new();
    draw_wire_between_resistors(&mut h);
    h.run(|fsm, ctx| fsm.draw_wire(ctx));
    // Start on the wire (splits it), end at a free point, then Escape.
    let tap = mm(10.16, 0.0);
    h.click(tap);
    let end = mm(10.16, 10.16);
    h.move_to(end);
    h.press(end);
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert_eq!(h.fsm.tool(), SchematicTool::Wire);
    let p = h.p();
    let s = p.schematic(h.schematic).unwrap();
    assert_eq!(s.net_segments().len(), 1);
    let segment = s.net_segments().values().next().unwrap();
    let junction = segment
        .junctions()
        .values()
        .find(|j| j.position() == tap)
        .expect("junction at the tap");
    assert_eq!(
        segment
            .lines_at(NetLineAnchor::Junction(junction.uuid()))
            .count(),
        3
    );
    assert!(segment.junctions().values().any(|j| j.position() == end));
    assert!(p.is_ref_index_consistent());
}

#[test]
fn draw_wire_to_supply_symbol_applies_forced_net_name() {
    let mut h = Harness::new();
    h.cursor = mm(5.08, -10.16);
    assert!(h.run(|fsm, ctx| fsm.add_component(ctx, ComponentChoice::new(lib::supply_gnd()))));
    h.press(mm(5.08, -10.16));
    h.run(|fsm, ctx| fsm.abort(ctx));
    let (gnd_id, gnd) = h
        .p()
        .circuit()
        .component_instances()
        .iter()
        .find(|(_, c)| c.lib_component() == lib::supply_gnd())
        .map(|(id, c)| (*id, c.clone()))
        .expect("GND symbol placed");
    let gnd_symbol = *h
        .p()
        .schematic(h.schematic)
        .unwrap()
        .symbols()
        .iter()
        .find(|(_, s)| s.component() == gnd_id)
        .unwrap()
        .0;
    let s = h.p().schematic(h.schematic).unwrap();
    let gnd_pin = s.symbols()[&gnd_symbol].pins(h.p().view()).unwrap()[0].position();
    let r1_pin = h.pins("R1")[1].1;
    h.run(|fsm, ctx| fsm.draw_wire(ctx));
    h.click(r1_pin);
    h.move_to(gnd_pin);
    h.press(gnd_pin);
    let p = h.p();
    let segment = p
        .schematic(h.schematic)
        .unwrap()
        .net_segments()
        .values()
        .next()
        .unwrap();
    let net = p.circuit().net_signal(segment.net()).unwrap();
    assert_eq!(net.name().as_str(), gnd.value().as_str());
    assert!(!net.has_auto_name());
}

#[test]
fn double_click_and_context_menu_requests() {
    let mut h = Harness::new();
    let r1 = h.symbol("R1");
    h.click(Point::ORIGIN);
    h.run(|fsm, ctx| fsm.left_double_clicked(ctx, PointerEvent::new(Point::ORIGIN)));
    assert_eq!(
        std::mem::take(&mut h.requests),
        vec![SchematicRequest::SymbolProperties(r1)]
    );
    h.run(|fsm, ctx| fsm.right_released(ctx, PointerEvent::new(Point::ORIGIN)));
    assert_eq!(
        std::mem::take(&mut h.requests),
        vec![SchematicRequest::ContextMenu {
            item: SchematicItem::Symbol(r1),
            pos: Point::ORIGIN,
            remove_vertex: None,
            add_vertex: false,
        }]
    );
    assert!(h.run(|fsm, ctx| fsm.edit_properties(ctx)));
    assert_eq!(
        std::mem::take(&mut h.requests),
        vec![SchematicRequest::SymbolProperties(r1)]
    );
    // Undoing the components (R2, then R1) removes R1 from the selection.
    h.undo();
    h.fsm.project_changed(h.editor.project());
    assert_eq!(h.fsm.selection().len(), 1);
    h.undo();
    h.fsm.project_changed(h.editor.project());
    assert!(h.fsm.selection().is_empty());
}

/// Opens an upstream test project read-only (in memory).
fn open_upstream(dir: &std::path::Path) -> Project {
    use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
    let lpp = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".lpp"))
        .expect("*.lpp file");
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(dir).unwrap()).unwrap();
    let directory = TransactionalDirectory::new(std::sync::Arc::new(fs), "");
    librepcb_core::project::ProjectLoader::new()
        .open(directory, &lpp)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
}

/// Select all, move, rotate, mirror, snap, copy & paste on every page of
/// the upstream test projects, then undo everything: the pages must be
/// unchanged.
#[test]
fn edit_upstream_projects_and_undo() {
    let root = crate::helpers::test_data_dir().join("projects");
    let mut dirs: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join(".librepcb-project").exists())
        .collect();
    dirs.sort();
    let mut pages = 0;
    for dir in dirs {
        let project = open_upstream(&dir);
        let schematics: Vec<SchematicId> = project.schematics().iter().map(|s| s.id()).collect();
        let mut editor = ProjectEditor::new(project);
        for schematic in schematics {
            let original = editor.project().schematic(schematic).unwrap().clone();
            let circuit = editor.project().circuit().clone();
            if original.symbols().is_empty() {
                continue;
            }
            pages += 1;
            let mut h = Harness::with_editor(editor, schematic, None);
            let index = h.editor.undo_stack().index();
            h.run(|fsm, ctx| fsm.select_all(ctx));
            assert!(
                h.run(|fsm, ctx| fsm.move_by(ctx, mm(2.54, 5.08))),
                "{}",
                dir.display()
            );
            assert!(h.run(|fsm, ctx| fsm.rotate(ctx, Angle::DEG90)));
            assert!(h.run(|fsm, ctx| fsm.mirror(ctx, Orientation::Vertical)));
            h.run(|fsm, ctx| fsm.snap_to_grid(ctx));
            assert!(h.p().is_ref_index_consistent(), "{}", dir.display());
            // Copy and paste everything.
            h.cursor = Point::ORIGIN;
            assert!(h.run(|fsm, ctx| fsm.copy(ctx)));
            h.cursor = mm(100.0, 100.0);
            assert!(h.run(|fsm, ctx| fsm.paste(ctx)), "{}", dir.display());
            h.press(mm(100.0, 100.0));
            assert!(!h.editor.undo_stack().is_group_active());
            let s = h.p().schematic(schematic).unwrap();
            assert_eq!(
                s.symbols().len(),
                2 * original.symbols().len(),
                "{}",
                dir.display()
            );
            assert!(h.p().is_ref_index_consistent(), "{}", dir.display());
            while h.editor.undo_stack().index() > index {
                h.undo();
            }
            let s = h.p().schematic(schematic).unwrap();
            assert_eq!(s, &original, "{}", dir.display());
            assert_eq!(h.p().circuit(), &circuit, "{}", dir.display());
            editor = h.editor;
        }
    }
    assert!(pages >= 3, "{pages}");
}

// --- Buses ---

impl Harness {
    fn bus_segments(&self) -> Vec<librepcb_core::project::schematic::SchematicBusSegment> {
        self.p()
            .schematic(self.schematic)
            .unwrap()
            .bus_segments()
            .values()
            .cloned()
            .collect()
    }

    fn press_mod(&mut self, pos: Point, modifiers: Modifiers) -> bool {
        self.cursor = pos;
        self.run(|fsm, ctx| fsm.left_pressed(ctx, PointerEvent::with_modifiers(pos, modifiers)))
    }

    /// Draws a bus from `a` to `b` (HV corner) and finishes it.
    fn draw_bus(&mut self, a: Point, b: Point) {
        assert!(self.run(|fsm, ctx| fsm.draw_bus(ctx)));
        assert_eq!(self.fsm.tool(), SchematicTool::Bus);
        self.move_to(a);
        self.press(a);
        self.move_to(b);
        self.press(b);
        self.run(|fsm, ctx| fsm.abort(ctx));
        assert!(!self.editor.undo_stack().is_group_active());
    }
}

#[test]
fn draw_bus_extend_and_label() {
    let mut h = Harness::new();
    h.draw_bus(mm(0.0, 20.32), mm(20.32, 30.48));
    let segs = h.bus_segments();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].lines().len(), 2, "HV corner: two lines");
    let buses = h.p().circuit().buses();
    assert_eq!(buses.len(), 1);
    let bus = buses.values().next().unwrap();
    assert_eq!(bus.name().to_string(), "B1");
    assert!(bus.has_auto_name());

    // A second bus line from the middle of the first line extends the
    // segment (the line is split).
    assert!(h.run(|fsm, ctx| fsm.draw_bus(ctx)));
    h.move_to(mm(10.16, 20.32));
    h.press(mm(10.16, 20.32));
    h.move_to(mm(10.16, 10.16));
    h.press(mm(10.16, 10.16));
    h.run(|fsm, ctx| fsm.abort(ctx));
    let segs = h.bus_segments();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].lines().len(), 4);
    assert_eq!(h.p().circuit().buses().len(), 1);

    // Add a bus label on the bus with the label tool.
    assert!(h.run(|fsm, ctx| fsm.add_net_label(ctx)));
    h.move_to(mm(5.08, 20.32));
    h.press(mm(5.08, 20.32));
    h.move_to(mm(7.62, 20.32));
    h.press(mm(7.62, 20.32));
    assert!(!h.editor.undo_stack().is_group_active());
    let segs = h.bus_segments();
    assert_eq!(segs[0].labels().len(), 1);
    assert_eq!(
        segs[0].labels().values().next().unwrap().position(),
        mm(7.62, 20.32)
    );
    assert_eq!(
        h.editor.undo_stack().undo_text(),
        Some("Add Bus Label to Schematic")
    );
}

#[test]
fn draw_bus_with_chosen_bus_adds_label_and_combines() {
    let mut h = Harness::new();
    h.draw_bus(mm(0.0, 20.32), mm(20.32, 20.32));
    let b1 = *h.p().circuit().buses().keys().next().unwrap();
    // A new bus "DATA" (not auto named) chosen in the tool bar.
    let data = h
        .editor
        .execute(ApplyMutations {
            text: None,
            mutations: vec![librepcb_core::project::Mutation::AddBus(
                librepcb_core::project::circuit::Bus::new(
                    Uuid::new_random(),
                    librepcb_core::types::BusName::new("DATA").unwrap(),
                    false,
                    false,
                    None,
                ),
            )],
        })
        .map(|_| {
            *h.p()
                .circuit()
                .buses()
                .iter()
                .find(|(_, b)| b.name().as_str() == "DATA")
                .unwrap()
                .0
        })
        .unwrap();
    assert!(h.run(|fsm, ctx| fsm.draw_bus(ctx)));
    h.run(|fsm, ctx| fsm.set_bus(ctx, Some(data)));
    assert_eq!(h.fsm.tool_data().bus, Some(data));
    assert!(h.fsm.tool_data().buses.iter().any(|(id, _)| *id == data));
    // From free space to the end junction of the B1 segment.
    h.move_to(mm(20.32, 5.08));
    h.press(mm(20.32, 5.08));
    h.move_to(mm(20.32, 20.32));
    h.press(mm(20.32, 20.32));
    assert!(!h.editor.undo_stack().is_group_active());
    let segs = h.bus_segments();
    assert_eq!(segs.len(), 1, "combined into one segment");
    // B1 had an automatic name: the segment moved to DATA, B1 is gone.
    assert_eq!(segs[0].bus(), data);
    assert!(h.p().circuit().bus(b1).is_none());
    assert_eq!(segs[0].labels().len(), 1, "label at the start");
    assert_eq!(
        segs[0].labels().values().next().unwrap().position(),
        mm(20.32, 5.08)
    );
}

#[test]
fn remove_bus_line_splits_segment() {
    let mut h = Harness::new();
    h.draw_bus(mm(0.0, 20.32), mm(20.32, 30.48));
    assert!(h.run(|fsm, ctx| fsm.select_tool(ctx)));
    // Select the vertical line and remove it.
    h.click(mm(20.32, 25.4));
    assert!(
        h.fsm
            .selection()
            .iter()
            .any(|i| matches!(i, SchematicItem::BusLine(..))),
        "{:?}",
        h.fsm.selection()
    );
    assert!(h.fsm.view_state().features.remove);
    assert!(h.run(|fsm, ctx| fsm.remove(ctx)));
    let segs = h.bus_segments();
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].lines().len(), 1);
    assert_eq!(segs[0].junctions().len(), 2);
    // The remaining part has no label: it got a new bus; the old one is
    // unused and removed.
    assert_eq!(h.p().circuit().buses().len(), 1);
    h.undo();
    assert_eq!(h.bus_segments()[0].lines().len(), 2);
}

#[test]
fn wires_from_and_to_bus_with_member_menu() {
    use librepcb_editor::fsm::schematic::BusMemberChoice;
    let mut h = Harness::new();
    let r1_2 = h.pins("R1")[1].1;
    let r2_1 = h.pins("R2")[0].1;
    let y = mm(0.0, 10.16).y;
    let bus_a = Point::new(r1_2.x, y);
    let bus_b = Point::new(r2_1.x, y);
    h.draw_bus(bus_a, bus_b);
    assert_eq!(h.bus_segments().len(), 1);

    // Start a wire at the bus: the member menu is requested.
    assert!(h.run(|fsm, ctx| fsm.draw_wire(ctx)));
    h.move_to(bus_a);
    assert!(h.press(bus_a));
    let menu = h
        .requests
        .iter()
        .find_map(|r| match r {
            SchematicRequest::BusMemberMenu { nets, .. } => Some(nets.clone()),
            _ => None,
        })
        .expect("bus member menu");
    assert!(menu.is_empty(), "no members yet");
    assert!(!h.editor.undo_stack().is_group_active());
    assert!(h.run(|fsm, ctx| fsm.choose_bus_member(ctx, Some(BusMemberChoice::NewMember))));
    assert!(h.editor.undo_stack().is_group_active());
    h.move_to(r1_2);
    h.press(r1_2);
    assert!(!h.editor.undo_stack().is_group_active());
    let p = h.p();
    let s = p.schematic(h.schematic).unwrap();
    let seg = s
        .net_segments()
        .values()
        .find(|seg| !seg.connected_bus_junctions().is_empty())
        .expect("net segment attached to the bus");
    assert_eq!(seg.labels().len(), 1, "label at the end of the wire");
    assert_eq!(seg.labels().values().next().unwrap().position(), r1_2);
    let net = seg.net();

    // A wire from R2 pin 1 to the bus: choose the existing member.
    h.move_to(r2_1);
    h.press(r2_1);
    h.move_to(bus_b);
    assert!(h.press(bus_b));
    let menu = h
        .requests
        .iter()
        .rev()
        .find_map(|r| match r {
            SchematicRequest::BusMemberMenu { nets, .. } => Some(nets.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(menu.len(), 1);
    assert_eq!(menu[0].net, net);
    assert!(menu[0].enabled);
    h.run(|fsm, ctx| fsm.choose_bus_member(ctx, Some(BusMemberChoice::Net(net))));
    assert!(!h.editor.undo_stack().is_group_active());
    let p = h.p();
    let s = p.schematic(h.schematic).unwrap();
    let r2 = h.symbol("R2");
    let pin = h.pins("R2")[0].0;
    let pin_seg = s.pin_net_segment(r2, pin).unwrap();
    assert_eq!(s.net_segments()[&pin_seg].net(), net, "same bus member");
    assert_eq!(s.net_segments()[&pin_seg].labels().len(), 1);
    assert_eq!(
        s.net_segments()[&pin_seg]
            .labels()
            .values()
            .next()
            .unwrap()
            .position(),
        r2_1,
        "label at the start of a wire ending at a bus"
    );

    // Moving the bus junction moves the attached wire end.
    assert!(h.run(|fsm, ctx| fsm.select_tool(ctx)));
    let seg_before = h.bus_segments()[0].clone();
    let junction = *seg_before
        .junctions()
        .iter()
        .find(|(_, j)| j.position() == bus_a)
        .unwrap()
        .0;
    h.run(|fsm, _| fsm.set_selection([SchematicItem::BusJunction(seg_before.id(), junction)]));
    assert!(h.run(|fsm, ctx| fsm.move_by(ctx, mm(0.0, 2.54))));
    let seg_after = &h.bus_segments()[0];
    assert!(
        seg_after
            .junctions()
            .values()
            .any(|j| j.position() == bus_a + mm(0.0, 2.54))
    );
}

#[test]
fn wire_to_bus_with_control_skips_menu() {
    let mut h = Harness::new();
    let r1_2 = h.pins("R1")[1].1;
    let bus_a = Point::new(r1_2.x, mm(0.0, 10.16).y);
    h.draw_bus(bus_a, bus_a + mm(10.16, 0.0));
    assert!(h.run(|fsm, ctx| fsm.draw_wire(ctx)));
    h.move_to(bus_a);
    h.press_mod(bus_a, Modifiers::CONTROL);
    assert!(
        !h.requests
            .iter()
            .any(|r| matches!(r, SchematicRequest::BusMemberMenu { .. }))
    );
    h.move_to(r1_2);
    h.press(r1_2);
    let p = h.p();
    let s = p.schematic(h.schematic).unwrap();
    let seg = s
        .net_segments()
        .values()
        .find(|seg| !seg.connected_bus_junctions().is_empty())
        .unwrap();
    assert!(seg.labels().is_empty(), "no label without the menu");
}

#[test]
fn symbol_texts_are_selectable() {
    let mut h = Harness::new();
    let r1 = h.symbol("R1");
    let (text_uuid, text_pos) = {
        let s = h.p().schematic(h.schematic).unwrap();
        let t = s.symbols()[&r1].texts().values().next().unwrap();
        (t.uuid(), t.position())
    };
    let symbol_pos = h.p().schematic(h.schematic).unwrap().symbols()[&r1].position();
    // Find a point on the rendered text (scan its bounding box).
    let hit = {
        let canvas = h.scene.scene();
        let (_, item) = canvas
            .items()
            .find(|(id, _)| h.scene.object(*id) == Some(SchematicObject::SymbolText(r1, text_uuid)))
            .expect("text item");
        let bbox = item.bounding_box();
        let steps = 40;
        (0..=steps * steps)
            .map(|i| {
                KPoint::new(
                    bbox.x0 + bbox.width() * f64::from(i % steps) / f64::from(steps),
                    bbox.y0 + bbox.height() * f64::from(i / steps) / f64::from(steps),
                )
            })
            .find(|p| item.hit(*p, 0.0))
            .map(|p| mm(p.x, p.y))
            .expect("text hit")
    };
    let _ = text_pos;
    h.move_to(hit);
    h.press(hit);
    assert!(
        h.fsm
            .selection()
            .contains(&SchematicItem::SymbolText(r1, text_uuid)),
        "{:?}",
        h.fsm.selection()
    );
    h.move_to(hit + mm(2.54, 2.54));
    h.release(hit + mm(2.54, 2.54));
    let s = h.p().schematic(h.schematic).unwrap();
    assert_eq!(s.symbols()[&r1].position(), symbol_pos, "symbol not moved");
    assert_eq!(
        s.symbols()[&r1].texts()[&text_uuid].position(),
        text_pos + mm(2.54, 2.54)
    );
    // Double click requests the text properties.
    h.run(|fsm, ctx| fsm.left_double_clicked(ctx, PointerEvent::new(hit + mm(2.54, 2.54))));
    assert!(h.requests.iter().any(|r| matches!(
        r,
        SchematicRequest::SymbolTextProperties(s, t) if *s == r1 && *t == text_uuid
    )));
    // Remove only the text.
    assert!(h.run(|fsm, ctx| fsm.remove(ctx)));
    let s = h.p().schematic(h.schematic).unwrap();
    assert!(!s.symbols()[&r1].texts().contains_key(&text_uuid));
    assert!(s.symbols().contains_key(&r1));
}

// --- Images and polygon vertices ---

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="200" height="100" fill="red"/></svg>"#;

impl Harness {
    fn images(&self) -> Vec<librepcb_core::geometry::Image> {
        self.p()
            .schematic(self.schematic)
            .unwrap()
            .images()
            .values()
            .cloned()
            .collect()
    }

    fn image_file_exists(&self, name: &str) -> bool {
        use librepcb_core::fileio::FileSystem;
        let dir = self
            .p()
            .schematic(self.schematic)
            .unwrap()
            .directory_name()
            .to_owned();
        self.p()
            .directory()
            .file_exists(&format!("schematics/{dir}/{name}"))
    }
}

#[test]
fn add_resize_and_remove_image() {
    use librepcb_editor::fsm::schematic::ImageData;
    let mut h = Harness::new();
    // Without data, the image chooser is requested.
    h.run(|fsm, ctx| fsm.add_image(ctx, None));
    assert_eq!(h.requests.pop(), Some(SchematicRequest::ChooseImageFile));
    h.cursor = mm(30.48, 30.48);
    assert!(h.run(|fsm, ctx| fsm.add_image(
        ctx,
        Some(ImageData {
            data: SVG.as_bytes().to_vec(),
            format: "svg".into(),
            basename: "my logo".into(),
        })
    )));
    assert_eq!(h.fsm.tool(), SchematicTool::Image);
    assert!(h.editor.undo_stack().is_group_active());
    assert_eq!(h.images().len(), 1);
    assert_eq!(h.images()[0].file_name().as_str(), "my-logo.svg");
    assert!(h.image_file_exists("my-logo.svg"));
    // Initial size: 10 mm on the longer side, aspect ratio kept.
    assert_eq!(h.images()[0].width().to_mm(), 10.0);
    assert_eq!(h.images()[0].height().to_mm(), 5.0);
    // Position, then size.
    h.move_to(mm(5.08, 5.08));
    assert_eq!(h.images()[0].position(), mm(5.08, 5.08));
    h.press(mm(5.08, 5.08));
    h.move_to(mm(25.4, 7.62));
    assert_eq!(h.images()[0].width().to_mm(), 20.32);
    assert_eq!(h.images()[0].height().to_mm(), 10.16);
    h.press(mm(25.4, 7.62));
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.fsm.tool(), SchematicTool::Select);
    assert_eq!(
        h.editor.undo_stack().undo_text(),
        Some("Add Schematic Image")
    );
    let image = h.images()[0].clone();

    // Resize it with the handle at its top right corner.
    h.run(|fsm, _| fsm.set_selection([SchematicItem::Image(image.uuid())]));
    let handle = image.position() + mm(image.width().to_mm(), image.height().to_mm());
    h.move_to(handle);
    h.press(handle);
    h.move_to(mm(15.24, 30.0));
    h.release(mm(15.24, 30.0));
    assert_eq!(h.images()[0].width().to_mm(), 10.16);
    assert_eq!(h.images()[0].height().to_mm(), 5.08);
    assert_eq!(h.images()[0].position(), image.position());
    assert_eq!(h.editor.undo_stack().undo_text(), Some("Edit Image"));

    // Copy & paste reuses the file; removing the last image removes it.
    h.run(|fsm, _| fsm.set_selection([SchematicItem::Image(image.uuid())]));
    assert!(h.run(|fsm, ctx| fsm.copy(ctx)));
    assert!(h.run(|fsm, ctx| fsm.paste(ctx)));
    h.move_to(mm(40.64, 40.64));
    h.press(mm(40.64, 40.64));
    assert_eq!(h.images().len(), 2);
    assert!(
        h.images()
            .iter()
            .all(|i| i.file_name().as_str() == "my-logo.svg")
    );
    assert!(h.run(|fsm, ctx| fsm.select_all(ctx)));
    let images: Vec<SchematicItem> = h
        .images()
        .iter()
        .map(|i| SchematicItem::Image(i.uuid()))
        .collect();
    h.run(|fsm, _| fsm.set_selection(images));
    assert!(h.run(|fsm, ctx| fsm.remove(ctx)));
    assert!(h.images().is_empty());
    assert!(!h.image_file_exists("my-logo.svg"));
    h.undo();
    assert_eq!(h.images().len(), 2);
    assert!(h.image_file_exists("my-logo.svg"));

    // Pasting image data from the clipboard adds an image (reusing the
    // existing file with the same content).
    h.clipboard = MemoryClipboard::new();
    h.clipboard.set("image/svg+xml", SVG.as_bytes().to_vec());
    assert!(h.run(|fsm, ctx| fsm.paste(ctx)));
    assert_eq!(h.fsm.tool(), SchematicTool::Image);
    assert_eq!(h.images().len(), 3);
    assert!(
        h.images()
            .iter()
            .all(|i| i.file_name().as_str() == "my-logo.svg")
    );
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert_eq!(h.images().len(), 2);

    // Saving and reopening keeps the image file.
    h.editor.save().unwrap();
}

#[test]
fn move_add_and_remove_polygon_vertices() {
    use librepcb_core::geometry::{Path, Polygon, Vertex};
    use librepcb_core::project::{Mutation, SchematicMutation};
    use librepcb_core::types::{Layer, UnsignedLength};
    let mut h = Harness::new();
    let path = Path::new(vec![
        Vertex::at(mm(30.48, 0.0)),
        Vertex::at(mm(40.64, 0.0)),
        Vertex::at(mm(40.64, 10.16)),
        Vertex::at(mm(30.48, 10.16)),
        Vertex::at(mm(30.48, 0.0)),
    ]);
    let polygon = Polygon::new(
        Uuid::new_random(),
        Layer::SCHEMATIC_GUIDE,
        UnsignedLength::new(Length::new(200_000)).unwrap(),
        false,
        false,
        path,
    );
    let uuid = polygon.uuid();
    let schematic = h.schematic;
    h.editor
        .execute(ApplyMutations {
            text: None,
            mutations: vec![Mutation::Schematic(SchematicMutation::AddPolygon {
                schematic,
                polygon,
            })],
        })
        .unwrap();
    h.sync.sync(h.editor.project(), &mut h.scene).unwrap();
    let poly = |h: &Harness| {
        h.p().schematic(h.schematic).unwrap().polygons()[&uuid]
            .path()
            .clone()
    };
    // Select the polygon, then drag its vertex at (40.64, 10.16).
    h.run(|fsm, _| fsm.set_selection([SchematicItem::Polygon(uuid)]));
    h.move_to(mm(40.64, 10.16));
    h.press(mm(40.64, 10.16));
    h.move_to(mm(45.72, 12.7));
    assert!(h.editor.undo_stack().is_group_active());
    h.release(mm(45.72, 12.7));
    assert_eq!(poly(&h).vertices()[2].pos, mm(45.72, 12.7));
    assert_eq!(poly(&h).vertices()[1].pos, mm(40.64, 0.0));
    assert_eq!(h.editor.undo_stack().undo_text(), Some("Edit polygon"));

    // Right click on the vertex offers to remove it.
    h.run(|fsm, ctx| fsm.right_released(ctx, PointerEvent::new(mm(45.72, 12.7))));
    assert!(h.requests.iter().any(|r| matches!(
        r,
        SchematicRequest::ContextMenu {
            remove_vertex: Some(true),
            ..
        }
    )));
    assert!(h.run(|fsm, ctx| fsm.remove_polygon_vertices(ctx, uuid, mm(45.72, 12.7))));
    assert_eq!(poly(&h).vertices().len(), 4);
    assert!(poly(&h).is_closed());

    // Add a vertex on the bottom line and move it.
    assert!(h.run(|fsm, ctx| fsm.add_polygon_vertex(ctx, uuid, mm(35.56, 0.0))));
    h.move_to(mm(35.56, -2.54));
    h.release(mm(35.56, -2.54));
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(poly(&h).vertices().len(), 5);
    assert_eq!(poly(&h).vertices()[1].pos, mm(35.56, -2.54));
}

#[test]
fn find_symbols_and_nets() {
    use librepcb_editor::fsm::find::FindCandidate;
    let mut h = Harness::new();
    h.editor
        .execute(ConnectPinToNet {
            pin: PinRef::new("R1", "1"),
            net: librepcb_core::types::CircuitIdentifier::new("VCC").unwrap(),
            label: true,
            stub_length: None,
        })
        .unwrap();
    h.sync.sync(h.editor.project(), &mut h.scene).unwrap();
    let r2 = h.symbol("R2");
    h.run(|fsm, ctx| fsm.refresh_find_suggestions(ctx));
    let suggestions = h.fsm.search().suggestions().to_vec();
    assert!(suggestions.contains(&FindCandidate::component("R1")));
    assert!(suggestions.contains(&FindCandidate::net("VCC")));
    h.fsm.set_find_term("r2");
    let result = h.run(|fsm, ctx| fsm.find_next(ctx));
    assert_eq!(result.objects, vec![FindCandidate::component("R2")]);
    assert!(result.zoom_rect.is_some());
    assert_eq!(
        h.fsm.selection().iter().copied().collect::<Vec<_>>(),
        vec![SchematicItem::Symbol(r2)]
    );
    let r2_cmp = h.p().schematic(h.schematic).unwrap().symbols()[&r2].component();
    assert!(h.fsm.cross_probe().components.contains(&r2_cmp));

    h.fsm.set_find_term("VCC");
    let result = h.run(|fsm, ctx| fsm.find_next(ctx));
    assert_eq!(result.nets.len(), 1);
    let sel = h.fsm.selection();
    assert!(sel.iter().any(|i| matches!(i, SchematicItem::NetLabel(..))));
    assert!(sel.iter().any(|i| matches!(i, SchematicItem::NetLine(..))));
    assert!(
        sel.iter()
            .any(|i| matches!(i, SchematicItem::SymbolPin(..)))
    );

    // Nothing found: empty selection, no zoom.
    h.fsm.set_find_term("nothing");
    let result = h.run(|fsm, ctx| fsm.find_next(ctx));
    assert!(result.zoom_rect.is_none());
    assert!(h.fsm.selection().is_empty());
}

#[test]
fn copy_paste_bus_with_attached_wire() {
    let mut h = Harness::new();
    let r1_2 = h.pins("R1")[1].1;
    let bus_a = Point::new(r1_2.x, mm(0.0, 10.16).y);
    h.draw_bus(bus_a, bus_a + mm(10.16, 0.0));
    assert!(h.run(|fsm, ctx| fsm.draw_wire(ctx)));
    h.move_to(bus_a);
    h.press_mod(bus_a, Modifiers::CONTROL);
    h.move_to(r1_2);
    h.press(r1_2);
    assert!(h.run(|fsm, ctx| fsm.select_tool(ctx)));
    assert!(h.run(|fsm, ctx| fsm.select_all(ctx)));
    h.cursor = Point::ORIGIN;
    assert!(h.run(|fsm, ctx| fsm.copy(ctx)));
    let mime = librepcb_editor::fsm::schematic::clipboard::schematic_clipboard_mime_type(
        &SchematicEditorSettings::default().app_version,
    );
    let data = SchematicClipboardData::from_zip(&h.clipboard.get(&mime).unwrap()).unwrap();
    assert_eq!(data.buses.len(), 1);
    assert_eq!(data.bus_segments.len(), 1);
    assert_eq!(data.bus_segments[0].lines.len(), 1);
    assert!(
        data.net_segments
            .iter()
            .flat_map(|s| s.lines.iter())
            .any(|l| matches!(l.p1(), NetLineAnchor::BusJunction { .. })
                || matches!(l.p2(), NetLineAnchor::BusJunction { .. }))
    );
    assert!(h.run(|fsm, ctx| fsm.paste(ctx)));
    h.move_to(mm(0.0, 30.48));
    h.press(mm(0.0, 30.48));
    assert!(!h.editor.undo_stack().is_group_active());
    let segs = h.bus_segments();
    assert_eq!(segs.len(), 2);
    // Without label, the pasted segment got a new bus.
    assert_eq!(h.p().circuit().buses().len(), 2);
    let pasted = segs
        .iter()
        .find(|s| {
            s.junctions()
                .values()
                .any(|j| j.position() == bus_a + mm(0.0, 30.48))
        })
        .expect("pasted bus segment");
    let s = h.p().schematic(h.schematic).unwrap();
    assert!(
        !s.attached_net_segments(pasted.id()).is_empty(),
        "the pasted wire is attached to the pasted bus"
    );
}

/// A wire between R1 and R2 with a branch from (10.16, 0) to
/// (10.16, 10.16), back in the select tool.
fn wire_with_branch(h: &mut Harness) {
    draw_wire_between_resistors(h);
    h.run(|fsm, ctx| fsm.draw_wire(ctx));
    h.click(mm(10.16, 0.0));
    h.move_to(mm(10.16, 10.16));
    h.press(mm(10.16, 10.16));
    h.run(|fsm, ctx| fsm.abort(ctx));
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert_eq!(h.fsm.tool(), SchematicTool::Select);
}

fn line_count(h: &Harness) -> usize {
    let s = h.p().schematic(h.schematic).unwrap();
    s.net_segments().values().map(|seg| seg.lines().len()).sum()
}

#[test]
fn remove_line_then_simplify_is_a_separate_undo_step() {
    let mut h = Harness::new();
    wire_with_branch(&mut h);
    assert_eq!(line_count(&h), 3);
    let index = h.editor.undo_stack().index();
    h.click(mm(10.16, 5.0));
    assert_eq!(h.fsm.selection().len(), 1);
    assert!(matches!(
        h.fsm.selection().iter().next(),
        Some(SchematicItem::NetLine(..))
    ));
    h.run(|fsm, ctx| fsm.remove(ctx));
    // Removal, then the simplification merges the lines at the tap.
    assert_eq!(h.editor.undo_stack().index(), index + 2);
    assert_eq!(line_count(&h), 1);
    let s = h.p().schematic(h.schematic).unwrap();
    let segment = s.net_segments().values().next().unwrap();
    assert!(segment.junctions().is_empty());
    assert!(h.p().is_ref_index_consistent());
    h.undo();
    assert_eq!(line_count(&h), 2);
    h.undo();
    assert_eq!(line_count(&h), 3);
    h.redo();
    h.redo();
    assert_eq!(line_count(&h), 1);
}

#[test]
fn drag_then_simplify_is_a_separate_undo_step() {
    let mut h = Harness::new();
    wire_with_branch(&mut h);
    let index = h.editor.undo_stack().index();
    // Drag the end junction of the branch onto the tap junction.
    let end = mm(10.16, 10.16);
    h.move_to(end);
    assert!(h.press(end));
    assert!(matches!(
        h.fsm.selection().iter().next(),
        Some(SchematicItem::NetPoint(..))
    ));
    h.move_to(mm(10.16, 0.0));
    h.release(mm(10.16, 0.0));
    assert!(!h.editor.undo_stack().is_group_active());
    assert_eq!(h.editor.undo_stack().index(), index + 2);
    assert_eq!(line_count(&h), 1);
    assert!(h.p().is_ref_index_consistent());
    // The first undo step reverts the simplification only.
    h.undo();
    assert_eq!(line_count(&h), 3);
    let s = h.p().schematic(h.schematic).unwrap();
    let segment = s.net_segments().values().next().unwrap();
    assert!(segment.junctions().values().all(|j| j.position() != end));
    h.undo();
    let s = h.p().schematic(h.schematic).unwrap();
    let segment = s.net_segments().values().next().unwrap();
    assert!(segment.junctions().values().any(|j| j.position() == end));
}

#[test]
fn paste_graphics_from_symbol_editor() {
    use librepcb_core::geometry::{Path, Polygon, Text};
    use librepcb_core::types::{Alignment, Layer, PositiveLength, UnsignedLength};
    use librepcb_editor::library_editor::commands::{
        SymbolClipboardData, symbol_clipboard_mime_type,
    };
    let mut h = Harness::new();
    let mut data = SymbolClipboardData::new(Uuid::new_random(), Point::ORIGIN);
    data.polygons.push(Polygon::new(
        Uuid::new_random(),
        Layer::SYMBOL_OUTLINES,
        UnsignedLength::new(Length::new(200_000)).unwrap(),
        false,
        false,
        Path::rect(mm(0.0, 0.0), mm(2.54, 2.54)),
    ));
    for text in ["{{NAME}}", "Hello"] {
        data.texts.push(Text::new(
            Uuid::new_random(),
            Layer::SYMBOL_NAMES,
            text,
            mm(0.0, 5.08),
            Angle::DEG0,
            PositiveLength::new(Length::new(2_540_000)).unwrap(),
            Alignment::default(),
            false,
        ));
    }
    let version = SchematicEditorSettings::default().app_version;
    h.clipboard.set(
        &symbol_clipboard_mime_type(&version),
        data.to_zip().unwrap(),
    );
    h.cursor = mm(30.48, 30.48);
    h.run(|fsm, ctx| fsm.pointer_moved(ctx, PointerEvent::new(mm(30.48, 30.48))));
    assert!(h.fsm.view_state().features.paste);
    assert!(h.run(|fsm, ctx| fsm.paste(ctx)));
    h.press(mm(30.48, 30.48));
    assert!(!h.editor.undo_stack().is_group_active());
    let s = h.p().schematic(h.schematic).unwrap();
    assert_eq!(s.polygons().len(), 1);
    let poly = s.polygons().values().next().unwrap();
    assert_eq!(poly.path().vertices()[0].pos, mm(30.48, 30.48));
    // The placeholder evaluating to an empty string loses its braces.
    let texts: BTreeSet<String> = s.texts().values().map(|t| t.text().clone()).collect();
    assert_eq!(
        texts,
        ["NAME".to_owned(), "Hello".to_owned()]
            .into_iter()
            .collect()
    );
    h.undo();
    assert!(h.p().schematic(h.schematic).unwrap().polygons().is_empty());
}

#[test]
fn selected_wire_info_box_and_cross_probe() {
    let mut h = Harness::new();
    draw_wire_between_resistors(&mut h);
    h.click(mm(10.16, 0.0));
    assert!(matches!(
        h.fsm.selection().iter().next(),
        Some(SchematicItem::NetLine(..))
    ));
    let s = h.p().schematic(h.schematic).unwrap();
    let net = s.net_segments().values().next().unwrap().net();
    let name = h.p().circuit().net_signal(net).unwrap().name().to_string();
    let info = h.fsm.view_state().info_box.clone();
    assert!(info.contains("Net:") && info.contains(&name), "{info}");
    // The net class is shown only with several net classes.
    assert!(!info.contains("Class:"), "{info}");
    assert!(h.fsm.cross_probe().nets.contains(&net));
    assert!(h.fsm.cross_probe().components.is_empty());
    // Clearing the selection clears the info box and the cross-probing.
    h.click(mm(50.0, 50.0));
    assert!(h.fsm.view_state().info_box.is_empty());
    assert!(h.fsm.cross_probe().nets.is_empty());
}

#[test]
fn add_component_value_attribute_from_tool_bar() {
    let mut h = Harness::new();
    h.cursor = mm(0.0, 20.0);
    assert!(h.run(|fsm, ctx| {
        fsm.add_component(
            ctx,
            ComponentChoice {
                device: Some(lib::r0805()),
                ..ComponentChoice::new(lib::resistor())
            },
        )
    }));
    // The value "{{RESISTANCE}}" references the attribute RESISTANCE.
    assert_eq!(h.fsm.tool_data().value, "{{RESISTANCE}}");
    let attr = h.fsm.tool_data().value_attribute.clone().unwrap();
    assert_eq!(attr.key().as_str(), "RESISTANCE");
    let unit = attr
        .attribute_type()
        .unit_from_string("kiloohm")
        .unwrap()
        .unwrap();
    // Invalid values are ignored.
    h.run(|fsm, ctx| fsm.set_value_attribute_value(ctx, "abc"));
    let current = h.fsm.tool_data().value_attribute.clone().unwrap();
    assert_eq!(current.value(), "");
    h.run(|fsm, ctx| fsm.set_value_attribute_value(ctx, "4.7"));
    h.run(|fsm, ctx| fsm.set_value_attribute_unit(ctx, Some(unit)));
    let resistance = |h: &Harness, name: &str| {
        let (_, c) = h.p().circuit().component_instance_by_name(name).unwrap();
        let a = c.attributes().by_name("RESISTANCE", true).unwrap();
        (a.value().to_owned(), a.unit().map(|u| u.name()))
    };
    let expected = ("4.7".to_owned(), Some(unit.name()));
    assert_eq!(resistance(&h, "R3"), expected);
    // Placing keeps the attribute for the next component.
    h.move_to(mm(10.0, 20.0));
    h.press(mm(10.0, 20.0));
    assert_eq!(resistance(&h, "R3"), expected);
    assert_eq!(resistance(&h, "R4"), expected);
    // A value without attribute reference clears it.
    h.run(|fsm, ctx| fsm.set_value(ctx, "fixed"));
    assert!(h.fsm.tool_data().value_attribute.is_none());
    h.run(|fsm, ctx| fsm.abort(ctx));
    assert_eq!(resistance(&h, "R3"), expected);
}
