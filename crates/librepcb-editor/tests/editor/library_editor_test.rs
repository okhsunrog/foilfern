//! Headless scenarios of the library element editors (symbol and package
//! FSMs, component and device editing, checks) on the upstream "Populated
//! Library".

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use librepcb_core::attribute::AttributeList;
use librepcb_core::fileio::{
    FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem,
};
use librepcb_core::geometry::PadFunction;
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::{Device, Part};
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::library::sym::SymbolCheckMessage;
use librepcb_core::library::{LibraryBaseElement, LibraryCheckMessage};
use librepcb_core::serialization::Mode;
use librepcb_core::types::{
    Angle, CircuitIdentifier, Length, Orientation, Point, SimpleString, Uuid,
};
use librepcb_editor::fsm::library::hit_test::ModelHitTester;
use librepcb_editor::fsm::library::package::PackageContext;
use librepcb_editor::fsm::library::symbol::SymbolContext;
use librepcb_editor::fsm::library::{
    LibraryContext, LibraryEditorSettings, LibraryRequest, LibraryTool, PackageEditorFsm,
    SymbolEditorFsm,
};
use librepcb_editor::fsm::{Clipboard, Key, KeyEvent, MemoryClipboard, Modifiers, PointerEvent};
use librepcb_editor::library_editor::checks::{FixOutcome, FixParams};
use librepcb_editor::library_editor::commands::*;
use librepcb_editor::library_editor::{
    ComponentEditor, DeviceEditor, EditableElement, LibraryElementCache, LibraryElementEditor,
    PackageEditor, SymbolEditor,
};

use super::helpers::{library_source, test_data_dir};

fn uuid(s: &str) -> Uuid {
    s.parse().expect("valid UUID")
}

fn mm(x: f64, y: f64) -> Point {
    Point::from_mm(x, y).expect("valid point")
}

fn ev(x: f64, y: f64) -> PointerEvent {
    PointerEvent::new(mm(x, y))
}

const DIODE_SYMBOL: &str = "9b75d0ce-ac4e-4a52-a88a-8777f66d3241";
const RESISTOR_CMP: &str = "ef80cd5e-2689-47ee-8888-31d04fc99174";
const R0805_DEV: &str = "078650d3-483c-4b9e-a848-b14f1aad2edc";
const RESC2012_PKG: &str = "2d00d07c-bfc1-4a96-a1cb-195c5ff93db9";
const R0603_PKG: &str = "5d9abd1b-cf0b-4cf7-8666-20a1add9971e";

fn populated_library() -> PathBuf {
    test_data_dir().join("libraries/Populated Library.lplib")
}

fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dir");
    for entry in std::fs::read_dir(src).expect("read dir") {
        let entry = entry.expect("dir entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir_all(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy file");
        }
    }
}

/// Copies an element directory (e.g. `sym/<uuid>`) of the populated library
/// into a temporary directory and returns (temp dir, element dir).
fn copy_element(kind: &str, uuid: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("temp dir");
    let dir = tmp.path().join(uuid);
    copy_dir_all(&populated_library().join(kind).join(uuid), &dir);
    (tmp, dir)
}

fn open_dir(dir: &Path) -> TransactionalDirectory {
    let fs = TransactionalFileSystem::open_rw(&FilePath::new(dir).expect("absolute path"))
        .expect("open file system");
    TransactionalDirectory::new(Arc::new(fs), "")
}

/// Opens an element read-only (e.g. while an editor holds the lock).
fn open_readonly<E: EditableElement>(dir: &Path) -> E {
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(dir).expect("absolute path"))
        .expect("open file system");
    E::open(TransactionalDirectory::new(Arc::new(fs), "")).expect("open element")
}

fn open<E: EditableElement>(dir: &Path) -> LibraryElementEditor<E> {
    LibraryElementEditor::open(open_dir(dir)).expect("open element")
}

fn bytes<E: LibraryBaseElement>(e: &E) -> Vec<u8> {
    e.to_sexpression()
        .to_byte_array(Mode::LibrePcb)
        .expect("serialize")
}

