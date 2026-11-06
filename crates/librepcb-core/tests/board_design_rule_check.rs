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
        "drill_clearance_violation"
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
        "unused_layer" => board == "checkUsedLayers",
        _ => false,
    }
}
#[test]
fn manufacturing_messages_match_upstream_approvals() {
    let project = load("DRC");
    let revision = project.revision();
    let mut total = 0;
    for board in project.boards() {
        let data = BoardDrcData::extract(&project, board, &board.settings().drc_settings).unwrap();
        let mut findings = BoardDesignRuleCheck::run_manufacturing_rules(&data);
        findings.extend(BoardDesignRuleCheck::run_drill_clearances(&data).unwrap());
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
    assert_eq!(total, 36, "upstream DRC approval vectors changed");
    assert_eq!(revision, project.revision());
}
#[test]
fn snapshots_outlive_the_project_and_run_concurrently() {
    let data = {
        let project = load("Gerber Test");
        let board = &project.boards()[0];
        BoardDrcData::extract(&project, board, &board.settings().drc_settings).unwrap()
    };
    let expected = BoardDesignRuleCheck::run_manufacturing_rules(&data);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| BoardDesignRuleCheck::run_manufacturing_rules(&data));
        let b = scope.spawn(|| BoardDesignRuleCheck::run_manufacturing_rules(&data));
        assert_eq!(a.join().unwrap(), expected);
        assert_eq!(b.join().unwrap(), expected);
    });
}

#[test]
fn manufacturing_messages_match_official_cli() {
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
        for finding in BoardDesignRuleCheck::run_manufacturing_rules(&data)
            .into_iter()
            .chain(BoardDesignRuleCheck::run_drill_clearances(&data).unwrap())
        {
            let message = finding.rule_message();
            let severity = match message.severity() {
                Severity::Hint => "HINT",
                Severity::Warning => "WARNING",
                Severity::Error => "ERROR",
            };
            expected.push(format!("[{severity}] {}", message.message()));
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
    let prefixes = [
        "Clearance drill ↔ drill",
        "Clearance drill ↔ board outline",
        "Trace width",
        "Min. plane width",
        "Polygon width",
        "Stroke width",
        "Circle width",
        "Text height",
        "Via drill diameter",
        "Pad drill diameter",
        "NPTH drill diameter",
        "Plated slot width",
        "NPTH slot width",
        "Hole is a slot",
        "Hole is a multi-segment slot",
        "Ignored thermal spoke width",
        "Objects on disabled layer",
        "Unused layer:",
    ];
    let mut actual: Vec<_> = stderr
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- ["))
        .map(|l| format!("[{l}"))
        .filter(|l| {
            l.split_once("] ")
                .is_some_and(|(_, message)| prefixes.iter().any(|p| message.starts_with(p)))
        })
        .collect();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected, "{stderr}");
}
