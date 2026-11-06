//! Port of the implemented subset of BoardDesignRuleCheckTest::testMessages.
//! Upstream approvals remain the oracle; test data is never copied here.

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::board::drc::{BoardDesignRuleCheck, BoardDrcData};
use librepcb_core::project::{Project, ProjectLoader};
use librepcb_core::serialization::SExpression;
use std::{collections::BTreeSet, path::Path, sync::Arc};

fn load(name: &str) -> Project {
    let root = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects")
        .join(name);
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(root).unwrap()).unwrap();
    ProjectLoader::new()
        .open(TransactionalDirectory::new(Arc::new(fs), ""), "project.lpp")
        .unwrap()
}
fn included(node: &SExpression, board: &str) -> bool {
    let kind: String = node.child_value("@0").unwrap();
    match kind.as_str() {
        "copper_clearance_violation"
        | "copper_board_clearance_violation"
        | "copper_hole_clearance_violation"
        | "minimum_annular_ring_violation"
        | "empty_netsegment"
        | "unconnected_junction"
        | "impossible_connection"
        | "forbidden_via"
        | "invalid_via"
        | "invalid_pad_connection"
        | "missing_board_outline"
        | "multiple_board_outlines"
        | "open_board_outline"
        | "intersecting_board_outlines"
        | "cutout_outside_board_area"
        | "minimum_board_inner_radius_violation"
        | "drill_clearance_violation"
        | "drill_board_clearance_violation"
        | "minimum_width_violation"
        | "minimum_text_height_violation"
        | "minimum_drill_diameter_violation"
        | "minimum_slot_width_violation"
        | "forbidden_slot"
        | "plane_thermal_spoke_width_ignored"
        | "disabled_layer" => true,
        // Same whitelist as the upstream test: empty layers on unrelated
        // boards are intentionally ignored in its stored expectations.
        "missing_device" => board == "checkForUnplacedComponents",
        "missing_connection" => false,
        "useless_via" => ["checkVias", "checkVias2", "checkVias3"].contains(&board),
        "unused_layer" => board == "checkUsedLayers",
        "plated_cutouts" => board == "checkBoardCutouts",
        _ => true,
    }
}
#[test]
fn test_messages() {
    let project = load("DRC");
    let revision = project.revision();
    let mut total = 0;
    for board in project.boards() {
        let data = BoardDrcData::extract(&project, board, &board.settings().drc_settings).unwrap();
        let findings = all_checks(&data);
        if board.name().as_str() == "checkForMissingConnections" {
            let actual: Vec<_> = findings
                .iter()
                .map(|f| f.rule_message().approval())
                .filter(|n| n.child_value::<String>("@0").unwrap() == "missing_connection")
                .cloned()
                .collect();
            let expected: Vec<_> = board
                .drc_approvals()
                .iter()
                .filter(|n| n.child_value::<String>("@0").unwrap() == "missing_connection")
                .cloned()
                .collect();
            compare_equivalent_airwire_trees(&project, board, &actual, &expected);
            total += actual.len();
        }
        let mut actual = BTreeSet::new();
        for finding in findings {
            assert_eq!(finding.board(), board.id());
            let approval = finding.rule_message().approval();
            if included(approval, board.name().as_str()) {
                assert!(
                    actual.insert(approval.clone()),
                    "ambiguous approval: {approval:?}"
                );
            }
        }
        let expected: BTreeSet<_> = board
            .drc_approvals()
            .iter()
            .filter(|n| included(n, board.name().as_str()))
            .cloned()
            .collect();
        assert_eq!(actual, expected, "{}", board.name());
        total += actual.len();
    }
    assert_eq!(total, 222, "upstream DRC approval vectors changed");
    assert_eq!(revision, project.revision());
}
#[test]
fn snapshots_outlive_the_project_and_run_concurrently() {
    let data = {
        let project = load("Gerber Test");
        let board = &project.boards()[0];
        BoardDrcData::extract(&project, board, &board.settings().drc_settings).unwrap()
    };
    let expected = all_checks(&data);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| all_checks(&data));
        let b = scope.spawn(|| all_checks(&data));
        assert_eq!(a.join().unwrap(), expected);
        assert_eq!(b.join().unwrap(), expected);
    });
}

#[test]
fn full_drc_messages_match_official_cli() {
    use librepcb_core::fileio::file_utils;
    use librepcb_core::rule_check::Severity;
    use librepcb_core::serialization::Mode;
    use std::process::Command;
    let configured = std::env::var_os("LIBREPCB_CLI");
    let cli = configured.clone().unwrap_or_else(|| "librepcb-cli".into());
    let available = Command::new(&cli)
        .arg("--version")
        .env("QT_QPA_PLATFORM", "offscreen")
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(
        available || configured.is_none(),
        "Configured LIBREPCB_CLI cannot run"
    );
    if !available {
        eprintln!("librepcb-cli missing; CLI comparison skipped");
        return;
    }
    let project = load("DRC");
    let tmp = tempfile::tempdir().unwrap();
    let copy = tmp.path().join("project");
    let source = Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects/DRC");
    file_utils::copy_dir_recursively(
        &FilePath::new(source).unwrap(),
        &FilePath::new(&copy).unwrap(),
    )
    .unwrap();
    let mut expected = Vec::new();
    for board in project.boards() {
        let path = copy
            .join("boards")
            .join(board.directory_name())
            .join("board.lp");
        let mut node =
            SExpression::parse(&std::fs::read(&path).unwrap(), Some(&path), Mode::LibrePcb)
                .unwrap();
        let SExpression::List(drc) = node.child_mut("design_rule_check").unwrap() else {
            panic!("expected DRC list");
        };
        drc.children_mut()
            .retain(|n| !matches!(n, SExpression::List(list) if list.name() == "approved"));
        std::fs::write(path, node.to_string_with_mode(Mode::LibrePcb).unwrap()).unwrap();
        let data = BoardDrcData::extract(&project, board, &board.settings().drc_settings).unwrap();
        for finding in all_checks(&data) {
            let message = finding.rule_message();
            let severity = match message.severity() {
                Severity::Hint => "HINT",
                Severity::Warning => "WARNING",
                Severity::Error => "ERROR",
            };
            expected.push(normalize_message(format!(
                "[{severity}] {}",
                message.message()
            )));
        }
    }
    let output = Command::new(cli)
        .arg("open-project")
        .arg("--drc")
        .arg(copy.join("project.lpp"))
        .env("QT_QPA_PLATFORM", "offscreen")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .env_remove("LIBREPCB_UPGRADE_UNSTABLE")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(matches!(output.status.code(), Some(0 | 1)), "{stderr}");
    let mut actual: Vec<_> = stderr
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- ["))
        .map(|l| format!("[{l}"))
        .collect();
    actual = actual.into_iter().map(normalize_message).collect();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected, "{stderr}");
}

