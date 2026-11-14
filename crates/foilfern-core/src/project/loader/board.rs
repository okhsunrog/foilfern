//! Board part of the project loader (upstream
//! `ProjectLoader::loadBoards()`, `loadBoard()`,
//! `loadBoardDeviceInstance()`, `loadBoardNetSegment()`,
//! `loadBoardPlane()`, `loadBoardUserSettings()`).
//!
//! Each board is deserialized into a complete value first (rejecting
//! duplicate item UUIDs with the upstream messages) and then added to the
//! project in one step, which validates all items against the circuit and
//! the library (anchor resolution, pad nets and layers, cohesion, ...) with
//! the upstream messages. Afterwards the air wires are built (upstream
//! `forceAirWiresRebuild()` in `Board::addToProject()`).

use std::collections::BTreeSet;

use super::{parse_file, split_index_path};
use crate::project::Project;
use crate::project::board::{Board, BoardItemKind};
use crate::project::error::{EntityKind, Error, Result};
use crate::serialization::SExpression;
use crate::types::Uuid;

pub(super) fn load_boards(p: &mut Project, auto_assign_device_models: bool) -> Result<()> {
    let index = parse_file(p, "boards/boards.lp")?;
    for node in index.children_named("board") {
        let relative: String = node.child_value("@0")?;
        load_board(p, &relative, auto_assign_device_models)?;
    }
    log::debug!("Successfully loaded {} boards.", p.boards.len());
    Ok(())
}

fn load_board(p: &mut Project, relative_file_path: &str, auto_assign_models: bool) -> Result<()> {
    let (dir, directory_name) = split_index_path(relative_file_path);
    let root = parse_file(p, relative_file_path)?;
    check_unique_items(&root)?;
    let mut board = Board::deserialize(&root, &directory_name)?;
    if auto_assign_models {
        auto_assign_device_models(p, &mut board);
    }
    load_user_settings(p, &mut board, &dir);
    let id = p.add_board(board, None)?;
    p.rebuild_air_wires(id)?;
    Ok(())
}

/// Rejects duplicate UUIDs (upstream: the `add*()` methods of `Board`,
/// `BI_Device` and `BI_NetSegment`).
fn check_unique_items(root: &SExpression) -> Result<()> {
    let check = |parent: &SExpression, name: &str, error: &dyn Fn(Uuid) -> Error| -> Result<()> {
        let mut seen = BTreeSet::new();
        for node in parent.children_named(name) {
            let uuid: Uuid = node.child_value("@0")?;
            if !seen.insert(uuid) {
                return Err(error(uuid));
            }
        }
        Ok(())
    };
    let item = |kind: BoardItemKind| {
        move |uuid| Error::DuplicateUuid {
            kind: kind.into(),
            uuid,
        }
    };
    check(root, "device", &Error::DuplicateDevice)?;
    for device in root.children_named("device") {
        check(device, "stroke_text", &item(BoardItemKind::StrokeText))?;
    }
    check(root, "netsegment", &|uuid| Error::DuplicateUuid {
        kind: EntityKind::NetSegment,
        uuid,
    })?;
    for segment in root.children_named("netsegment") {
        check(segment, "pad", &item(BoardItemKind::Pad))?;
        check(segment, "via", &item(BoardItemKind::Via))?;
        check(segment, "junction", &item(BoardItemKind::Junction))?;
        check(segment, "trace", &item(BoardItemKind::Trace))?;
    }
    check(root, "plane", &|uuid| Error::DuplicateUuid {
        kind: EntityKind::Plane,
        uuid,
    })?;
    check(root, "zone", &item(BoardItemKind::Zone))?;
    check(root, "polygon", &item(BoardItemKind::Polygon))?;
    check(root, "stroke_text", &item(BoardItemKind::StrokeText))?;
    check(root, "hole", &item(BoardItemKind::Hole))?;
    Ok(())
}

/// Assigns the default 3D model to devices without a valid model (upstream
/// `mAutoAssignDeviceModels`, used after upgrading the project library).
fn auto_assign_device_models(p: &Project, board: &mut Board) {
    for device in board.devices.values_mut() {
        let Some(lib_device) = p.library.device(&device.lib_device()) else {
            continue; // Reported when the board is added.
        };
        let Some(package) = p.library.package(&lib_device.package_uuid()) else {
            continue;
        };
        let Some(footprint) = package.footprints().by_uuid(&device.lib_footprint()) else {
            continue;
        };
        let valid = device
            .lib_model()
            .is_some_and(|m| package.models().contains_uuid(&m) && footprint.models().contains(&m));
        if !valid {
            let model = device.default_lib_model(package);
            device.set_lib_model(model);
        }
    }
}

/// Loads `settings.user.lp`; errors are logged and the defaults used
/// (these files are usually not under version control).
fn load_user_settings(p: &Project, board: &mut Board, dir: &str) {
    let result = parse_file(p, &format!("{dir}/settings.user.lp"))
        .and_then(|root| Ok(board.load_user_settings(&root)?));
    if let Err(e) = result {
        log::error!("Could not load board user settings, defaults will be used instead: {e}");
    }
}
