//! Compares the design rule check with the official `librepcb-cli`.
//!
//! For every project in the upstream `tests/data/projects`, the upstream CLI
//! runs `open-project --drc` (on a temporary copy) and prints per board the
//! number of approved messages and the non-approved messages. This crate
//! runs [`Project::run_drc()`] on the same boards; the number of approved
//! messages (i.e. messages whose approval S-expression is stored in the
//! board, so this compares approvals) and the non-approved messages
//! (severity and text) must be identical.
//!
//! The upstream CLI is taken from the environment variable `LIBREPCB_CLI`,
//! or `librepcb-cli` in `PATH`. Without it, the comparison is skipped (with
//! a message on stderr).
//!
//! A second test removes all DRC approvals from a copy of the `DRC` test
//! project first, so all messages are printed and compared (e.g. the order
//! of the objects in copper clearance messages).
//!
//! Known differences, normalized before the comparison: the order of the
//! two objects of copper clearance messages (upstream iterates `QHash`es,
//! see COMPAT.md).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::ProjectLoader;
use librepcb_core::project::board::drc::DrcProgress;
use librepcb_core::serialization::{Mode, SExpression};

fn projects_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data/projects")
}

/// Returns the upstream CLI command, if available.
fn upstream_cli() -> Option<PathBuf> {
    let cli = std::env::var_os("LIBREPCB_CLI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("librepcb-cli"));
    let ok = Command::new(&cli)
        .arg("--version")
        .env("QT_QPA_PLATFORM", "offscreen")
        .output()
        .is_ok_and(|out| out.status.success());
    ok.then_some(cli)
}

fn copy_dir(src: &Path, dst: &Path) {
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry.unwrap();
        let target = dst.join(entry.path().strip_prefix(src).unwrap());
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn lpp_file(dir: &Path) -> Option<String> {
    std::fs::read_dir(dir)
        .ok()?
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".lpp"))
}

/// DRC summary of one board: approved count and non-approved messages.
#[derive(Debug, Default, PartialEq, Eq)]
struct BoardSummary {
    approved: usize,
    messages: Vec<String>,
}

/// Normalizes a message line for the comparison.
fn normalize(line: &str) -> String {
    // The order of the objects of copper clearance violations depends on
    // the iteration order of upstream's `QHash`es.
    if let Some(rest) = line.strip_prefix("[ERROR] Clearance on ")
        && let Some((layers, rest)) = rest.split_once(": ")
        && let Some((objects, value)) = rest.split_once(" < ")
        && let Some((a, b)) = objects.split_once(" ↔ ")
    {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        return format!("[ERROR] Clearance on {layers}: {a} ↔ {b} < {value}");
    }
    line.to_owned()
}

/// Runs the upstream CLI and parses its output per board.
fn run_upstream(cli: &Path, dir: &Path, lpp: &str) -> BTreeMap<String, BoardSummary> {
    let log = dir.join("drc.log");
    let file = std::fs::File::create(&log).unwrap();
    let status = Command::new(cli)
        .current_dir(dir)
        .args(["open-project", "--drc", lpp])
        .env("QT_QPA_PLATFORM", "offscreen")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .stdout(Stdio::from(file.try_clone().unwrap()))
        .stderr(Stdio::from(file))
        .status()
        .unwrap();
    let output = std::fs::read_to_string(&log).unwrap();
    // The CLI exits with an error if there are non-approved messages.
    let _ = status;
    let mut boards: BTreeMap<String, BoardSummary> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in output.lines() {
        if let Some(name) = line
            .strip_prefix("  Board '")
            .and_then(|l| l.strip_suffix("':"))
        {
            current = Some(name.to_owned());
            boards.insert(name.to_owned(), BoardSummary::default());
        } else if let Some(board) = current.as_ref().and_then(|c| boards.get_mut(c)) {
            if let Some(n) = line.strip_prefix("    Approved messages: ") {
                board.approved = n.trim().parse().unwrap();
            } else if let Some(msg) = line.strip_prefix("      - ") {
                board.messages.push(normalize(msg));
            }
        }
    }
    for board in boards.values_mut() {
        board.messages.sort();
    }
    boards
}

