//! Ports of the upstream `BoardGerberExportTest`, `BoardPickPlaceGeneratorTest`
//! and `BoardD356NetlistExportTest`
//! (tests/unittests/core/project/board/board{gerberexport,pickplacegenerator,d356netlistexport}test.cpp):
//! the "Gerber Test" project is exported and compared with the expected
//! files in the upstream test data (volatile data like the generation
//! software and date is replaced by constants, like upstream).

use chrono::NaiveDate;
use librepcb_core::export::{BoardSide, PickPlaceCsvWriter, PickPlaceSides, Timestamp};
use librepcb_core::fileio::FilePath;
use librepcb_core::project::AssemblyVariantId;
use librepcb_core::project::board::{
    BoardD356NetlistExport, BoardGerberExport, BoardPickPlaceGenerator, ExportInfo,
};
use regex::Regex;

use super::plane_fragments_builder_test::open_project;
use crate::helpers::{TempDir, test_data_dir};

/// The constant values upstream's tests write into the files.
fn export_info() -> ExportInfo {
    ExportInfo {
        app_version: "0.1.2".to_owned(),
        creation_date: Timestamp::Local(
            NaiveDate::from_ymd_opt(2019, 1, 2)
                .unwrap()
                .and_hms_opt(3, 4, 5)
                .unwrap(),
        ),
    }
}

fn expected_dir(test: &str) -> std::path::PathBuf {
    test_data_dir().join("unittests/librepcbproject").join(test)
}

#[test]
fn gerber_export() {
    let tmp = TempDir::new();
    let mut project = open_project("Gerber Test", "project.lpp");
    let board = project.boards()[0].id();
    let av = AssemblyVariantId(project.circuit().assembly_variants()[0].uuid());

    // Force planes rebuild.
    project.rebuild_planes(board, None).unwrap();

    // Export fabrication data.
    let mut config = project
        .board(board)
        .unwrap()
        .settings()
        .fabrication_output_settings
        .clone();
    config.output_base_path = format!("{}/{{{{PROJECT}}}}", tmp.path().as_str());
    let mut export = BoardGerberExport::new(&project, board, &export_info()).unwrap();
    export.export_pcb_layers(&config).unwrap();
    let file = |name: &str| tmp.path().path_to(name);
    export
        .export_glue_layer(BoardSide::Top, av, &file("test_project_GLUE-TOP.gbr"))
        .unwrap();
    export
        .export_glue_layer(BoardSide::Bottom, av, &file("test_project_GLUE-BOTTOM.gbr"))
        .unwrap();
    export
        .export_component_layer(BoardSide::Top, av, &file("test_project_ASSEMBLY-TOP.gbr"))
        .unwrap();
    export
        .export_component_layer(
            BoardSide::Bottom,
            av,
            &file("test_project_ASSEMBLY-BOTTOM.gbr"),
        )
        .unwrap();

    // Compare generated files with expected content (without MD5 line).
    let md5 = Regex::new(r"(?m)^.*TF\.MD5,.*$").unwrap();
    let written = export.written_files().to_vec();
    assert_eq!(written.len(), 15);
    let mut failures = Vec::new();
    for fp in &written {
        let actual = std::fs::read_to_string(fp.as_path()).unwrap();
        let actual = md5.replace_all(&actual, "").into_owned();
        let expected = std::fs::read_to_string(
            expected_dir("BoardGerberExportTest/expected").join(fp.file_name()),
        )
        .unwrap();
        if actual != expected {
            failures.push(fp.file_name().to_owned());
        }
    }
    assert!(failures.is_empty(), "files differ: {failures:?}");
}

#[test]
fn pick_place_generator() {
    let tmp = TempDir::new();
    let project = open_project("Gerber Test", "project.lpp");
    let board = project.boards()[0].id();
    let av = AssemblyVariantId(project.circuit().assembly_variants()[0].uuid());
    let data = BoardPickPlaceGenerator::new(&project, board, av)
        .unwrap()
        .generate()
        .unwrap();
    let mut writer = PickPlaceCsvWriter::new(&data, "0.1.2");
    let mut written: Vec<FilePath> = Vec::new();

    // Top devices with comment.
    writer.set_board_sides(PickPlaceSides::Top);
    writer.set_include_metadata_comment(true);
    let fp = tmp.path().path_to("top.csv");
    writer.generate_csv().unwrap().save_to_file(&fp).unwrap();
    written.push(fp);

    // Bottom devices with comment.
    writer.set_board_sides(PickPlaceSides::Bottom);
    writer.set_include_metadata_comment(true);
    let fp = tmp.path().path_to("bottom.csv");
    writer.generate_csv().unwrap().save_to_file(&fp).unwrap();
    written.push(fp);

    // Top+bottom devices without comment.
    writer.set_board_sides(PickPlaceSides::Both);
    writer.set_include_metadata_comment(false);
    let fp = tmp.path().path_to("both.csv");
    writer.generate_csv().unwrap().save_to_file(&fp).unwrap();
    written.push(fp);

    // Replace volatile data in exported files with well-known, constant data.
    let software = Regex::new("Generation Software:(.*)").unwrap();
    let date = Regex::new("Generation Date:(.*)").unwrap();
    for fp in &written {
        let actual = std::fs::read_to_string(fp.as_path()).unwrap();
        let actual = software.replace_all(&actual, "Generation Software:");
        let actual = date.replace_all(&actual, "Generation Date:");
        let expected = std::fs::read_to_string(
            expected_dir("BoardPickPlaceGeneratorTest/expected").join(fp.file_name()),
        )
        .unwrap();
        assert_eq!(expected, actual, "{}", fp.file_name());
    }
}

#[test]
fn d356_netlist_export() {
    let project = open_project("Gerber Test", "project.lpp");
    let board = project.boards()[0].id();
    let content = BoardD356NetlistExport::new(&project, board, &export_info())
        .unwrap()
        .generate()
        .unwrap();

    // Replace volatile data with well-known, constant data.
    let software = Regex::new("Generation Software: LibrePCB (.*)").unwrap();
    let date = Regex::new("Generation Date: (.*)").unwrap();
    let content = software.replace_all(&content, "Generation Software:");
    let content = date.replace_all(&content, "Generation Date:");
    let expected =
        std::fs::read_to_string(expected_dir("BoardD356NetlistExportTest/expected.d356")).unwrap();
    assert_eq!(expected, content);
}
