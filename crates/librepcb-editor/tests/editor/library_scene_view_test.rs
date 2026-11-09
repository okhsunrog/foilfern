//! The symbol editor FSM with a view over the symbol scene (what the
//! application does), and rebuilding the scene after modifications.

use std::path::Path;
use std::sync::Arc;

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::types::{Length, Point, Uuid};
use librepcb_editor::fsm::library::{
    LibraryContext, LibraryEditorSettings, LibraryView, SymbolEditorFsm,
};
use librepcb_editor::fsm::{MemoryClipboard, PointerEvent};
use librepcb_editor::library_editor::SymbolEditor;
use librepcb_editor::library_editor::commands::{SymbolItem, all_symbol_items};
use librepcb_scene::{ColorScheme, SymbolScene, SymbolSceneObject};

use super::helpers::test_data_dir;

fn mm(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).expect("valid point")
}

/// A [`LibraryView`] over the symbol scene.
struct SymbolSceneView(SymbolScene);

fn symbol_item(o: SymbolSceneObject) -> SymbolItem {
    match o {
        SymbolSceneObject::Pin(u) => SymbolItem::Pin(u),
        SymbolSceneObject::Polygon(u) => SymbolItem::Polygon(u),
        SymbolSceneObject::Circle(u) => SymbolItem::Circle(u),
        SymbolSceneObject::Text(u) => SymbolItem::Text(u),
        SymbolSceneObject::Image(u) => SymbolItem::Image(u),
    }
}

impl LibraryView<SymbolItem> for SymbolSceneView {
    fn items_at(&self, pos: Point, tolerance: Length) -> Vec<SymbolItem> {
        self.0
            .objects_at(pos, tolerance)
            .into_iter()
            .map(symbol_item)
            .collect()
    }

    fn items_in_rect(&self, p1: Point, p2: Point) -> Vec<SymbolItem> {
        self.0
            .objects_in_rect(p1, p2)
            .into_iter()
            .map(symbol_item)
            .collect()
    }

    fn tolerance(&self) -> Length {
        Length::new(100_000)
    }
}

fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dir");
    for entry in std::fs::read_dir(src).expect("read dir") {
        let entry = entry.expect("dir entry");
        std::fs::copy(entry.path(), dst.join(entry.file_name())).expect("copy file");
    }
}

#[test]
fn symbol_scene_as_view() {
    let uuid = "9b75d0ce-ac4e-4a52-a88a-8777f66d3241"; // Diode.
    let tmp = tempfile::tempdir().expect("temp dir");
    let dir = tmp.path().join(uuid);
    copy_dir_all(
        &test_data_dir()
            .join("libraries/Populated Library.lplib/sym")
            .join(uuid),
        &dir,
    );
    let fs = TransactionalFileSystem::open_rw(&FilePath::new(&dir).expect("path")).expect("fs");
    let mut editor =
        SymbolEditor::open(TransactionalDirectory::new(Arc::new(fs), "")).expect("open");
    let mut fsm = SymbolEditorFsm::new(LibraryEditorSettings::default());
    let mut clip = MemoryClipboard::new();
    let font = librepcb_scene::default_stroke_font();
    let scheme = ColorScheme::SCHEMATIC_LIGHT;
    let pin_c: Uuid = "48dad2d5-4392-4d8f-870e-9e69d9727b83"
        .parse()
        .expect("uuid");

    let view = SymbolSceneView(SymbolScene::build(editor.element(), font.as_ref(), &scheme));
    assert_eq!(
        view.0
            .objects_at(mm(0.8, 0.0), Length::new(100_000))
            .first()
            .copied(),
        Some(SymbolSceneObject::Pin(pin_c))
    );
    assert!(!view.0.items_of(SymbolSceneObject::Pin(pin_c)).is_empty());
    {
        let mut ctx = LibraryContext::new(&mut editor, &view, &mut clip);
        // Click the line of pin "C" (from 2.54/0 to 0/0, not covered by the name of pin "A").
        let e = PointerEvent::new(mm(0.8, 0.0));
        fsm.left_pressed(&mut ctx, e);
        fsm.left_released(&mut ctx, e);
        assert!(fsm.selection().contains(&SymbolItem::Pin(pin_c)));
        // Rubber band selection of everything, then remove.
        fsm.left_pressed(&mut ctx, PointerEvent::new(mm(-20.0, -20.0)));
        fsm.pointer_moved(&mut ctx, PointerEvent::new(mm(20.0, 20.0)));
        fsm.left_released(&mut ctx, PointerEvent::new(mm(20.0, 20.0)));
        assert_eq!(
            fsm.selection().len(),
            all_symbol_items(ctx.editor.element()).len()
        );
        assert!(fsm.remove(&mut ctx));
    }
    // Rebuild the scene after the modification: empty.
    let scene = SymbolScene::build(editor.element(), font.as_ref(), &scheme);
    assert!(scene.scene().is_empty());
    // Undo: the scene has the items again.
    assert!(editor.undo().expect("undo"));
    let scene = SymbolScene::build(editor.element(), font.as_ref(), &scheme);
    assert!(!scene.items_of(SymbolSceneObject::Pin(pin_c)).is_empty());
}