/// Runs our DRC on all boards of a project.
fn run_ours(dir: &Path, lpp: &str) -> BTreeMap<String, BoardSummary> {
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(dir).unwrap()).unwrap();
    let mut project = ProjectLoader::new()
        .open(TransactionalDirectory::new(Arc::new(fs), ""), lpp)
        .unwrap();
    let boards: Vec<_> = project
        .boards_with_ids()
        .map(|(id, b)| {
            (
                id,
                b.properties().name.to_string(),
                b.drc_approvals().clone(),
            )
        })
        .collect();
    let mut result = BTreeMap::new();
    for (id, name, approvals) in boards {
        let drc = project
            .run_drc(id, None, false, &|_: DrcProgress<'_>| {})
            .unwrap();
        assert!(drc.errors.is_empty(), "{name}: {:?}", drc.errors);
        let mut summary = BoardSummary::default();
        for msg in &drc.messages {
            if approvals.contains(msg.approval()) {
                summary.approved += 1;
            } else {
                let m = msg.message();
                summary.messages.push(normalize(&format!(
                    "[{}] {}",
                    m.severity().name_tr().to_uppercase(),
                    m.message()
                )));
            }
        }
        summary.messages.sort();
        result.insert(name, summary);
    }
    result
}

/// Returns the lines only in `a` (multiset difference).
fn only_in(a: &[String], b: &[String]) -> Vec<String> {
    let mut b = b.to_vec();
    a.iter()
        .filter(|x| match b.iter().position(|y| y == *x) {
            Some(i) => {
                b.remove(i);
                false
            }
            None => true,
        })
        .cloned()
        .collect()
}

#[test]
fn drc_like_upstream_cli() {
    compare_with_cli(|_| true, |_| {});
}

#[test]
fn drc_without_approvals_like_upstream_cli() {
    compare_with_cli(|name| name == "DRC", remove_approvals);
}

/// Removes the DRC approvals from all boards of a project.
fn remove_approvals(dir: &Path) {
    for entry in walkdir::WalkDir::new(dir.join("boards")) {
        let path = entry.unwrap().into_path();
        if path.file_name().is_none_or(|n| n != "board.lp") {
            continue;
        }
        let content = std::fs::read(&path).unwrap();
        let mut root = SExpression::parse(&content, Some(&path), Mode::LibrePcb).unwrap();
        let drc = root
            .child_mut("design_rule_check")
            .and_then(SExpression::as_list_mut)
            .unwrap();
        drc.children_mut()
            .retain(|c| c.as_list().is_none_or(|l| l.name() != "approved"));
        std::fs::write(&path, root.to_byte_array(Mode::LibrePcb).unwrap()).unwrap();
    }
}

/// Compares the DRC of the projects accepted by `filter` (copied and
/// modified by `prepare`) with the upstream CLI.
fn compare_with_cli(filter: impl Fn(&str) -> bool, prepare: impl Fn(&Path)) {
    let Some(cli) = upstream_cli() else {
        eprintln!("librepcb-cli not found (set LIBREPCB_CLI), skipping the DRC comparison");
        return;
    };
    librepcb_i18n::set_language("en").unwrap();
    let tmp = tempfile::Builder::new()
        .prefix("librepcb-drc-")
        .tempdir()
        .unwrap();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(projects_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join(".librepcb-project").is_file())
        .collect();
    dirs.sort();

    let mut failures = Vec::new();
    let mut checked = 0;
    for src in dirs {
        let Some(lpp) = lpp_file(&src) else {
            continue;
        };
        let name = src.file_name().unwrap().to_string_lossy().into_owned();
        if !filter(&name) {
            continue;
        }
        let copy = tmp.path().join(&name);
        copy_dir(&src, &copy);
        prepare(&copy);
        let upstream = run_upstream(&cli, &copy, &lpp);
        let ours = run_ours(&copy, &lpp);
        assert_eq!(
            upstream.keys().collect::<Vec<_>>(),
            ours.keys().collect::<Vec<_>>(),
            "{name}: boards"
        );
        for (board, expected) in &upstream {
            checked += 1;
            let actual = &ours[board];
            if actual == expected {
                continue;
            }
            failures.push(format!(
                "{name} / {board}: approved {} (upstream {}), missing messages {:#?}, \
                 extra messages {:#?}",
                actual.approved,
                expected.approved,
                only_in(&expected.messages, &actual.messages),
                only_in(&actual.messages, &expected.messages),
            ));
        }
    }
    println!("checked {checked} boards");
    assert!(
        failures.is_empty(),
        "DRC differs from upstream:\n{}",
        failures.join("\n")
    );
}
