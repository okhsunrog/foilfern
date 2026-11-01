//! Port of tests/unittests/core/project/board/boardgerberexporttest.cpp.
use librepcb_core::export::{BoardSide, Timestamp};
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::board::BoardGerberExport;
use librepcb_core::project::{AssemblyVariantId, Project, ProjectLoader};
use std::{path::Path, sync::Arc};

fn load(name: &str) -> Project {
    let root = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects")
        .join(name);
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(root).unwrap()).unwrap();
    ProjectLoader::new()
        .open(TransactionalDirectory::new(Arc::new(fs), ""), "project.lpp")
        .unwrap()
}
fn normalize(text: &str) -> String {
    let software = regex::Regex::new(r"TF\.GenerationSoftware,LibrePCB,LibrePCB,[^\s\*]*").unwrap();
    let date = regex::Regex::new(r"TF\.CreationDate,[^\s\*]*").unwrap();
    let md5 = regex::Regex::new(r".*TF\.MD5,.*").unwrap();
    md5.replace_all(
        &date.replace_all(
            &software.replace_all(text, "TF.GenerationSoftware,LibrePCB,LibrePCB,0.1.2"),
            "TF.CreationDate,2019-01-02T03:04:05",
        ),
        "",
    )
    .into_owned()
}
#[test]
fn upstream_golden_files() {
    let mut project = load("Gerber Test");
    let board_id = project.boards()[0].id();
    project.rebuild_planes(board_id, None).unwrap();
    let board = project.board(board_id).unwrap();
    let exporter = BoardGerberExport::new(&project, board, "0.1.2", Timestamp::now());
    let mut config = board.settings().fabrication_output_settings.clone();
    config.output_base_path = "{{PROJECT}}".into();
    let mut files = exporter.generate_pcb_layers(&config).unwrap();
    let variant = AssemblyVariantId(
        project
            .circuit()
            .assembly_variants()
            .first()
            .unwrap()
            .uuid(),
    );
    for (side, suffix) in [(BoardSide::Top, "TOP"), (BoardSide::Bottom, "BOTTOM")] {
        files.insert(
            format!("test_project_GLUE-{suffix}.gbr"),
            exporter.generate_glue_layer(side, variant).unwrap(),
        );
        files.insert(
            format!("test_project_ASSEMBLY-{suffix}.gbr"),
            exporter.generate_component_layer(side, variant).unwrap(),
        );
    }
    let expected_root = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/unittests/librepcbproject/BoardGerberExportTest/expected");
    assert_eq!(
        files.len(),
        std::fs::read_dir(&expected_root).unwrap().count()
    );
    for (name, content) in files {
        let expected = std::fs::read_to_string(expected_root.join(&name)).unwrap();
        let actual = normalize(&content);
        if actual != expected {
            let line = actual
                .lines()
                .zip(expected.lines())
                .position(|(a, e)| a != e)
                .unwrap_or(actual.lines().count().min(expected.lines().count()));
            panic!(
                "{name}: first different line {}: actual {:?}, expected {:?} ({} / {} bytes)",
                line + 1,
                actual.lines().nth(line),
                expected.lines().nth(line),
                actual.len(),
                expected.len()
            );
        }
    }
}

#[test]
fn curved_slot_g85_error_and_filename_collisions() {
    let mut project = load("Gerber Test");
    let id = project.boards()[0].id();
    project.rebuild_planes(id, None).unwrap();
    let board = project.board(id).unwrap();
    let revision = project.revision();
    let exporter = BoardGerberExport::new(&project, board, "0.1.2", Timestamp::now());
    let mut config = board.settings().fabrication_output_settings.clone();
    config.use_g85_slot_command = true;
    assert!(matches!(
        exporter.generate_pcb_layers(&config),
        Err(librepcb_core::project::Error::Export(
            librepcb_core::export::Error::CurvedSlotG85
        ))
    ));
    config.use_g85_slot_command = false;
    config.output_base_path = "{{PROJECT}}".into();
    let separate = exporter.generate_pcb_layers(&config).unwrap();
    // A late top paste output must overwrite an earlier bottom mask output.
    config.suffix_solder_mask_bot = config.suffix_solder_paste_top.clone();
    let collided = exporter.generate_pcb_layers(&config).unwrap();
    let name = format!("test_project{}", config.suffix_solder_paste_top);
    assert_eq!(collided[&name], separate[&name]);
    assert_eq!(collided.len() + 1, separate.len());
    assert_eq!(project.revision(), revision);
}

#[test]
fn disabled_masks_legends_and_paste_are_absent() {
    let project = load("Gerber Test");
    let mut board = project.boards()[0].clone();
    let mut settings = board.settings().clone();
    settings.solder_resist = None;
    settings.silkscreen_layers_top.clear();
    settings.silkscreen_layers_bot.clear();
    settings.fabrication_output_settings.enable_solder_paste_top = false;
    settings.fabrication_output_settings.enable_solder_paste_bot = false;
    settings.fabrication_output_settings.output_base_path = "test".into();
    board.set_settings(settings);
    let exporter = BoardGerberExport::new(&project, &board, "0.1.2", Timestamp::now());
    let files = exporter
        .generate_pcb_layers(&board.settings().fabrication_output_settings)
        .unwrap();
    assert!(
        files
            .keys()
            .all(|name| !name.contains("SOLDER") && !name.contains("SILKSCREEN"))
    );
    assert!(files.keys().any(|name| name.contains("COPPER")));
    assert!(files.keys().any(|name| name.contains("DRILLS")));
}