fn run_sym<R>(
    editor: &mut SymbolEditor,
    fsm: &mut SymbolEditorFsm,
    clipboard: &mut MemoryClipboard,
    f: impl FnOnce(&mut SymbolEditorFsm, &mut SymbolContext<'_>) -> R,
) -> R {
    let view = ModelHitTester::for_symbol(editor.element());
    let mut ctx = LibraryContext::new(editor, &view, clipboard);
    let r = f(fsm, &mut ctx);
    assert!(
        !fsm.take_requests()
            .iter()
            .any(|r| matches!(r, LibraryRequest::ShowError(_))),
        "FSM reported an error"
    );
    r
}

fn run_pkg<R>(
    editor: &mut PackageEditor,
    fsm: &mut PackageEditorFsm,
    clipboard: &mut MemoryClipboard,
    f: impl FnOnce(&mut PackageEditorFsm, &mut PackageContext<'_>) -> R,
) -> R {
    let fpt = fsm.footprint();
    let view = match fpt.and_then(|u| editor.element().footprints().by_uuid(&u)) {
        Some(f) => ModelHitTester::for_footprint(f),
        None => ModelHitTester::for_footprint(&librepcb_core::library::pkg::Footprint::new(
            Uuid::new_random(),
            librepcb_core::types::ElementName::new("x").expect("valid name"),
            String::new(),
        )),
    };
    let mut ctx = LibraryContext::new(editor, &view, clipboard);
    let r = f(fsm, &mut ctx);
    let errors: Vec<_> = fsm
        .take_requests()
        .into_iter()
        .filter(|r| matches!(r, LibraryRequest::ShowError(_)))
        .collect();
    assert!(errors.is_empty(), "FSM reported errors: {errors:?}");
    r
}

fn undo_all<E: EditableElement>(editor: &mut LibraryElementEditor<E>) -> usize {
    let mut n = 0;
    while editor.undo().expect("undo") {
        n += 1;
    }
    n
}

fn redo_all<E: EditableElement>(editor: &mut LibraryElementEditor<E>) -> usize {
    let mut n = 0;
    while editor.redo().expect("redo") {
        n += 1;
    }
    n
}

#[test]
fn saving_unmodified_elements_is_byte_identical() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let lib = tmp.path().join("lib.lplib");
    copy_dir_all(&populated_library(), &lib);
    let mut count = 0;
    for (kind, file) in [
        ("sym", "symbol.lp"),
        ("pkg", "package.lp"),
        ("cmp", "component.lp"),
        ("dev", "device.lp"),
    ] {
        for entry in std::fs::read_dir(lib.join(kind)).expect("read dir") {
            let dir = entry.expect("entry").path();
            let original = std::fs::read(dir.join(file)).expect("read element");
            match kind {
                "sym" => open::<Symbol>(&dir).save().expect("save"),
                "pkg" => open::<Package>(&dir).save().expect("save"),
                "cmp" => open::<Component>(&dir).save().expect("save"),
                _ => open::<Device>(&dir).save().expect("save"),
            }
            let saved = std::fs::read(dir.join(file)).expect("read element");
            assert_eq!(
                String::from_utf8_lossy(&saved),
                String::from_utf8_lossy(&original),
                "{}",
                dir.display()
            );
            count += 1;
        }
    }
    assert_eq!(count, 19 + 8 + 15 + 8);
}

#[test]
fn draw_symbol_with_pins_and_save() {
    let (_tmp, dir) = copy_element("sym", DIODE_SYMBOL);
    let mut editor: SymbolEditor = open(&dir);
    let original = editor.element().content();
    let mut fsm = SymbolEditorFsm::new(LibraryEditorSettings::default());
    let mut clip = MemoryClipboard::new();

    // Select all and remove everything.
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.select_all(ctx));
        assert!(fsm.remove(ctx));
    });
    assert!(editor.element().is_empty());

    // Add three pins, the third one rotated.
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.set_tool(ctx, LibraryTool::AddPins));
        assert_eq!(fsm.tool(), LibraryTool::AddPins);
        fsm.click(ctx, ev(-7.62, 2.54));
        fsm.click(ctx, ev(-7.62, -2.54));
        fsm.pointer_moved(ctx, ev(7.7, 0.1));
        fsm.rotate(ctx, Angle::DEG180);
        fsm.left_pressed(ctx, ev(7.7, 0.1));
        // The next pin follows the cursor; Escape aborts it.
        assert!(fsm.abort(ctx));
        assert_eq!(fsm.tool(), LibraryTool::Select);
    });
    let pins: Vec<_> = editor.element().pins().iter().cloned().collect();
    assert_eq!(pins.len(), 3);
    assert_eq!(
        pins.iter().map(|p| p.name().as_str()).collect::<Vec<_>>(),
        ["1", "2", "3"]
    );
    assert_eq!(pins[0].position(), mm(-7.62, 2.54));
    assert_eq!(pins[2].position(), mm(7.62, 0.0));
    assert_eq!(pins[2].rotation(), Angle::DEG180);

    // Rectangle, line, arc, circle.
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.set_tool(ctx, LibraryTool::DrawRect));
        fsm.click(ctx, ev(-5.08, -5.08));
        fsm.click(ctx, ev(5.08, 5.08));
        assert!(fsm.set_tool(ctx, LibraryTool::DrawLine));
        fsm.click(ctx, ev(-5.08, 0.0));
        fsm.click(ctx, ev(-2.54, 0.0));
        fsm.click(ctx, ev(-2.54, 0.0)); // Empty segment finishes.
        assert!(fsm.set_tool(ctx, LibraryTool::DrawArc));
        fsm.click(ctx, ev(0.0, 0.0)); // Center.
        fsm.click(ctx, ev(2.54, 0.0)); // Start.
        fsm.pointer_moved(ctx, ev(0.0, 1.5));
        fsm.click(ctx, ev(0.0, 2.54)); // End.
        assert!(fsm.set_tool(ctx, LibraryTool::DrawCircle));
        fsm.click(ctx, ev(2.54, 2.54));
        fsm.click(ctx, ev(5.0, 2.6));
    });
    let polygons: Vec<_> = editor.element().polygons().iter().cloned().collect();
    assert_eq!(polygons.len(), 3);
    assert_eq!(polygons[0].path().vertices().len(), 5);
    assert!(polygons[0].is_grab_area());
    assert!(polygons[0].path().is_closed());
    assert_eq!(polygons[1].path().vertices().len(), 2);
    assert!(!polygons[1].is_grab_area());
    let arc = polygons[2].path().vertices();
    assert_eq!(arc.len(), 2);
    assert_eq!(arc[0].pos, mm(2.54, 0.0));
    assert_eq!(arc[1].pos, mm(0.0, 2.54));
    assert_eq!(arc[0].angle, Angle::DEG90);
    let circles: Vec<_> = editor.element().circles().iter().cloned().collect();
    assert_eq!(circles.len(), 1);
    assert_eq!(circles[0].center(), mm(2.54, 2.54));
    assert_eq!(
        *circles[0].diameter(),
        Length::from_mm(5.08).expect("length")
    );

    // Names and values.
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        fsm.pointer_moved(ctx, ev(0.0, 7.62));
        assert!(fsm.set_tool(ctx, LibraryTool::AddNames));
        fsm.left_pressed(ctx, ev(-5.08, 7.62));
        assert!(fsm.abort(ctx));
        assert!(fsm.set_tool(ctx, LibraryTool::AddValues));
        fsm.left_pressed(ctx, ev(-5.08, -7.62));
        assert!(fsm.abort(ctx));
    });
    let texts: Vec<_> = editor.element().texts().iter().cloned().collect();
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0].text(), "{{NAME}}");
    assert_eq!(texts[0].position(), mm(-5.08, 7.62));
    assert_eq!(texts[1].text(), "{{VALUE}}");
    assert!(!editor.is_group_active());

    // The checks are happy with names and values.
    let msgs = editor.run_checks().expect("checks");
    assert!(!msgs.iter().any(|m| matches!(
        m,
        LibraryCheckMessage::Symbol(
            SymbolCheckMessage::MissingSymbolName | SymbolCheckMessage::MissingSymbolValue
        )
    )));
    assert!(editor.is_dirty());
    assert!(editor.is_interface_broken());

    // Save, reopen: identical.
    editor.save().expect("save");
    assert!(!editor.is_dirty());
    let saved_content = editor.element().content();
    let saved = std::fs::read(dir.join("symbol.lp")).expect("read");
    let reopened: Symbol = open_readonly(&dir);
    assert_eq!(bytes(&reopened), saved);
    assert_eq!(reopened.content(), saved_content);

    // Undo everything: back to the original symbol; redo everything.
    let n = undo_all(&mut editor);
    assert!(n >= 10, "{n} undo steps");
    assert_eq!(editor.element().content(), original);
    assert!(editor.is_dirty());
    assert_eq!(redo_all(&mut editor), n);
    assert_eq!(editor.element().content(), saved_content);
    assert!(!editor.is_dirty());
}

