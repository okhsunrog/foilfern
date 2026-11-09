//! Port of the upstream `BoardSpecctraExportTest`
//! (tests/unittests/core/project/board/boardspecctraexporttest.cpp): the
//! "Gerber Test" project is exported and compared with the expected DSN file
//! of the upstream test data. In addition, the output is parsed with the
//! independent `topola_specctra` parser.

use std::io::BufReader;

use librepcb_core::project::board::BoardSpecctraExport;

use super::plane_fragments_builder_test::open_project;
use crate::helpers::test_data_dir;

fn expected_dir() -> std::path::PathBuf {
    test_data_dir().join("unittests/librepcbproject/BoardSpecctraExportTest")
}

fn export(dir: &str, lpp: &str) -> String {
    let project = open_project(dir, lpp);
    let board = project.boards()[0].id();
    let dsn = BoardSpecctraExport::new(&project, board)
        .unwrap()
        .with_host("LibrePCB-UnitTests", "0")
        .generate()
        .unwrap();
    String::from_utf8(dsn).unwrap()
}

#[test]
fn test() {
    let actual = export("Gerber Test", "project.lpp");
    let expected = std::fs::read_to_string(expected_dir().join("expected.dsn")).unwrap();
    if actual != expected {
        let out = std::env::temp_dir().join("librepcb-rs-specctra-actual.dsn");
        std::fs::write(&out, &actual).unwrap();
        panic!(
            "DSN differs from upstream expectation, actual output written to {}",
            out.display()
        );
    }
}

/// Parses a DSN file with the independent `topola_specctra` parser.
fn parse_with_topola(dsn: &str) -> specctra::structure::Pcb {
    let mut tokenizer = specctra::read::ListTokenizer::new(BufReader::new(dsn.as_bytes()));
    let file: specctra::structure::DsnFile = tokenizer.read_value().unwrap();
    file.pcb
}

#[test]
fn topola_parses_export() {
    // Note: `topola_specctra` requires the children of an image in a fixed
    // order (outlines, pins, keepouts), while the export (like upstream)
    // writes footprint cutouts before the pins, so the "Gerber Test" board
    // (with footprint cutouts) cannot be checked with it.
    for (dir, lpp) in [("Nested Planes", "project.lpp"), ("DRC", "project.lpp")] {
        let project = open_project(dir, lpp);
        let board = &project.boards()[0];
        let dsn = BoardSpecctraExport::new(&project, board.id())
            .unwrap()
            .generate()
            .unwrap();
        let pcb = parse_with_topola(std::str::from_utf8(&dsn).unwrap());
        assert_eq!(
            pcb.structure.layers.len(),
            board.copper_layers().len(),
            "{dir}"
        );
        // The dummy "BOARD" component and one per device.
        assert_eq!(
            pcb.placement.components.len(),
            board.devices().len() + 1,
            "{dir}"
        );
        assert_eq!(pcb.library.images.len(), board.devices().len() + 1);
        // All nets and one dummy net per net segment without net.
        let mut expected: Vec<String> = project
            .circuit()
            .net_signals()
            .values()
            .map(|n| n.name().to_string())
            .collect();
        expected.extend(
            board
                .net_segments()
                .values()
                .filter(|s| s.net().is_none())
                .map(|s| format!("~anonymous~{}", s.uuid())),
        );
        let actual: Vec<String> = pcb.network.nets.iter().map(|n| n.name.clone()).collect();
        assert_eq!(actual, expected, "{dir}");
        let traces: usize = board
            .net_segments()
            .values()
            .map(|s| s.traces().len())
            .sum();
        let vias: usize = board.net_segments().values().map(|s| s.vias().len()).sum();
        assert_eq!(pcb.wiring.wires.len(), traces, "{dir}");
        assert_eq!(pcb.wiring.vias.len(), vias, "{dir}");
        for via in &pcb.wiring.vias {
            assert!(pcb.library.find_padstack_by_name(&via.name).is_some());
        }
    }
}
