//! Compares the ERC, the BOM export and the JSON export of all projects in
//! the upstream test data with the official `librepcb-cli`.
//!
//! - ERC: the sorted list of non-approved messages and the number of
//!   approved messages printed by `librepcb-cli open-project --erc`. Then all
//!   our approvals are stored in a copy of the project (saved by this
//!   crate), and the CLI must report every message as approved, which
//!   verifies the approval S-expressions byte for byte.
//! - BOM: the generic BOM (`--export-bom`) and the board BOMs
//!   (`--export-board-bom`) of every assembly variant, with the default
//!   attributes of the project, byte-identical.
//! - JSON: a `project_json` output job (`--jobs`, `--run-jobs`),
//!   byte-identical.
//!
//! The upstream CLI is taken from the environment variable `LIBREPCB_CLI`,
//! or `librepcb-cli` in `PATH`. Without it, the comparison is skipped (with
//! a message on stderr).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use librepcb_core::export::BomCsvWriter;
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::project::erc::run_erc;
use librepcb_core::project::{BomGenerator, Mutation, Project, ProjectLoader, json_export};
use librepcb_core::rule_check::all_approvals;

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

/// Runs the upstream CLI (in English, without GUI) and returns stdout and
/// stderr. The exit code is ignored (ERC messages make it fail).
fn run_cli(cli: &Path, args: &[&str]) -> (String, String) {
    let out = Command::new(cli)
        .args(args)
        .env("QT_QPA_PLATFORM", "offscreen")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .env_remove("LIBREPCB_UPGRADE_UNSTABLE")
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn project_dirs() -> Vec<PathBuf> {
    let root = projects_dir()
        .canonicalize()
        .expect("upstream test data not found (set LIBREPCB_UPSTREAM_DIR)");
    let mut dirs: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join(".librepcb-project").is_file())
        .collect();
    dirs.sort();
    dirs
}

fn lpp_name(dir: &Path) -> String {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".lpp"))
        .expect("no *.lpp file")
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

fn open(dir: &Path, writable: bool) -> Project {
    let path = FilePath::new(dir).unwrap();
    let fs = if writable {
        TransactionalFileSystem::open_rw(&path)
    } else {
        TransactionalFileSystem::open_ro(&path)
    }
    .unwrap();
    ProjectLoader::new()
        .open(
            TransactionalDirectory::new(Arc::new(fs), ""),
            &lpp_name(dir),
        )
        .unwrap()
}

/// Our ERC result like the CLI prints it: (approved count, sorted
/// "[SEVERITY] message" lines of the non-approved messages).
fn our_erc(project: &Project) -> (usize, Vec<String>) {
    let mut approved = 0;
    let mut lines = Vec::new();
    for msg in run_erc(project) {
        if project.erc_approvals().contains(msg.approval()) {
            approved += 1;
        } else {
            let m = msg.message();
            lines.push(format!(
                "[{}] {}",
                m.severity().name_tr().to_uppercase(),
                m.message()
            ));
        }
    }
    lines.sort();
    (approved, lines)
}

/// The ERC result printed by the CLI.
fn cli_erc(stdout: &str, stderr: &str) -> (usize, Vec<String>) {
    let approved = stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("Approved messages: "))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no ERC summary in output:\n{stdout}\n{stderr}"));
    // Depending on the build, the messages go to stdout or stderr.
    let mut lines: Vec<String> = stdout
        .lines()
        .chain(stderr.lines())
        .filter_map(|l| l.strip_prefix("    - "))
        .filter(|l| l.starts_with('['))
        .map(str::to_owned)
        .collect();
    lines.sort();
    (approved, lines)
}