#[test]
fn symbol_select_move_rotate_copy_paste() {
    let (_tmp, dir) = copy_element("sym", DIODE_SYMBOL);
    let mut editor: SymbolEditor = open(&dir);
    let original = editor.element().content();
    let mut fsm = SymbolEditorFsm::new(LibraryEditorSettings::default());
    let mut clip = MemoryClipboard::new();
    let pin_c = uuid("48dad2d5-4392-4d8f-870e-9e69d9727b83");

    // Click on pin "C" (2.54/0, pointing to the left) and drag it.
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        fsm.left_pressed(ctx, ev(2.0, 0.0));
        assert_eq!(
            fsm.selection().iter().copied().collect::<Vec<_>>(),
            [SymbolItem::Pin(pin_c)]
        );
        fsm.pointer_moved(ctx, ev(4.6, 2.5));
        // Live preview inside an open group.
        assert!(ctx.editor.is_group_active());
        fsm.left_released(ctx, ev(4.6, 2.5));
    });
    let pin = editor
        .element()
        .pins()
        .by_uuid(&pin_c)
        .expect("pin")
        .clone();
    assert_eq!(pin.position(), mm(5.08, 2.54));
    assert_eq!(editor.history().len(), 1);

    // Rotate and mirror the selection, snap, arrow keys.
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.rotate(ctx, Angle::DEG90));
        assert!(fsm.mirror(ctx, Orientation::Horizontal));
        assert!(fsm.move_by(ctx, mm(0.0, -2.54)));
    });
    let pin = editor
        .element()
        .pins()
        .by_uuid(&pin_c)
        .expect("pin")
        .clone();
    assert_eq!(pin.position(), mm(5.08, 0.0));
    // 180° + 90° = 270°, mirrored horizontally: 180° - 270° = -90°.
    assert_eq!(pin.rotation(), -Angle::DEG90);
    assert_eq!(editor.history().len(), 4);

    // Copy all and paste with an offset.
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.select_all(ctx));
        fsm.pointer_moved(ctx, ev(0.0, 0.0));
        assert!(fsm.copy(ctx));
        fsm.abort(ctx); // Clear the selection.
        fsm.pointer_moved(ctx, ev(0.0, 12.7));
        assert!(fsm.paste(ctx));
        fsm.pointer_moved(ctx, ev(0.0, 25.4));
        fsm.left_pressed(ctx, ev(0.0, 25.4));
    });
    assert_eq!(editor.element().pins().len(), 4);
    let names: BTreeSet<_> = editor
        .element()
        .pins()
        .iter()
        .map(|p| p.name().as_str().to_owned())
        .collect();
    // Pasted pin names are made unique.
    assert_eq!(names.len(), 4);
    assert!(
        editor
            .element()
            .pins()
            .iter()
            .any(|p| p.position() == mm(5.08, 25.4))
    );
    assert_eq!(editor.history().len(), 5);
    assert_eq!(editor.history()[4].text, "Paste Symbol Elements");

    // Undo everything.
    undo_all(&mut editor);
    assert_eq!(editor.element().content(), original);
}

#[test]
fn import_pins_are_pasted() {
    let (_tmp, dir) = copy_element("sym", DIODE_SYMBOL);
    let mut editor: SymbolEditor = open(&dir);
    let mut fsm = SymbolEditorFsm::new(LibraryEditorSettings::default());
    let mut clip = MemoryClipboard::new();
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        let names: Vec<_> = ["VCC", "GND", "A"]
            .iter()
            .map(|n| CircuitIdentifier::new(*n).expect("valid name"))
            .collect();
        assert!(fsm.import_pins(ctx, &names));
        fsm.pointer_moved(ctx, ev(10.16, 10.16));
        fsm.left_pressed(ctx, ev(10.16, 10.16));
    });
    let pins: Vec<_> = editor
        .element()
        .pins()
        .iter()
        .map(|p| (p.name().as_str().to_owned(), p.position()))
        .collect();
    assert_eq!(pins.len(), 5);
    // "A" exists already: renamed to "A1".
    assert!(pins.contains(&("A1".to_owned(), mm(10.16, 5.08))));
    assert!(pins.contains(&("VCC".to_owned(), mm(10.16, 10.16))));
}

#[test]
fn symbol_check_fixes() {
    let (_tmp, dir) = copy_element("sym", DIODE_SYMBOL);
    let mut editor: SymbolEditor = open(&dir);
    let pin_c = uuid("48dad2d5-4392-4d8f-870e-9e69d9727b83");
    editor
        .execute(EditSymbolPin {
            position: Some(mm(2.6, 0.0)),
            ..EditSymbolPin::new(pin_c)
        })
        .expect("edit pin");
    let msgs = editor.run_checks().expect("checks");
    let msg = msgs
        .iter()
        .find(|m| {
            matches!(
                m,
                LibraryCheckMessage::Symbol(SymbolCheckMessage::PinNotOnGrid { .. })
            )
        })
        .expect("pin not on grid")
        .clone();
    assert!(SymbolEditor::can_auto_fix(&msg));
    assert_eq!(
        editor.auto_fix(&msg, &FixParams::default()).expect("fix"),
        FixOutcome::Applied
    );
    assert_eq!(
        editor
            .element()
            .pins()
            .by_uuid(&pin_c)
            .expect("pin")
            .position(),
        mm(2.54, 0.0)
    );
    let msgs = editor.run_checks().expect("checks");
    assert!(!msgs.iter().any(|m| matches!(
        m,
        LibraryCheckMessage::Symbol(SymbolCheckMessage::PinNotOnGrid { .. })
    )));

    // Approvals bypass the undo stack but make the element dirty.
    undo_all(&mut editor);
    assert!(!editor.is_dirty());
    let approval = LibraryCheckMessage::Symbol(SymbolCheckMessage::MissingSymbolName)
        .to_message()
        .approval()
        .clone();
    assert!(editor.set_message_approved(&approval, true));
    assert!(editor.is_dirty());
    assert!(!editor.can_undo());
}

