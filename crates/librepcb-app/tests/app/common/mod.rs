//! Helpers shared by the integration tests: a small project with two
//! resistors on a schematic and a board, simulated pointer input and the UI
//! thread with Slint's headless platform.

#![allow(dead_code)]

mod headless;

pub use headless::with_headless;

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use librepcb_app::project::AppProject;
use librepcb_app::tabs::editing::point_to_world;
use librepcb_app::tabs::{Board2dTab, SchematicTab, TabUpdate};
use librepcb_canvas::kurbo::Point as ScreenPoint;
use librepcb_canvas::{Modifiers, PointerButton, PointerKind};
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::{BoardId, SchematicId, SymbolId};
use librepcb_core::types::{Angle, CircuitIdentifier, ElementName, Length, Point, Uuid};
use librepcb_editor::commands::{
    AddBoard, AddComponent, AddDevice, AddSchematic, ConnectPinToNet, PinRef, SetBoardOutline,
    SymbolPlacement,
};
use librepcb_editor::{DirectoryLibrarySource, ProjectEditor};

pub fn mm(x: f64, y: f64) -> Point {
    Point::new(Length::from_mm(x).unwrap(), Length::from_mm(y).unwrap())
}

pub fn uuid(s: &str) -> Uuid {
    s.parse().unwrap()
}

pub const RESISTOR: &str = "ef80cd5e-2689-47ee-8888-31d04fc99174";
pub const R0805: &str = "078650d3-483c-4b9e-a848-b14f1aad2edc";

pub fn library_source() -> Arc<DirectoryLibrarySource> {
    let dir = FilePath::new(
        Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
            .join("tests/data/libraries/Populated Library.lplib"),
    )
    .unwrap();
    Arc::new(DirectoryLibrarySource::from_libraries([&dir]).unwrap())
}