const JOBS: &str = "(librepcb_jobs
 (job 84da2b4c-2c40-48ac-99f2-b8e9acfe78ec (name \"Project Data\")
  (type project_json)
  (output \"project.json\")
 )
)
";

#[test]
fn exports_like_upstream_cli() {
    let Some(cli) = upstream_cli() else {
        eprintln!(
            "librepcb-cli not found (set LIBREPCB_CLI), skipping the comparison with upstream"
        );
        return;
    };
    let mut tmp = tempfile::Builder::new()
        .prefix("librepcb-exports-")
        .tempdir()
        .unwrap();
    // Set LIBREPCB_KEEP_EXPORT_OUTPUT=1 to inspect the exported files.
    if std::env::var_os("LIBREPCB_KEEP_EXPORT_OUTPUT").is_some_and(|v| v == "1") {
        tmp.disable_cleanup(true);
        println!("exported files are kept in {}", tmp.path().display());
    }
    let jobs_file = tmp.path().join("jobs.lp");
    std::fs::write(&jobs_file, JOBS).unwrap();

    let mut failures = Vec::new();
    let mut checked = Vec::new();
    let (mut bom_count, mut approval_count) = (0, 0);
    for (index, src) in project_dirs().iter().enumerate() {
        let name = src.file_name().unwrap().to_string_lossy().into_owned();
        let work = tmp.path().join(index.to_string());
        let copy = work.join("project");
        let out = work.join("out");
        copy_dir(src, &copy);
        std::fs::create_dir_all(&out).unwrap();
        let lpp = copy.join(lpp_name(&copy));
        let bom = out.join("bom_{{VARIANT}}.csv");
        let board_bom = out.join("board_{{BOARD_INDEX}}_{{VARIANT}}.csv");
        let (stdout, stderr) = run_cli(
            &cli,
            &[
                "open-project",
                "--erc",
                "--export-bom",
                bom.to_str().unwrap(),
                "--export-board-bom",
                board_bom.to_str().unwrap(),
                "--jobs",
                jobs_file.to_str().unwrap(),
                "--run-jobs",
                "--outdir",
                out.to_str().unwrap(),
                lpp.to_str().unwrap(),
            ],
        );

        let project = open(src, false);

        // ERC messages.
        let ours = our_erc(&project);
        let theirs = cli_erc(&stdout, &stderr);
        if ours != theirs {
            failures.push(format!("{name}: ERC differs:\n{ours:#?}\n!=\n{theirs:#?}"));
        }

        // BOMs.
        let attributes = project.settings().custom_bom_attributes.clone();
        let mut compare = |file: String, csv: String| {
            bom_count += 1;
            match std::fs::read_to_string(out.join(&file)) {
                Ok(expected) if expected == csv => {}
                Ok(expected) => {
                    failures.push(format!("{name}: {file} differs:\n{expected}\n!=\n{csv}"))
                }
                Err(e) => failures.push(format!("{name}: {file}: {e}\n{stdout}\n{stderr}")),
            }
        };
        for av in project.circuit().assembly_variants().iter() {
            let mut generator = BomGenerator::new(&project);
            generator.set_additional_attributes(attributes.clone());
            let variant = librepcb_core::project::AssemblyVariantId(av.uuid());
            let csv = |bom| {
                BomCsvWriter::new(&bom)
                    .generate_csv()
                    .unwrap()
                    .to_csv_string()
                    .unwrap()
            };
            compare(
                format!("bom_{}.csv", av.name()),
                csv(generator.generate(None, variant)),
            );
            for (i, board) in project.boards().iter().enumerate() {
                compare(
                    format!("board_{i}_{}.csv", av.name()),
                    csv(generator.generate(Some(board), variant)),
                );
            }
        }

        // JSON.
        match std::fs::read(out.join("project.json")) {
            Ok(expected) => {
                let actual = json_export::to_utf8(&project);
                if expected != actual {
                    failures.push(format!(
                        "{name}: project.json differs:\n{}\n!=\n{}",
                        String::from_utf8_lossy(&expected),
                        String::from_utf8_lossy(&actual)
                    ));
                }
            }
            Err(e) => failures.push(format!("{name}: project.json: {e}\n{stdout}\n{stderr}")),
        }

        // ERC approvals: approve everything in a copy saved by us, then
        // the CLI must report all messages as approved.
        let approved_copy = work.join("approved");
        copy_dir(src, &approved_copy);
        let mut p = open(&approved_copy, true);
        let msgs = run_erc(&p);
        let approvals = all_approvals(msgs.iter().map(|m| m.message()));
        p.apply(Mutation::SetErcApprovals(approvals.clone()))
            .unwrap();
        p.save().unwrap();
        p.directory().file_system().save().unwrap();
        drop(p);
        let (stdout, stderr) = run_cli(
            &cli,
            &[
                "open-project",
                "--erc",
                approved_copy
                    .join(lpp_name(&approved_copy))
                    .to_str()
                    .unwrap(),
            ],
        );
        let theirs = cli_erc(&stdout, &stderr);
        approval_count += msgs.len();
        if theirs != (msgs.len(), Vec::new()) {
            failures.push(format!(
                "{name}: approvals not accepted by upstream ({} messages, {} distinct \
                 approvals):\n{theirs:#?}",
                msgs.len(),
                approvals.len()
            ));
        }
        checked.push(name);
    }
    println!(
        "checked projects: {checked:?}, {bom_count} BOM files, {approval_count} approved messages"
    );
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert!(bom_count >= 10, "too few BOM files compared");
    assert!(approval_count >= 30, "too few ERC messages compared");
}

/// Compares the ERC of the (saved) project in `dir` with the CLI, then
/// approves all messages in a copy and checks that the CLI accepts them.
/// Returns the number of messages.
fn compare_erc(cli: &Path, dir: &Path, work: &Path, failures: &mut Vec<String>) -> usize {
    let name = dir.display().to_string();
    let (stdout, stderr) = run_cli(
        cli,
        &[
            "open-project",
            "--erc",
            dir.join(lpp_name(dir)).to_str().unwrap(),
        ],
    );
    let project = open(dir, false);
    let ours = our_erc(&project);
    let theirs = cli_erc(&stdout, &stderr);
    if ours != theirs {
        failures.push(format!("{name}: ERC differs:\n{ours:#?}\n!=\n{theirs:#?}"));
    }
    drop(project);

    let approved_copy = work.join("approved");
    copy_dir(dir, &approved_copy);
    let mut p = open(&approved_copy, true);
    let msgs = run_erc(&p);
    let approvals = all_approvals(msgs.iter().map(|m| m.message()));
    p.apply(Mutation::SetErcApprovals(approvals)).unwrap();
    p.save().unwrap();
    p.directory().file_system().save().unwrap();
    drop(p);
    let (stdout, stderr) = run_cli(
        cli,
        &[
            "open-project",
            "--erc",
            approved_copy
                .join(lpp_name(&approved_copy))
                .to_str()
                .unwrap(),
        ],
    );
    let theirs = cli_erc(&stdout, &stderr);
    if theirs != (msgs.len(), Vec::new()) {
        failures.push(format!(
            "{name}: approvals not accepted by upstream ({} messages):\n{theirs:#?}",
            msgs.len()
        ));
    }
    msgs.len()
}

/// Modifies a test project to raise the ERC messages which the test data
/// does not contain (unused net class and bus, unconnected junctions, open
/// wire, unplaced gate, connected pin without wire) and compares the ERC
/// with the CLI.
#[test]
fn erc_of_modified_project_like_upstream_cli() {
    use librepcb_core::geometry::{Junction, NetLine, NetLineAnchor};
    use librepcb_core::project::circuit::{Bus, ComponentInstance, NetClass};
    use librepcb_core::project::erc::ErcMessageKind;
    use librepcb_core::project::schematic::{SchematicBusSegment, SchematicNetSegment};
    use librepcb_core::project::{ComponentSignalRef, SchematicMutation};
    use librepcb_core::types::{Length, Point, UnsignedLength, Uuid};

    let Some(cli) = upstream_cli() else {
        eprintln!(
            "librepcb-cli not found (set LIBREPCB_CLI), skipping the comparison with upstream"
        );
        return;
    };
    let tmp = tempfile::Builder::new()
        .prefix("librepcb-erc-")
        .tempdir()
        .unwrap();
    let dir = tmp.path().join("project");
    copy_dir(&projects_dir().join("Gerber Test"), &dir);
    let mut p = open(&dir, true);
    let pt = |x: i64, y: i64| Point::new(Length::new(x), Length::new(y));

    // Unused net class and bus.
    let bus = *p.circuit().buses().keys().next().expect("a bus");
    p.apply(Mutation::AddNetClass(NetClass::new(
        Uuid::new_random(),
        "Unused".parse().unwrap(),
    )))
    .unwrap();
    p.apply(Mutation::AddBus(Bus::new(
        Uuid::new_random(),
        "UNUSED".parse().unwrap(),
        false,
        false,
        None,
    )))
    .unwrap();

    // A net which is not open (so open wires are reported).
    let open_nets: Vec<_> = run_erc(&p)
        .iter()
        .filter_map(|m| match m.kind() {
            ErcMessageKind::OpenNet(n) => Some(n),
            _ => None,
        })
        .collect();
    let net = *p
        .circuit()
        .net_signals()
        .keys()
        .find(|n| !open_nets.contains(n))
        .expect("a connected net");
    let schematic = p.schematics()[0].id();

    // Net segment with an unconnected junction.
    let mut segment = SchematicNetSegment::new(Uuid::new_random(), net);
    segment.insert_junction(Junction::new(Uuid::new_random(), pt(1_000_000_000, 0)));
    p.apply(Mutation::Schematic(SchematicMutation::AddNetSegment {
        schematic,
        segment,
    }))
    .unwrap();

    // Net segment with an open wire.
    let mut segment = SchematicNetSegment::new(Uuid::new_random(), net);
    let (j1, j2) = (Uuid::new_random(), Uuid::new_random());
    segment.insert_junction(Junction::new(j1, pt(1_010_000_000, 0)));
    segment.insert_junction(Junction::new(j2, pt(1_020_000_000, 0)));
    segment.insert_line(NetLine::new(
        Uuid::new_random(),
        UnsignedLength::new(Length::new(158_750)).unwrap(),
        NetLineAnchor::Junction(j1),
        NetLineAnchor::Junction(j2),
    ));
    p.apply(Mutation::Schematic(SchematicMutation::AddNetSegment {
        schematic,
        segment,
    }))
    .unwrap();

    // Bus segment with an unconnected junction.
    let mut segment = SchematicBusSegment::new(Uuid::new_random(), bus);
    segment.insert_junction(Junction::new(Uuid::new_random(), pt(1_030_000_000, 0)));
    p.apply(Mutation::Schematic(SchematicMutation::AddBusSegment {
        schematic,
        segment,
    }))
    .unwrap();

    // Unplaced gates: a component instance without symbols.
    let lib_component = p
        .library()
        .components()
        .values()
        .next()
        .expect("a library component");
    let variant = lib_component
        .symbol_variants()
        .iter()
        .next()
        .unwrap()
        .uuid();
    let cmp = ComponentInstance::new(
        Uuid::new_random(),
        lib_component,
        variant,
        "UNPLACED1".parse().unwrap(),
        false,
    )
    .unwrap();
    p.apply(Mutation::AddComponentInstance(cmp)).unwrap();

    // Connected pin without wire: connect an unconnected, unwired signal.
    let signals: Vec<_> = p
        .circuit()
        .component_instances()
        .values()
        .flat_map(|c| {
            c.signals()
                .iter()
                .filter(|(_, s)| s.net().is_none())
                .map(|(uuid, _)| ComponentSignalRef {
                    component: c.id(),
                    signal: *uuid,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let connected = signals.iter().any(|s| {
        p.apply(Mutation::SetComponentSignalNet {
            signal: *s,
            net: Some(net),
        })
        .is_ok()
            && run_erc(&p)
                .iter()
                .any(|m| matches!(m.kind(), ErcMessageKind::ConnectedPinWithoutWire { .. }))
    });
    assert!(connected, "no connectable signal");

    p.save().unwrap();
    p.directory().file_system().save().unwrap();
    let kinds: Vec<_> = run_erc(&p).iter().map(|m| m.kind()).collect();
    drop(p);
    for expected in [
        "UnusedNetClass",
        "UnusedBus",
        "UnconnectedNetJunction",
        "UnconnectedBusJunction",
        "OpenWireInSegment",
        "Unplaced",
        "ConnectedPinWithoutWire",
    ] {
        assert!(
            kinds.iter().any(|k| format!("{k:?}").starts_with(expected)),
            "no {expected} message: {kinds:?}"
        );
    }

    let mut failures = Vec::new();
    let count = compare_erc(&cli, &dir, tmp.path(), &mut failures);
    println!("{count} ERC messages compared");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