#[test]
fn add_pads_to_footprint_and_renumber() {
    let (_tmp, dir) = copy_element("pkg", RESC2012_PKG);
    let mut editor: PackageEditor = open(&dir);
    let original = editor.element().content();
    let fpt = editor
        .element()
        .footprints()
        .first()
        .expect("footprint")
        .uuid();
    let mut fsm = PackageEditorFsm::new(LibraryEditorSettings::default());
    let mut clip = MemoryClipboard::new();

    // Tools need a footprint.
    run_pkg(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(!fsm.set_tool(ctx, LibraryTool::AddThtPads));
        assert!(fsm.set_footprint(ctx, Some(fpt)));
    });

    // Two more package pads.
    let new_pads = editor
        .execute(AddPackagePads {
            names: "3..4".to_owned(),
        })
        .expect("add package pads");
    assert_eq!(new_pads.len(), 2);

    // Add two SMT pads, connected to the free package pads 3 and 4.
    run_pkg(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.set_tool(ctx, LibraryTool::AddSmtPads(PadFunction::StandardPad)));
        assert_eq!(fsm.tool_data().package_pad, Some(new_pads[0]));
        fsm.pointer_moved(ctx, ev(2.54, 2.54));
        fsm.left_pressed(ctx, ev(2.54, 2.54));
        assert_eq!(fsm.tool_data().package_pad, Some(new_pads[1]));
        fsm.pointer_moved(ctx, ev(5.08, 2.54));
        fsm.rotate(ctx, Angle::DEG90);
        fsm.left_pressed(ctx, ev(5.08, 2.54));
        assert_eq!(fsm.tool_data().package_pad, None);
        assert!(fsm.abort(ctx));
    });
    let pads: Vec<_> = editor
        .element()
        .footprints()
        .by_uuid(&fpt)
        .expect("fpt")
        .pads()
        .iter()
        .cloned()
        .collect();
    assert_eq!(pads.len(), 4);
    assert_eq!(pads[2].package_pad_uuid(), Some(new_pads[0]));
    assert_eq!(pads[2].pad().position(), mm(2.54, 2.54));
    assert_eq!(pads[3].package_pad_uuid(), Some(new_pads[1]));
    assert_eq!(pads[3].pad().rotation(), Angle::DEG90);
    assert!(editor.is_interface_broken());

    // Add a THT pad and a hole, draw a zone.
    run_pkg(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.set_tool(ctx, LibraryTool::AddThtPads));
        fsm.left_pressed(ctx, ev(-5.08, 5.08));
        fsm.abort(ctx);
        assert!(fsm.set_tool(ctx, LibraryTool::AddHoles));
        fsm.left_pressed(ctx, ev(-5.08, -5.08));
        fsm.abort(ctx);
        assert!(fsm.set_tool(ctx, LibraryTool::DrawZone));
        fsm.click(ctx, ev(0.0, -5.08));
        fsm.click(ctx, ev(2.54, -5.08));
        fsm.click(ctx, ev(2.54, -7.62));
        fsm.click(ctx, ev(2.54, -7.62)); // Finish.
        fsm.abort(ctx);
    });
    let f = editor
        .element()
        .footprints()
        .by_uuid(&fpt)
        .expect("fpt")
        .clone();
    assert_eq!(f.pads().len(), 5);
    assert!(f.pads().iter().last().expect("pad").pad().is_tht());
    assert_eq!(f.holes().len(), 1);
    assert_eq!(f.zones().len(), 1);
    assert_eq!(
        f.zones().first().expect("zone").outline().vertices().len(),
        3
    );

    // Remove the THT pad again (select it by clicking).
    run_pkg(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        fsm.left_pressed(ctx, ev(-5.08, 5.08));
        fsm.left_released(ctx, ev(-5.08, 5.08));
        assert_eq!(fsm.selection().len(), 1);
        assert!(fsm.remove(ctx));
    });
    let pads: Vec<_> = editor
        .element()
        .footprints()
        .by_uuid(&fpt)
        .expect("fpt")
        .pads()
        .iter()
        .cloned()
        .collect();
    assert_eq!(pads.len(), 4);

    // Re-number the pads by clicking them in a new order (Ctrl: single).
    let order = [
        pads[3].clone(),
        pads[2].clone(),
        pads[1].clone(),
        pads[0].clone(),
    ];
    run_pkg(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.set_tool(ctx, LibraryTool::RenumberPads));
    });
    for pad in &order {
        run_pkg(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
            let e = PointerEvent::with_modifiers(pad.pad().position(), Modifiers::CONTROL);
            fsm.pointer_moved(ctx, e);
            fsm.left_pressed(ctx, e);
        });
    }
    assert_eq!(fsm.tool(), LibraryTool::Select);
    assert!(!editor.is_group_active());
    let names: BTreeMap<Uuid, String> = editor
        .element()
        .pads()
        .iter()
        .map(|p| (p.uuid(), p.name().as_str().to_owned()))
        .collect();
    let f = editor
        .element()
        .footprints()
        .by_uuid(&fpt)
        .expect("fpt")
        .clone();
    for (i, pad) in order.iter().enumerate() {
        let pkg_pad = f
            .pads()
            .by_uuid(&pad.uuid())
            .expect("pad")
            .package_pad_uuid();
        assert_eq!(
            pkg_pad.map(|u| names[&u].clone()),
            Some((i + 1).to_string())
        );
    }
    assert_eq!(
        editor.history().last().map(|h| h.text.as_str()),
        Some("Re-number pads")
    );

    // Undo the renumbering, then everything.
    assert!(editor.undo().expect("undo"));
    let f = editor
        .element()
        .footprints()
        .by_uuid(&fpt)
        .expect("fpt")
        .clone();
    assert_eq!(
        f.pads()
            .by_uuid(&pads[2].uuid())
            .expect("pad")
            .package_pad_uuid(),
        Some(new_pads[0])
    );
    undo_all(&mut editor);
    assert_eq!(editor.element().content(), original);
    assert!(!editor.is_interface_broken());
    redo_all(&mut editor);
    editor.save().expect("save");
    let saved = std::fs::read(dir.join("package.lp")).expect("read");
    let reopened: Package = open_readonly(&dir);
    assert_eq!(bytes(&reopened), saved);
}