/// Creates a project with the resistors R1 and R2 on a schematic and a
/// board (not connected), saves it and opens it like the application.
pub fn create_project(dir: &Path) -> (Rc<AppProject>, SchematicId, BoardId) {
    {
        let fs = TransactionalFileSystem::open_rw(&FilePath::new(dir).unwrap()).unwrap();
        let directory = TransactionalDirectory::new(Arc::new(fs), "");
        let fonts = FilePath::new(
            Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("share/librepcb/fontobene"),
        )
        .unwrap();
        let project = librepcb_editor::create_project(directory, "test.lpp", Some(&fonts)).unwrap();
        let mut editor = ProjectEditor::with_source(project, library_source());
        let schematic = editor
            .execute(AddSchematic {
                name: ElementName::new("Main").unwrap(),
                index: None,
            })
            .unwrap();
        let board = editor
            .execute(AddBoard::new(ElementName::new("default").unwrap()))
            .unwrap()
            .board;
        for (name, x) in [("R1", 20.32), ("R2", 40.64)] {
            editor
                .execute(AddComponent {
                    device: Some(uuid(R0805)),
                    place: Some(SymbolPlacement {
                        schematic: Some(schematic),
                        position: mm(x, 50.8),
                        rotation: Angle::DEG0,
                        mirrored: false,
                        gate_offset: None,
                    }),
                    ..AddComponent::new(uuid(RESISTOR))
                })
                .unwrap();
            editor
                .execute(AddDevice {
                    component: name.into(),
                    board: Some(board),
                    device: None,
                    footprint: None,
                    position: mm(x / 2.0, 10.0),
                    rotation: Angle::DEG0,
                    mirrored: false,
                })
                .unwrap();
        }
        // R1.1 and R2.2 on the net GND (air wire on the board).
        for (component, pin) in [("R1", "1"), ("R2", "2")] {
            editor
                .execute(ConnectPinToNet {
                    pin: PinRef::new(component, pin),
                    net: CircuitIdentifier::new("GND").unwrap(),
                    label: true,
                    stub_length: None,
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
        editor.save().unwrap();
    }
    let lpp = FilePath::new(dir.join("test.lpp")).unwrap();
    let project = Rc::new(AppProject::open(&lpp, library_source()).unwrap());
    assert!(project.is_writable());
    let (sch, brd) = {
        let p = project.shared().lock();
        (
            p.project().schematics()[0].id(),
            p.project().boards()[0].id(),
        )
    };
    (project, sch, brd)
}

/// Simulates pointer input on a tab.
pub trait Input {
    fn pointer(&mut self, kind: PointerKind, button: PointerButton, pos: ScreenPoint) -> TabUpdate;
    fn screen(&self, p: Point) -> ScreenPoint;

    fn move_to(&mut self, p: Point) -> TabUpdate {
        let s = self.screen(p);
        self.pointer(PointerKind::Move, PointerButton::Other, s)
    }

    fn press(&mut self, p: Point) -> TabUpdate {
        let s = self.screen(p);
        self.pointer(PointerKind::Down, PointerButton::Left, s)
    }

    fn release(&mut self, p: Point) -> TabUpdate {
        let s = self.screen(p);
        self.pointer(PointerKind::Up, PointerButton::Left, s)
    }

    fn click(&mut self, p: Point) {
        self.move_to(p);
        self.press(p);
        self.release(p);
        // Avoid double clicks between consecutive clicks at the same place.
        std::thread::sleep(std::time::Duration::from_millis(510));
    }

    fn right_click(&mut self, p: Point) -> TabUpdate {
        let s = self.screen(p);
        self.move_to(p);
        self.pointer(PointerKind::Down, PointerButton::Right, s);
        self.pointer(PointerKind::Up, PointerButton::Right, s)
    }
}

impl Input for SchematicTab {
    fn pointer(&mut self, kind: PointerKind, button: PointerButton, pos: ScreenPoint) -> TabUpdate {
        self.pointer_event(kind, button, pos, Modifiers::default())
    }

    fn screen(&self, p: Point) -> ScreenPoint {
        self.canvas().view().world_to_screen(point_to_world(p))
    }
}

impl Input for Board2dTab {
    fn pointer(&mut self, kind: PointerKind, button: PointerButton, pos: ScreenPoint) -> TabUpdate {
        self.pointer_event(kind, button, pos, Modifiers::default())
    }

    fn screen(&self, p: Point) -> ScreenPoint {
        self.canvas().view().world_to_screen(point_to_world(p))
    }
}

pub fn symbol(project: &AppProject, sch: SchematicId, name: &str) -> (SymbolId, Point) {
    let p = project.shared().lock();
    let proj = p.project();
    let (c, _) = proj.circuit().component_instance_by_name(name).unwrap();
    let s = proj.schematic(sch).unwrap();
    let (id, sym) = s
        .symbols()
        .iter()
        .find(|(_, s)| s.component() == c)
        .unwrap();
    (*id, sym.position())
}

/// Pin positions of a symbol, sorted by X.
pub fn pins(project: &AppProject, sch: SchematicId, name: &str) -> Vec<Point> {
    let (id, _) = symbol(project, sch, name);
    let p = project.shared().lock();
    let proj = p.project();
    let s = proj.schematic(sch).unwrap();
    let mut pins: Vec<Point> = s.symbols()[&id]
        .pins(proj.view())
        .unwrap()
        .into_iter()
        .map(|v| v.position())
        .collect();
    pins.sort_by_key(|p| p.x);
    pins
}

pub fn wire_count(project: &AppProject, sch: SchematicId) -> usize {
    let p = project.shared().lock();
    p.project()
        .schematic(sch)
        .unwrap()
        .net_segments()
        .values()
        .map(|s| s.lines().len())
        .sum()
}

pub fn trace_count(project: &AppProject, board: BoardId) -> usize {
    let p = project.shared().lock();
    p.project()
        .board(board)
        .unwrap()
        .net_segments()
        .values()
        .map(|s| s.traces().len())
        .sum()
}

pub fn undo_index(project: &AppProject) -> usize {
    project.shared().lock().editor.undo_stack().index()
}
