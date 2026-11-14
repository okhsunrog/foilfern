//! Saving and loading boards: byte-stable round trip through the project
//! loader (including the user settings) and loader errors.

use std::collections::BTreeMap;
use std::sync::Arc;

use foilfern_core::fileio::{FileSystem, TransactionalDirectory, TransactionalFileSystem};
use foilfern_core::project::board::BoardPlane;
use foilfern_core::project::{BoardMutation, Error, Mutation};
use foilfern_core::serialization::{List, Mode, SExpression, SerializeObject};

use super::*;
use crate::helpers::TempDir;

/// Creates the fixture with devices, a segment, a plane and items in a
/// project on disk and saves it.
fn saved_fixture(temp: &TempDir) -> Fixture {
    let fs = Arc::new(TransactionalFileSystem::open_rw(temp.path()).unwrap());
    let p = Project::create(
        TransactionalDirectory::new(Arc::clone(&fs), ""),
        "test.lpp",
        Uuid::new_random,
    )
    .unwrap();
    let mut f = Fixture::build(p);
    let board = f.board;
    for (component, pos) in [(f.r1, pt(0.0, 0.0)), (f.r2, pt(10.0, 0.0))] {
        let lib_device = f.p.library().device(&f.lib_device).unwrap();
        let package = f.p.library().package(&f.lib_package).unwrap();
        let device = foilfern_core::project::board::BoardDevice::from_library(
            component,
            lib_device,
            package,
            f.footprint,
            pos,
            Angle::DEG0,
            false,
            false,
            true,
            true,
        )
        .unwrap();
        f.p.add_board_device(board, device).unwrap();
    }
    let (segment, _) = gnd_segment(&f);
    f.p.add_board_net_segment(board, segment).unwrap();
    let mut plane = BoardPlane::new(
        Uuid::new_random(),
        Layer::BOT_COPPER,
        Some(f.gnd),
        Path::rect(pt(0.0, 0.0), pt(10.0, 10.0)),
    );
    plane.set_visible(false);
    plane.set_locked(true);
    f.p.add_board_plane(board, plane).unwrap();
    for item in super::mutation_test::sample_items() {
        f.p.add_board_item(board, item).unwrap();
    }
    f.p.apply(Mutation::Board(BoardMutation::SetLayersVisibility {
        board,
        visibility: BTreeMap::from([("top_cu".to_owned(), false), ("bot_cu".to_owned(), true)]),
    }))
    .unwrap();
    f.p.save().unwrap();
    fs.save().unwrap();
    f
}

#[test]
fn save_and_reopen() {
    let temp = TempDir::new();
    let f = saved_fixture(&temp);
    let before = f.board().clone();
    drop(f);
    let fs = Arc::new(TransactionalFileSystem::open_ro(temp.path()).unwrap());
    let directory = TransactionalDirectory::new(Arc::clone(&fs), "");
    let user =
        String::from_utf8(directory.read("boards/default/settings.user.lp").unwrap()).unwrap();
    assert!(user.contains("(layer top_cu (visible false))"));
    assert!(user.contains("(visible false)"));
    let mut reopened = Project::open(directory, "test.lpp").unwrap();
    assert!(reopened.is_ref_index_consistent());
    assert_eq!(reopened.boards()[0], before);
    assert_eq!(reopened.boards()[0].layers_visibility().len(), 2);
    assert!(
        !reopened.boards()[0]
            .planes()
            .values()
            .next()
            .unwrap()
            .visible()
    );
    // The devices got the footprint's stroke texts: none in the fixture.
    // Saving again is byte-identical.
    reopened.save().unwrap();
    assert!(
        reopened
            .directory()
            .file_system()
            .check_for_modifications()
            .unwrap()
            .is_empty()
    );
}

/// Serializes the board file of a board value (for comparisons).
fn board_bytes(board: &foilfern_core::project::board::Board) -> Vec<u8> {
    let mut root = List::new("librepcb_board");
    board.serialize(&mut root);
    SExpression::from(root)
        .to_byte_array(Mode::LibrePcb)
        .unwrap()
}

#[test]
fn deserialize_roundtrip() {
    let temp = TempDir::new();
    let f = saved_fixture(&temp);
    let bytes = board_bytes(f.board());
    let node = SExpression::parse(&bytes, None, Mode::LibrePcb).unwrap();
    let mut board = foilfern_core::project::board::Board::deserialize(&node, "default").unwrap();
    assert_eq!(board_bytes(&board), bytes);
    // The visibility of planes and layers comes from the user settings.
    assert!(board.planes().values().all(|p| p.visible()));
    board
        .load_user_settings(
            &SExpression::parse(
                b"(librepcb_board_user_settings (layer top_cu (visible false)))",
                None,
                Mode::LibrePcb,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(board.layers_visibility().get("top_cu"), Some(&false));
}

/// Saves the fixture, applies the replacement returned by `edit` to the
/// board file and returns the error of reopening the project.
fn open_error(edit: impl Fn(&Fixture) -> (String, String)) -> String {
    let temp = TempDir::new();
    let f = saved_fixture(&temp);
    let (from, to) = edit(&f);
    drop(f);
    let path = temp.path().path_to("boards/default/board.lp");
    let content = std::fs::read_to_string(path.as_path()).unwrap();
    assert!(content.contains(&from), "{from}");
    std::fs::write(path.as_path(), content.replacen(&from, &to, 1)).unwrap();
    let fs = Arc::new(TransactionalFileSystem::open_ro(temp.path()).unwrap());
    let result: Result<Project, Error> =
        Project::open(TransactionalDirectory::new(fs, ""), "test.lpp");
    result.err().unwrap().to_string()
}

#[test]
fn loader_errors() {
    // A trace to an inexistent junction.
    let error = open_error(|f| {
        let segment = f.board().net_segments().values().next().unwrap();
        let junction = segment.junctions().keys().next().unwrap();
        (
            format!("(junction {junction})"),
            format!("(junction {})", Uuid::new_random()),
        )
    });
    assert!(error.contains("does not exist in schematic."), "{error}");
    // A device of an inexistent component.
    let error = open_error(|f| {
        (
            format!("(device {}", f.r1),
            format!("(device {}", Uuid::new_random()),
        )
    });
    assert!(error.contains("does not exist in the circuit."), "{error}");
    // An inexistent net.
    let error = open_error(|f| {
        (
            format!("(net {})", f.gnd),
            format!("(net {})", Uuid::new_random()),
        )
    });
    assert!(error.starts_with("Inexistent net signal"), "{error}");
    // A duplicate device.
    let error = open_error(|f| (format!("(device {}", f.r2), format!("(device {}", f.r1)));
    assert!(
        error.starts_with("There is already a device with the component instance"),
        "{error}"
    );
    // An invalid plane connect style.
    let error = open_error(|_| {
        (
            "(connect_style thermal)".into(),
            "(connect_style foo)".into(),
        )
    });
    assert!(
        error.contains("Unknown plane connect style: 'foo'"),
        "{error}"
    );
}