#[test]
fn package_generate_outline_and_courtyard() {
    let (_tmp, dir) = copy_element("pkg", RESC2012_PKG);
    let mut editor: PackageEditor = open(&dir);
    let fpt = editor
        .element()
        .footprints()
        .first()
        .expect("footprint")
        .uuid();
    let outlines = |e: &PackageEditor| {
        e.element()
            .footprints()
            .by_uuid(&fpt)
            .expect("fpt")
            .polygons()
            .iter()
            .filter(|p| p.layer().is_package_outline())
            .count()
    };
    let before = outlines(&editor);
    assert!(
        editor
            .execute(GeneratePackageOutline { footprint: fpt })
            .expect("outline")
    );
    assert_eq!(outlines(&editor), before.max(1));
    let courtyard = editor
        .execute(GenerateCourtyard {
            footprint: fpt,
            offset: librepcb_core::types::PositiveLength::new(Length::new(200_000))
                .expect("positive"),
        })
        .expect("courtyard");
    assert!(courtyard);
    // The checks do not complain about missing outline or courtyard.
    let msgs = editor.run_checks().expect("checks");
    assert!(!msgs.iter().any(|m| matches!(
        m,
        LibraryCheckMessage::Package(
            librepcb_core::library::pkg::PackageCheckMessage::MissingCourtyard(_)
                | librepcb_core::library::pkg::PackageCheckMessage::MissingPackageOutline(_)
        )
    )));
}

#[test]
fn edit_component_signals_and_pinout() {
    let (_tmp, dir) = copy_element("cmp", RESISTOR_CMP);
    let mut editor: ComponentEditor = open(&dir);
    let original = editor.element().content();
    let original_bytes = std::fs::read(dir.join("component.lp")).expect("read");
    let variant = editor
        .element()
        .symbol_variants()
        .first()
        .expect("variant")
        .clone();
    let gate = variant.symbol_items().first().expect("gate").clone();
    let pin = gate.pin_signal_map().first().expect("pin").pin_uuid();

    // Add signals with a range, rename one, connect a pin to it.
    let sigs = editor
        .execute(AddComponentSignals {
            names: "X1..2".to_owned(),
        })
        .expect("add signals");
    assert_eq!(sigs.len(), 2);
    assert!(
        editor
            .execute(AddComponentSignals {
                names: "X1".to_owned()
            })
            .is_err()
    );
    editor
        .execute(EditComponentSignal {
            name: Some(CircuitIdentifier::new("CLK").expect("valid")),
            clock: Some(true),
            ..EditComponentSignal::new(sigs[0])
        })
        .expect("edit signal");
    editor
        .execute(SetPinSignal {
            variant: variant.uuid(),
            gate: gate.uuid(),
            pin,
            signal: Some(Some(sigs[0])),
            display_type: None,
        })
        .expect("connect pin");
    assert_eq!(
        editor
            .element()
            .signal_of_pin(variant.uuid(), gate.uuid(), pin)
            .expect("pin")
            .map(|s| s.name().as_str().to_owned()),
        Some("CLK".to_owned())
    );
    assert!(editor.is_interface_broken());

    // Removing the signal disconnects the pin.
    editor
        .execute(RemoveComponentSignal { signal: sigs[0] })
        .expect("remove signal");
    assert_eq!(
        editor
            .element()
            .signal_of_pin(variant.uuid(), gate.uuid(), pin)
            .expect("pin"),
        None
    );
    assert!(has_unassigned_signals(editor.element(), variant.uuid()));

    // Auto-connect the pins of all gates by name (symbols from the library).
    let cache = LibraryElementCache::new(Arc::new(library_source()));
    let symbol = cache.symbol(&gate.symbol_uuid()).expect("symbol");
    let symbols: HashMap<Uuid, &Symbol> = [(gate.symbol_uuid(), symbol.as_ref())].into();
    editor
        .execute(AutoConnectPins {
            variant: variant.uuid(),
            symbols: &symbols,
        })
        .expect("auto connect");
    let connected = editor
        .element()
        .symbol_variants()
        .first()
        .expect("variant")
        .clone();
    assert_eq!(connected, variant);

    // Gates: add one (suffixes A/B), move it up, remove it.
    let new_gate = editor
        .execute(AddGate {
            variant: variant.uuid(),
            symbol: &symbol,
            create_signals: true,
        })
        .expect("add gate");
    let v = editor
        .element()
        .symbol_variants()
        .first()
        .expect("variant")
        .clone();
    assert_eq!(v.symbol_items().len(), 2);
    assert_eq!(
        v.symbol_items()
            .iter()
            .map(|g| g.suffix().as_str().to_owned())
            .collect::<Vec<_>>(),
        ["A", "B"]
    );
    // Signals "1"/"2" exist: the new ones are "1_2" and "2_2".
    assert!(editor.element().signals().contains_name("1_2"));
    editor
        .execute(MoveGateUp {
            variant: variant.uuid(),
            gate: new_gate,
        })
        .expect("move gate");
    editor
        .execute(RemoveGate {
            variant: variant.uuid(),
            gate: new_gate,
            remove_signals: true,
        })
        .expect("remove gate");
    let v = editor
        .element()
        .symbol_variants()
        .first()
        .expect("variant")
        .clone();
    assert_eq!(
        v.symbol_items().first().expect("gate").suffix().as_str(),
        ""
    );
    assert!(!editor.element().signals().contains_name("1_2"));

    // Variants.
    let new_variant = editor
        .execute(AddSymbolVariant { symbol: &symbol })
        .expect("add variant");
    editor
        .execute(SetDefaultSymbolVariant {
            variant: new_variant,
        })
        .expect("set default");
    assert_eq!(
        editor
            .element()
            .symbol_variants()
            .first()
            .expect("variant")
            .uuid(),
        new_variant
    );

    // Undo all: original; save is byte identical.
    undo_all(&mut editor);
    assert_eq!(editor.element().content(), original);
    editor.save().expect("save");
    assert_eq!(
        std::fs::read(dir.join("component.lp")).expect("read"),
        original_bytes
    );
}