fn all_checks(data: &BoardDrcData) -> Vec<librepcb_core::project::board::drc::BoardDrcMessage> {
    let mut result = BoardDesignRuleCheck::run_manufacturing_rules(data);
    result.extend(BoardDesignRuleCheck::run_drill_clearances(data).unwrap());
    result.extend(BoardDesignRuleCheck::run_outline_rules(data).unwrap());
    result.extend(BoardDesignRuleCheck::run_structural_rules(data).unwrap());
    result.extend(BoardDesignRuleCheck::run_copper_rules(data).unwrap());
    result.extend(BoardDesignRuleCheck::run_placement_rules(data).unwrap());
    result.extend(BoardDesignRuleCheck::run_keepout_rules(data).unwrap());
    result.extend(BoardDesignRuleCheck::run_cutout_rules(data).unwrap());
    result
}
fn normalize_message(message: String) -> String {
    // The chosen edges of an equal-weight MST are intentionally different
    // (COMPAT.md). Compare counts per net here; the fixture test separately
    // verifies the complete trees, vertices and exact edge weights.
    if message.contains("Missing connection in")
        && let Some((prefix, _)) = message.split_once(": ")
    {
        return prefix.to_owned();
    }
    if message.contains("Clearance on ")
        && let Some((prefix, objects)) = message.split_once(": ")
        && let Some((objects, minimum)) = objects.rsplit_once(" < ")
        && let Some((a, b)) = objects.split_once(" ↔ ")
    {
        return format!("{prefix}: {} ↔ {} < {minimum}", a.min(b), a.max(b));
    }
    message
}

fn compare_equivalent_airwire_trees(
    project: &Project,
    board: &librepcb_core::project::board::Board,
    actual: &[SExpression],
    expected: &[SExpression],
) {
    use librepcb_core::project::{ComponentInstanceId, NetSegmentId};
    use librepcb_core::types::{Point, Uuid};
    use std::collections::BTreeMap;
    assert!(
        board.planes().is_empty(),
        "this reference fixture has no plane-connected components"
    );
    let positions = |node: &SExpression| -> Vec<Point> {
        if let Ok(uuid) = node.child_value::<Uuid>("netsegment/@0") {
            let segment = &board.net_segments()[&NetSegmentId(uuid)];
            segment
                .junctions()
                .values()
                .map(|j| j.position())
                .chain(segment.vias().values().map(|v| v.position()))
                .chain(segment.pads().values().map(|p| p.pad().position()))
                .collect()
        } else {
            let device = ComponentInstanceId(node.child_value::<Uuid>("device/@0").unwrap());
            let pad = node.child_value::<Uuid>("pad/@0").unwrap();
            assert!(board.footprint_pad_segment(device, pad).is_none());
            vec![
                board.devices()[&device]
                    .pad(&pad, project.library(), project.circuit())
                    .unwrap()
                    .unwrap()
                    .position(),
            ]
        }
    };
    let decode = |approvals: &[SExpression]| {
        let mut edges = Vec::new();
        let mut vertices = BTreeSet::new();
        let mut weights = Vec::new();
        for approval in approvals {
            let a = approval.required_child("from").unwrap();
            let b = approval.required_child("to").unwrap();
            let weight = positions(a)
                .iter()
                .flat_map(|p| {
                    positions(b).into_iter().map(move |q| {
                        let dx = i128::from(p.x.to_nm()) - i128::from(q.x.to_nm());
                        let dy = i128::from(p.y.to_nm()) - i128::from(q.y.to_nm());
                        dx * dx + dy * dy
                    })
                })
                .min()
                .unwrap();
            let key = |node: &SExpression| {
                let SExpression::List(mut n) = node.clone() else {
                    panic!("anchor is list")
                };
                n.set_name("anchor");
                SExpression::List(n)
            };
            let a = key(a);
            let b = key(b);
            vertices.insert(a.clone());
            vertices.insert(b.clone());
            edges.push((a, b));
            weights.push(weight);
        }
        let indices: BTreeMap<_, _> = vertices
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i))
            .collect();
        let mut union = petgraph::unionfind::UnionFind::<usize>::new(indices.len());
        for (a, b) in edges {
            assert!(
                union.union(indices[&a], indices[&b]),
                "airwire graph has a cycle"
            );
        }
        assert_eq!(
            approvals.len() + 1,
            vertices.len(),
            "airwire graph is not a spanning tree"
        );
        weights.sort();
        (vertices, weights)
    };
    assert_eq!(
        decode(actual),
        decode(expected),
        "alternative airwire trees must span the same anchors with the same weights"
    );
}
