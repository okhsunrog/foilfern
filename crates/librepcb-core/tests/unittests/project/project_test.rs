//! Project level: creation, metadata/settings/approvals, schematics and
//! boards, saving and reopening.

use std::collections::BTreeSet;
use std::sync::Arc;

use librepcb_core::fileio::{FileSystem, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::job::{GerberExcellonOutputJob, default_output_jobs};
use librepcb_core::project::board::{Board, BoardProperties};
use librepcb_core::project::schematic::SchematicProperties;
use librepcb_core::project::{BoardId, EntityKind, Error, Mutation, SchematicId};
use librepcb_core::serialization::SExpression;
use librepcb_core::types::{ElementName, FileProofName, Uuid};

use super::*;
use crate::helpers::TempDir;

#[test]
fn create() {
    let p = new_project();
    assert_eq!(p.metadata().name.as_str(), "Unnamed");
    assert_eq!(p.metadata().version.as_str(), "v1");
    assert_eq!(p.file_name(), "test.lpp");
    assert_eq!(p.circuit().assembly_variants().len(), 1);
    assert_eq!(
        p.circuit()
            .assembly_variants()
            .first()
            .unwrap()
            .name()
            .as_str(),
        "Std"
    );
    assert!(p.circuit().net_class_by_name("default").is_some());
    assert_eq!(p.revision(), 0);
    assert!(p.schematics().is_empty());
    assert!(p.primary_board().is_none());
    assert!(p.is_ref_index_consistent());

    assert!(matches!(
        Project::new(
            TransactionalDirectory::new_temporary().unwrap(),
            "test.txt",
            Uuid::new_random()
        ),
        Err(Error::InvalidProjectFileSuffix)
    ));
}

#[test]
fn metadata_settings_approvals() {
    let mut p = new_project();
    let mut metadata = p.metadata().clone();
    metadata.name = ElementName::new("My Project").unwrap();
    metadata.author = "me".into();
    metadata.version = FileProofName::new("v2").unwrap();
    assert_inverse(&mut p, Mutation::SetProjectMetadata(metadata));

    let settings = ProjectSettings {
        locale_order: vec!["de_DE".into(), "en_US".into()],
        norm_order: vec!["IEC 60617".into()],
        custom_bom_attributes: vec!["SUPPLIER".into()],
        default_lock_component_assembly: true,
    };
    assert_inverse(&mut p, Mutation::SetProjectSettings(settings));

    let jobs = default_output_jobs(GerberExcellonOutputJob::default(), Vec::new());
    assert_inverse(&mut p, Mutation::SetOutputJobs(jobs.clone()));
    p.apply(Mutation::SetOutputJobs(jobs.clone())).unwrap();
    assert_eq!(p.output_jobs(), &jobs);

    let approval = SExpression::list("approved");
    let inverse = assert_inverse(
        &mut p,
        Mutation::SetErcApproval {
            approval: approval.clone(),
            approved: true,
        },
    );
    assert_eq!(
        inverse,
        Mutation::SetErcApproval {
            approval: approval.clone(),
            approved: false,
        }
    );
    assert_inverse(
        &mut p,
        Mutation::SetErcApprovals(BTreeSet::from([approval])),
    );
}

#[test]
fn schematics() {
    let mut p = new_project();
    let main = Schematic::new(
        Uuid::new_random(),
        ElementName::new("Main").unwrap(),
        "main",
    );
    let main_id = main.id();
    assert_inverse(
        &mut p,
        Mutation::AddSchematic {
            schematic: main.clone(),
            index: None,
        },
    );
    p.add_schematic(main.clone(), None).unwrap();
    let power = Schematic::new(
        Uuid::new_random(),
        ElementName::new("Power").unwrap(),
        "power",
    );
    let power_id = power.id();
    // Insert in front, removal restores the page index.
    assert_inverse(
        &mut p,
        Mutation::AddSchematic {
            schematic: power.clone(),
            index: Some(0),
        },
    );
    p.add_schematic(power, Some(0)).unwrap();
    assert_eq!(p.schematic_index(power_id), Some(0));
    assert_eq!(p.schematic_index(main_id), Some(1));
    assert_eq!(
        p.schematic_by_name("Main").map(Schematic::id),
        Some(main_id)
    );

    let clash = Schematic::new(
        Uuid::new_random(),
        ElementName::new("Main").unwrap(),
        "other",
    );
    assert!(matches!(
        assert_fails(
            &mut p,
            Mutation::AddSchematic {
                schematic: clash,
                index: None
            }
        ),
        Error::DuplicateName {
            kind: EntityKind::Schematic,
            ..
        }
    ));
    let clash = Schematic::new(
        Uuid::new_random(),
        ElementName::new("Other").unwrap(),
        "main",
    );
    assert!(matches!(
        assert_fails(
            &mut p,
            Mutation::AddSchematic {
                schematic: clash,
                index: None
            }
        ),
        Error::DuplicateDirectoryName {
            kind: EntityKind::Schematic,
            ..
        }
    ));
    assert!(matches!(
        assert_fails(
            &mut p,
            Mutation::AddSchematic {
                schematic: main,
                index: None
            }
        ),
        Error::DuplicateUuid {
            kind: EntityKind::Schematic,
            ..
        }
    ));

    let mut props = p.schematic(main_id).unwrap().properties();
    props.name = ElementName::new("Power").unwrap();
    assert!(matches!(
        assert_fails(&mut p, Mutation::UpdateSchematic(props)),
        Error::DuplicateName { .. }
    ));
    let props = SchematicProperties {
        id: main_id,
        name: ElementName::new("Main Page").unwrap(),
        grid_interval: p.schematic(main_id).unwrap().grid_interval(),
        grid_unit: librepcb_core::types::LengthUnit::Mils,
    };
    assert_inverse(&mut p, Mutation::UpdateSchematic(props));

    assert_inverse(&mut p, Mutation::RemoveSchematic(power_id));
    assert!(matches!(
        assert_fails(&mut p, Mutation::RemoveSchematic(SchematicId::new_random())),
        Error::NotFound {
            kind: EntityKind::Schematic,
            ..
        }
    ));
}

#[test]
fn boards() {
    let mut p = new_project();
    let board = Board::new(
        Uuid::new_random(),
        ElementName::new("default").unwrap(),
        "default",
    );
    let id = board.id();
    assert_inverse(
        &mut p,
        Mutation::AddBoard {
            board: board.clone(),
            index: None,
        },
    );
    p.add_board(board.clone(), None).unwrap();
    assert_eq!(p.primary_board().map(Board::id), Some(id));
    let second = Board::new(
        Uuid::new_random(),
        ElementName::new("second").unwrap(),
        "second",
    );
    let second_id = second.id();
    p.add_board(second, Some(0)).unwrap();
    assert_eq!(p.primary_board().map(Board::id), Some(second_id));
    assert!(matches!(
        assert_fails(&mut p, Mutation::AddBoard { board, index: None }),
        Error::DuplicateUuid {
            kind: EntityKind::Board,
            ..
        }
    ));
    assert_inverse(
        &mut p,
        Mutation::UpdateBoard(BoardProperties {
            id,
            name: ElementName::new("renamed").unwrap(),
        }),
    );
    assert!(matches!(
        assert_fails(
            &mut p,
            Mutation::UpdateBoard(BoardProperties {
                id,
                name: ElementName::new("second").unwrap(),
            })
        ),
        Error::DuplicateName {
            kind: EntityKind::Board,
            ..
        }
    ));
    assert_inverse(&mut p, Mutation::RemoveBoard(second_id));
    assert!(matches!(
        assert_fails(&mut p, Mutation::RemoveBoard(BoardId::new_random())),
        Error::NotFound {
            kind: EntityKind::Board,
            ..
        }
    ));
}

#[test]
fn save_and_reopen() {
    let temp = TempDir::new();
    let fs = Arc::new(TransactionalFileSystem::open_rw(temp.path()).unwrap());
    let mut p = Project::create(
        TransactionalDirectory::new(Arc::clone(&fs), ""),
        "test.lpp",
        Uuid::new_random,
    )
    .unwrap();
    let gnd = add_net(&mut p, "GND");
    add_component(&mut p, "R1", Some(gnd));
    p.add_schematic(
        Schematic::new(
            Uuid::new_random(),
            ElementName::new("Main").unwrap(),
            "main",
        ),
        None,
    )
    .unwrap();
    p.add_board(
        Board::new(
            Uuid::new_random(),
            ElementName::new("default").unwrap(),
            "default",
        ),
        None,
    )
    .unwrap();
    let before = snapshot(&p);
    p.save().unwrap();
    fs.save().unwrap();
    drop(p);
    fs.release_lock().unwrap();

    let fs = Arc::new(TransactionalFileSystem::open_ro(temp.path()).unwrap());
    let directory = TransactionalDirectory::new(Arc::clone(&fs), "");
    assert!(directory.file_exists("project/metadata.lp"));
    assert!(directory.file_exists("circuit/circuit.lp"));
    assert!(directory.file_exists("schematics/main/schematic.lp"));
    assert!(directory.file_exists("boards/default/board.lp"));
    assert_eq!(directory.dirs("library/cmp").len(), 1);
    let reopened = Project::open(directory, "test.lpp").unwrap();
    assert_eq!(reopened.revision(), 0);
    assert!(reopened.is_ref_index_consistent());
    let after = snapshot(&reopened);
    assert_eq!(after.circuit, before.circuit);
    assert_eq!(after.metadata, before.metadata);
    assert_eq!(after.settings, before.settings);
    assert_eq!(after.library, before.library);
    assert_eq!(
        after
            .schematics
            .iter()
            .map(Schematic::id)
            .collect::<Vec<_>>(),
        before
            .schematics
            .iter()
            .map(Schematic::id)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        after.boards.iter().map(Board::id).collect::<Vec<_>>(),
        before.boards.iter().map(Board::id).collect::<Vec<_>>()
    );

    // Saving again is stable (byte-identical).
    let mut reopened = reopened;
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