#[test]
fn edit_device_pinout_and_parts() {
    let (_tmp, dir) = copy_element("dev", R0805_DEV);
    let mut editor: DeviceEditor = open(&dir);
    let original = editor.element().content();
    let original_pinout = device_pinout(editor.element());
    assert_eq!(original_pinout.len(), 2);
    let cache = LibraryElementCache::new(Arc::new(library_source()));
    let pkg = cache.package(&uuid(RESC2012_PKG)).expect("package");
    let cmp = cache.component(&uuid(RESISTOR_CMP)).expect("component");

    // Reset and auto-connect by name.
    editor.execute(reset_pinout()).expect("reset");
    assert!(device_pinout(editor.element()).is_empty());
    assert!(has_unconnected_pads_and_signals(
        editor.element(),
        cmp.signals()
    ));
    assert!(has_auto_connectable_pads(
        editor.element(),
        pkg.pads(),
        cmp.signals()
    ));
    let cmd = auto_connect_pinout(editor.element(), pkg.pads(), cmp.signals(), false);
    editor.execute(cmd).expect("auto connect");
    assert_eq!(device_pinout(editor.element()), original_pinout);

    // Load a swapped pinout from CSV (after reset).
    let csv = "Pad,Signal\r\n1,2\r\n2,1\r\n";
    let cmd = pinout_from_csv(editor.element(), pkg.pads(), cmp.signals(), csv, true);
    editor.execute(cmd).expect("load CSV");
    let pad1 = pkg.pads().by_name("1", true).expect("pad").uuid();
    let sig2 = cmp.signals().by_name("2", true).expect("signal").uuid();
    assert_eq!(device_pinout(editor.element()).get(&pad1), Some(&sig2));

    // Interactive assignment after a reset: pad "1" first, best match "1".
    editor.execute(reset_pinout()).expect("reset");
    let mut builder = InteractivePinout::start(editor.element(), pkg.pads(), cmp.signals());
    assert!(builder.is_active());
    assert_eq!(builder.current_pad().map(|(_, n)| n), Some("1"));
    assert_eq!(builder.choices()[0].name, "1");
    let cmd = builder
        .commit(editor.element(), cmp.signals())
        .expect("command");
    editor.execute(cmd).expect("assign");
    assert_eq!(builder.current_pad().map(|(_, n)| n), Some("2"));
    let cmd = builder
        .commit(editor.element(), cmp.signals())
        .expect("command");
    editor.execute(cmd).expect("assign");
    assert!(!builder.is_active());
    assert_eq!(device_pinout(editor.element()), original_pinout);

    // Change the package: pads of the other package, unconnected.
    let other = cache.package(&uuid(R0603_PKG)).expect("package");
    editor
        .execute(SetDevicePackage { package: &other })
        .expect("change package");
    assert_eq!(editor.element().package_uuid(), uuid(R0603_PKG));
    assert_eq!(
        editor.element().pad_signal_map().uuid_set(),
        other.pads().uuid_set()
    );

    // Parts.
    let part = Part::new(
        SimpleString::clean("RC0805FR-0710KL"),
        SimpleString::clean("Yageo"),
        AttributeList::new(),
    );
    let index = editor.execute(AddPart { part }).expect("add part");
    editor
        .execute(EditPart {
            index,
            mpn: Some(SimpleString::clean("RC0805FR-0747KL")),
            manufacturer: None,
            attributes: None,
        })
        .expect("edit part");
    editor.execute(DuplicatePart { index }).expect("duplicate");
    assert_eq!(editor.element().parts().len(), original.parts.len() + 2);
    editor.execute(RemovePart { index }).expect("remove");
    assert!(editor.execute(RemovePart { index: 100 }).is_err());

    // Undo all.
    undo_all(&mut editor);
    assert_eq!(editor.element().content(), original);
    assert!(!editor.is_dirty());
}

#[test]
fn metadata_edit_and_read_only() {
    let (_tmp, dir) = copy_element("sym", DIODE_SYMBOL);
    let mut editor: SymbolEditor = open(&dir);
    editor
        .execute(EditElementMetadata {
            name: Some(librepcb_core::types::ElementName::new("my diode").expect("valid")),
            ..EditElementMetadata::default()
        })
        .expect("edit");
    let msgs = editor.run_checks().expect("checks");
    let msg = msgs
        .iter()
        .find(|m| {
            matches!(
                m,
                LibraryCheckMessage::BaseElement(
                    librepcb_core::library::LibraryBaseElementCheckMessage::NameNotTitleCase { .. }
                )
            )
        })
        .expect("not title case")
        .clone();
    editor.auto_fix(&msg, &FixParams::default()).expect("fix");
    assert_eq!(editor.element().metadata().name().as_str(), "My Diode");
    assert_eq!(editor.history().len(), 2);

    // Read-only elements cannot be modified.
    let fs = TransactionalFileSystem::open_ro(
        &FilePath::new(populated_library().join("sym").join(DIODE_SYMBOL)).expect("path"),
    )
    .expect("open");
    let mut ro: SymbolEditor =
        LibraryElementEditor::open(TransactionalDirectory::new(Arc::new(fs), "")).expect("open");
    assert!(!ro.is_writable());
    assert!(
        ro.execute(RemoveSymbolItems {
            items: BTreeSet::new()
        })
        .is_err()
    );
}

#[test]
fn measure_tool_in_symbol_editor() {
    let (_tmp, dir) = copy_element("sym", DIODE_SYMBOL);
    let mut editor: SymbolEditor = open(&dir);
    let mut fsm = SymbolEditorFsm::new(LibraryEditorSettings::default());
    let mut clip = MemoryClipboard::new();
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.set_tool(ctx, LibraryTool::Measure));
        fsm.click(ctx, ev(-2.54, 0.1)); // Snaps to the pin.
        fsm.click(ctx, ev(2.5, 0.0));
        assert!(fsm.copy(ctx));
        fsm.key_pressed(ctx, KeyEvent::new(Key::Escape));
    });
    assert_eq!(clip.get("text/plain"), Some(b"5.08".to_vec()));
    assert!(!editor.is_dirty());
}

#[test]
fn file_of_removed_model_is_deleted_on_save() {
    let (_tmp, dir) = copy_element("pkg", RESC2012_PKG);
    let mut editor: PackageEditor = open(&dir);
    let model = editor.element().models().first().expect("model").clone();
    assert!(dir.join(model.file_name()).exists());
    editor
        .execute(RemovePackageModel {
            model: model.uuid(),
        })
        .expect("remove model");
    assert!(
        editor
            .element()
            .footprints()
            .iter()
            .all(|f| !f.models().contains(&model.uuid()))
    );
    editor.save().expect("save");
    assert!(!dir.join(model.file_name()).exists());
    // A new model's file is written.
    let new = editor
        .execute(AddPackageModel {
            name: librepcb_core::types::ElementName::new("New").expect("valid"),
            step: b"ISO-10303-21;".to_vec(),
            add_to_footprints: true,
        })
        .expect("add model");
    editor.save().expect("save");
    assert!(dir.join(format!("{new}.step")).exists());
    let _ = editor.element().directory().files("");
}

#[test]
fn package_flip_and_move_align() {
    let (_tmp, dir) = copy_element("pkg", RESC2012_PKG);
    let mut editor: PackageEditor = open(&dir);
    let original = editor.element().content();
    let fpt = editor
        .element()
        .footprints()
        .first()
        .expect("footprint")
        .uuid();
    let mut fsm = PackageEditorFsm::new(LibraryEditorSettings::default());
    let mut clip = MemoryClipboard::new();
    run_pkg(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.set_footprint(ctx, Some(fpt)));
        let pads: BTreeSet<_> = ctx
            .editor
            .element()
            .footprints()
            .by_uuid(&fpt)
            .expect("fpt")
            .pads()
            .iter()
            .map(|p| FootprintItem::Pad(p.uuid()))
            .collect();
        fsm.set_selection(pads);
        let positions = fsm.move_align_positions(ctx).expect("positions");
        assert_eq!(positions, [mm(0.975, 0.0), mm(-0.975, 0.0)]);
        assert!(fsm.move_align(ctx, &[mm(1.0, 0.0), mm(-1.0, 0.0)]));
        assert!(fsm.flip(ctx, Orientation::Horizontal));
    });
    let f = editor
        .element()
        .footprints()
        .by_uuid(&fpt)
        .expect("fpt")
        .clone();
    let pads: Vec<_> = f.pads().iter().collect();
    // Flipped around the center (0/0): positions swapped, bottom side.
    assert_eq!(pads[0].pad().position(), mm(-1.0, 0.0));
    assert_eq!(pads[1].pad().position(), mm(1.0, 0.0));
    assert!(
        pads.iter()
            .all(|p| p.pad().component_side() == librepcb_core::geometry::ComponentSide::Bottom)
    );
    assert_eq!(editor.history().len(), 2);
    undo_all(&mut editor);
    assert_eq!(editor.element().content(), original);
}

/// DXF import settings with a 10 × 5 mm rectangle and a circle (Ø 1 mm at
/// 5/2.5 mm).
fn dxf_settings(
    dir: &Path,
    layer: librepcb_core::types::Layer,
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
    .expect("write DXF");
    librepcb_editor::fsm::board::DxfImportSettings {
        file: FilePath::new(&file).expect("absolute path"),
        layer,
        line_width: librepcb_core::types::UnsignedLength::default(),
        scale_factor: 1.0,
        join_tangent_polylines: true,
        circles_as_drills: true,
        placement,
    }
}

#[test]
fn import_dxf_into_symbol_and_footprint() {
    use librepcb_core::types::Layer;
    let tmp = tempfile::tempdir().expect("temp dir");

    // Symbol, fixed placement: circles become polygons, one undo step.
    let (_sym_tmp, dir) = copy_element("sym", DIODE_SYMBOL);
    let mut editor: SymbolEditor = open(&dir);
    let polygons = editor.element().polygons().len();
    let mut fsm = SymbolEditorFsm::new(LibraryEditorSettings::default());
    let mut clip = MemoryClipboard::new();
    let settings = dxf_settings(tmp.path(), Layer::SYMBOL_OUTLINES, Some(mm(20.0, 5.0)));
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.import_dxf(ctx, &settings));
    });
    assert!(!editor.is_group_active());
    assert_eq!(editor.element().polygons().len(), polygons + 2);
    assert!(editor.element().polygons().iter().any(|p| {
        p.layer() == Layer::SYMBOL_OUTLINES && p.path().vertices()[0].pos == mm(20.0, 5.0)
    }));
    assert_eq!(editor.history().len(), 1);

    // Footprint, interactive placement: circles become holes.
    let (_pkg_tmp, dir) = copy_element("pkg", RESC2012_PKG);
    let mut editor: PackageEditor = open(&dir);
    let fpt = editor
        .element()
        .footprints()
        .first()
        .expect("footprint")
        .uuid();
    let footprint = |e: &PackageEditor| {
        e.element()
            .footprints()
            .by_uuid(&fpt)
            .expect("footprint")
            .clone()
    };
    let (polygons, holes) = {
        let f = footprint(&editor);
        (f.polygons().len(), f.holes().len())
    };
    let mut fsm = PackageEditorFsm::new(LibraryEditorSettings::default());
    let mut clip = MemoryClipboard::new();
    let settings = dxf_settings(tmp.path(), Layer::TOP_DOCUMENTATION, None);
    run_pkg(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.set_footprint(ctx, Some(fpt)));
        fsm.pointer_moved(ctx, ev(0.0, 0.0));
        assert!(fsm.import_dxf(ctx, &settings));
        fsm.pointer_moved(ctx, ev(1.0, 1.0));
        fsm.left_pressed(ctx, ev(1.0, 1.0));
        fsm.left_released(ctx, ev(1.0, 1.0));
    });
    assert!(!editor.is_group_active());
    let f = footprint(&editor);
    assert_eq!(f.polygons().len(), polygons + 1);
    assert_eq!(f.holes().len(), holes + 1);
    let hole = f.holes().iter().last().expect("hole");
    assert_eq!(hole.diameter().to_mm(), 1.0);

    // A file without objects is an error.
    let empty = tmp.path().join("empty.dxf");
    std::fs::write(&empty, "").expect("write DXF");
    let mut settings = dxf_settings(tmp.path(), Layer::TOP_DOCUMENTATION, None);
    settings.file = FilePath::new(&empty).expect("absolute path");
    let view = ModelHitTester::for_footprint(&footprint(&editor));
    let mut ctx = LibraryContext::new(&mut editor, &view, &mut clip);
    assert!(!fsm.import_dxf(&mut ctx, &settings));
    assert!(
        fsm.take_requests()
            .iter()
            .any(|r| matches!(r, LibraryRequest::ShowError(_)))
    );
}

/// Pasting a single copied pad onto selected pads applies its geometry
/// (upstream `pasteGeometryFromClipboard()`).
#[test]
fn paste_geometry_onto_selected_pads() {
    let (_tmp, dir) = copy_element("pkg", RESC2012_PKG);
    let mut editor: PackageEditor = open(&dir);
    let fpt = editor
        .element()
        .footprints()
        .first()
        .expect("footprint")
        .uuid();
    let pads = |e: &PackageEditor| -> Vec<librepcb_core::geometry::Pad> {
        e.element()
            .footprints()
            .by_uuid(&fpt)
            .expect("footprint")
            .pads()
            .iter()
            .map(|p| p.pad().clone())
            .collect()
    };
    let original = pads(&editor);
    assert_eq!(original.len(), 2);
    let positions: Vec<Point> = original.iter().map(|p| p.position()).collect();

    // The copied pad: another UUID and size.
    let source = editor
        .element()
        .footprints()
        .by_uuid(&fpt)
        .expect("footprint")
        .pads()
        .iter()
        .next()
        .expect("pad")
        .with_uuid(Uuid::new_random());
    let mut source = source;
    let width =
        librepcb_core::types::PositiveLength::new(Length::new(3_000_000)).expect("positive");
    source.pad_mut().set_width(width);
    let mut data = FootprintClipboardData::new(
        Uuid::new_random(),
        editor.element().pads().clone(),
        Point::ORIGIN,
    );
    data.pads.push(source);
    let mut clip = MemoryClipboard::new();
    let settings = LibraryEditorSettings::default();
    clip.set(
        &footprint_clipboard_mime_type(&settings.app_version),
        data.to_bytes().expect("serialize"),
    );
    let mut fsm = PackageEditorFsm::new(settings);
    run_pkg(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        assert!(fsm.set_footprint(ctx, Some(fpt)));
        // Nothing selected: a normal paste would insert the pad.
        assert!(!fsm.can_paste_geometry(ctx));
        fsm.set_selection(original.iter().map(|p| FootprintItem::Pad(p.uuid())));
        assert!(fsm.can_paste_geometry(ctx));
        assert!(fsm.paste(ctx));
    });
    assert!(!editor.is_group_active());
    let after = pads(&editor);
    assert_eq!(after.len(), 2);
    assert!(after.iter().all(|p| p.width() == width));
    // Positions and UUIDs are kept.
    assert_eq!(
        after.iter().map(|p| p.position()).collect::<Vec<_>>(),
        positions
    );
    assert_eq!(editor.history().len(), 1);
    assert_eq!(editor.history()[0].text, "Paste Geometry");
    undo_all(&mut editor);
    assert_eq!(pads(&editor), original);
}

/// Adding an image to a symbol (upstream `SymbolEditorState_AddImage`):
/// placed with a click, sized with the next one; afterwards resized with
/// its handle (upstream `RESIZING_IMAGE`).
#[test]
fn add_and_resize_symbol_image() {
    use librepcb_editor::fsm::schematic::ImageData;
    let (_tmp, dir) = copy_element("sym", DIODE_SYMBOL);
    let mut editor: SymbolEditor = open(&dir);
    let mut fsm = SymbolEditorFsm::new(LibraryEditorSettings::default());
    let mut clip = MemoryClipboard::new();
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"/>"#.to_vec();
    let data = |svg: &[u8]| ImageData {
        data: svg.to_vec(),
        format: "svg".into(),
        basename: "Logo".into(),
    };
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        fsm.pointer_moved(ctx, ev(0.0, 0.0));
        assert!(fsm.add_image(ctx, data(&svg)));
        fsm.pointer_moved(ctx, ev(5.08, 5.08));
        fsm.left_pressed(ctx, ev(5.08, 5.08));
        fsm.left_released(ctx, ev(5.08, 5.08));
        // The size follows the cursor (aspect ratio 2:1).
        fsm.pointer_moved(ctx, ev(15.24, 7.62));
        fsm.left_pressed(ctx, ev(15.24, 7.62));
    });
    assert!(!editor.is_group_active());
    let image = editor
        .element()
        .images()
        .iter()
        .next()
        .expect("image")
        .clone();
    assert_eq!(image.position(), mm(5.08, 5.08));
    assert_eq!(image.width().to_mm(), 10.16);
    assert_eq!(image.height().to_mm(), 5.08);
    assert_eq!(image.file_name().as_str(), "Logo.svg");
    assert_eq!(
        editor
            .element()
            .directory()
            .read_if_exists("Logo.svg")
            .expect("read"),
        Some(svg.clone())
    );
    assert_eq!(editor.history().len(), 1);
    assert_eq!(editor.history()[0].text, "Add Symbol Image");

    // Resize with the handle at the top right corner.
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        fsm.set_selection([SymbolItem::Image(image.uuid())]);
        fsm.left_pressed(ctx, ev(15.24, 10.16));
        fsm.pointer_moved(ctx, ev(25.4, 10.16));
        fsm.left_released(ctx, ev(25.4, 10.16));
    });
    let resized = editor
        .element()
        .images()
        .iter()
        .next()
        .expect("image")
        .clone();
    assert_eq!(resized.position(), mm(5.08, 5.08));
    assert_eq!(resized.width().to_mm(), 20.32);
    assert_eq!(resized.height().to_mm(), 10.16);
    assert_eq!(editor.history().len(), 2);
    assert_eq!(editor.history()[1].text, "Edit Image");

    // The same image again reuses the file; a click at the placed position
    // aborts adding it.
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        fsm.pointer_moved(ctx, ev(0.0, 0.0));
        assert!(fsm.add_image(ctx, data(&svg)));
        fsm.left_pressed(ctx, ev(0.0, 0.0));
        fsm.left_released(ctx, ev(0.0, 0.0));
        fsm.left_pressed(ctx, ev(0.0, 0.0));
    });
    assert!(!editor.is_group_active());
    assert_eq!(editor.element().images().len(), 1);
    assert_eq!(editor.history().len(), 2);
    run_sym(&mut editor, &mut fsm, &mut clip, |fsm, ctx| {
        fsm.pointer_moved(ctx, ev(0.0, 0.0));
        assert!(fsm.add_image(ctx, data(&svg)));
        fsm.left_pressed(ctx, ev(0.0, 0.0));
        fsm.pointer_moved(ctx, ev(2.54, 0.0));
        fsm.left_pressed(ctx, ev(2.54, 0.0));
    });
    let names: Vec<String> = editor
        .element()
        .images()
        .iter()
        .map(|i| i.file_name().to_string())
        .collect();
    assert_eq!(names, ["Logo.svg", "Logo.svg"]);
}
