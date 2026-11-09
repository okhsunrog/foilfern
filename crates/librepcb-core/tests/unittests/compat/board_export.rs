//! Compares the board exports with the official `librepcb-cli`.
//!
//! Every project with boards in the upstream `tests/data/projects` is copied
//! twice into a temporary directory. The upstream CLI exports one copy
//! (`open-project --export-pcb-fabrication-data --export-pnp-top ...
//! --export-pnp-bottom ... --export-netlist ...`, all boards and assembly
//! variants), this crate the other one (planes rebuild, Gerber/Excellon
//! fabrication data, pick&place CSV and Gerber X3 files, IPC-D-356A
//! netlists). Afterwards, all exported files must be byte-identical, except
//! the lines containing the generation software version, the creation date
//! and the MD5 checksum over them.
//!
//! The upstream CLI is taken from the environment variable `LIBREPCB_CLI`,
//! or `librepcb-cli` in `PATH`. Without it, the comparison is skipped (with
//! a message on stderr); the ported upstream unit tests still compare the
//! exports with the upstream golden files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use librepcb_core::export::{BoardSide, PickPlaceSides, Timestamp};
use librepcb_core::fileio::{
    FilePath, TransactionalDirectory, TransactionalFileSystem, file_utils,
};
use librepcb_core::project::board::{
    BoardFabricationOutputSettings, ExportInfo, export_component_layer, export_d356_netlist,
    export_fabrication_data, export_pick_place_csv,
};
use librepcb_core::project::{AssemblyVariantId, ProjectLoader};
use librepcb_core::serialization::{List, Mode, SExpression, SerializeObject};

fn data_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
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

/// Runs the upstream CLI with `args` in `cwd`, in English and without GUI.
fn run_cli(cli: &Path, cwd: &Path, args: &[&str]) {
    let out = Command::new(cli)
        .args(args)
        .current_dir(cwd)
        .env("QT_QPA_PLATFORM", "offscreen")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .env("LIBREPCB_SUPPRESS_DEPRECATION_WARNINGS", "1")
        .env_remove("LIBREPCB_UPGRADE_UNSTABLE")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "librepcb-cli {args:?} failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// File name patterns of the exports besides the fabrication data (the
/// CLI substitutes the attributes, see [`file_name()`]).
const PNP_TOP_CSV: &str = "pnp_top_{{BOARD_INDEX}}_{{VARIANT_INDEX}}.csv";
const PNP_TOP_GBR: &str = "pnp_top_{{BOARD_INDEX}}_{{VARIANT_INDEX}}.gbr";
const PNP_BOT_CSV: &str = "pnp_bot_{{BOARD_INDEX}}_{{VARIANT_INDEX}}.csv";
const PNP_BOT_GBR: &str = "pnp_bot_{{BOARD_INDEX}}_{{VARIANT_INDEX}}.gbr";
const NETLIST: &str = "netlist_{{BOARD_INDEX}}.d356";

fn file_name(pattern: &str, board: usize, variant: usize) -> String {
    pattern
        .replace("{{BOARD_INDEX}}", &board.to_string())
        .replace("{{VARIANT_INDEX}}", &variant.to_string())
}

/// The fabrication output settings of the two export runs: default
/// settings (separate PTH/NPTH drill files) and merged drill files, each
/// with one output directory per board (multiple boards would overwrite
/// each other's files with the default output path).
fn fabrication_settings() -> [BoardFabricationOutputSettings; 2] {
    let separate = BoardFabricationOutputSettings {
        output_base_path: "./output/{{BOARD_DIRNAME}}/{{PROJECT}}".into(),
        ..Default::default()
    };
    let merged = BoardFabricationOutputSettings {
        output_base_path: "./output_merged/{{BOARD_DIRNAME}}/{{PROJECT}}".into(),
        merge_drill_files: true,
        ..Default::default()
    };
    [separate, merged]
}

/// Writes fabrication output settings into a file for the CLI option
/// `--pcb-fabrication-settings`.
fn write_settings_file(settings: &BoardFabricationOutputSettings, file: &Path) {
    let mut root = List::new("fabrication_output_settings");
    settings.serialize(&mut root);
    let content = SExpression::from(root)
        .to_byte_array(Mode::LibrePcb)
        .unwrap();
    std::fs::write(file, content).unwrap();
}

/// Exports everything like the CLI invocations in [`export_upstream()`].
fn export_ours(project_dir: &Path, lpp: &str, out_dir: &Path) {
    let fs = TransactionalFileSystem::open_ro(&FilePath::new(project_dir).unwrap()).unwrap();
    let mut project = ProjectLoader::new()
        .open(TransactionalDirectory::new(Arc::new(fs), ""), lpp)
        .unwrap();
    let info = ExportInfo::now("0.0.0");
    let out = |name: String| FilePath::new(out_dir.join(name)).unwrap();
    let boards: Vec<_> = project.boards().iter().map(|b| b.id()).collect();
    for settings in fabrication_settings() {
        for board in &boards {
            export_fabrication_data(&mut project, *board, Some(&settings), &info).unwrap();
        }
    }
    let variants: Vec<AssemblyVariantId> = project
        .circuit()
        .assembly_variants()
        .iter()
        .map(|av| AssemblyVariantId(av.uuid()))
        .collect();
    for (side, csv, gbr) in [
        (BoardSide::Top, PNP_TOP_CSV, PNP_TOP_GBR),
        (BoardSide::Bottom, PNP_BOT_CSV, PNP_BOT_GBR),
    ] {
        for (b, board) in boards.iter().enumerate() {
            for (v, av) in variants.iter().enumerate() {
                let sides = match side {
                    BoardSide::Top => PickPlaceSides::Top,
                    BoardSide::Bottom => PickPlaceSides::Bottom,
                };
                export_pick_place_csv(
                    &project,
                    *board,
                    *av,
                    sides,
                    &out(file_name(csv, b, v)),
                    &info.app_version,
                    Timestamp::now(),
                )
                .unwrap();
                export_component_layer(
                    &project,
                    *board,
                    side,
                    *av,
                    &out(file_name(gbr, b, v)),
                    &info,
                )
                .unwrap();
            }
        }
    }
    for (b, board) in boards.iter().enumerate() {
        export_d356_netlist(&project, *board, &out(file_name(NETLIST, b, 0)), &info).unwrap();
    }
}

fn export_upstream(cli: &Path, project_dir: &Path, lpp: &str, out_dir: &Path) {
    let lpp = project_dir.join(lpp);
    let [separate, merged] = fabrication_settings();
    let separate_file = out_dir.join("separate.lp");
    let merged_file = out_dir.join("merged.lp");
    write_settings_file(&separate, &separate_file);
    write_settings_file(&merged, &merged_file);
    run_cli(
        cli,
        out_dir,
        &[
            "open-project",
            lpp.to_str().unwrap(),
            "--export-pcb-fabrication-data",
            "--pcb-fabrication-settings",
            merged_file.to_str().unwrap(),
        ],
    );
    std::fs::remove_file(&merged_file).unwrap();
    run_cli(
        cli,
        out_dir,
        &[
            "open-project",
            lpp.to_str().unwrap(),
            "--export-pcb-fabrication-data",
            "--pcb-fabrication-settings",
            separate_file.to_str().unwrap(),
            "--export-pnp-top",
            PNP_TOP_CSV,
            "--export-pnp-top",
            PNP_TOP_GBR,
            "--export-pnp-bottom",
            PNP_BOT_CSV,
            "--export-pnp-bottom",
            PNP_BOT_GBR,
            "--export-netlist",
            NETLIST,
        ],
    );
    std::fs::remove_file(&separate_file).unwrap();
}

/// Returns all files in `dir` (relative paths, recursively).
fn files(dir: &Path) -> BTreeSet<PathBuf> {
    if !dir.exists() {
        return BTreeSet::new();
    }
    walkdir::WalkDir::new(dir)
        .into_iter()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().strip_prefix(dir).unwrap().to_path_buf())
        .collect()
}

/// Removes the volatile lines (generation software, creation date and the
/// MD5 checksum depending on them).
fn normalize(content: &[u8]) -> String {
    String::from_utf8_lossy(content)
        .split_inclusive('\n')
        .filter(|l| {
            ![
                "TF.GenerationSoftware",
                "TF.CreationDate",
                "TF.MD5",
                "Generation Software:",
                "Generation Date:",
            ]
            .iter()
            .any(|s| l.contains(s))
        })
        .collect()
}

/// Describes the first difference between two files.
fn describe_diff(expected: &str, actual: &str) -> String {
    let mut e = expected.lines();
    let mut a = actual.lines();
    for line in 1.. {
        match (e.next(), a.next()) {
            (None, None) => break,
            (el, al) if el != al => {
                return format!(
                    "first difference in line {line}:\n  upstream: {}\n  ours:     {}",
                    el.unwrap_or("<EOF>"),
                    al.unwrap_or("<EOF>")
                );
            }
            _ => {}
        }
    }
    "files differ (line endings)".into()
}

fn compare(name: &str, upstream: &Path, ours: &Path, failures: &mut Vec<String>) -> usize {
    let mut identical = 0;
    for file in files(upstream).union(&files(ours)) {
        let expected = std::fs::read(upstream.join(file)).ok();
        let actual = std::fs::read(ours.join(file)).ok();
        let (Some(expected), Some(actual)) = (&expected, &actual) else {
            let which = if expected.is_some() {
                "ours"
            } else {
                "upstream"
            };
            failures.push(format!("{name}/{}: missing in {which}", file.display()));
            continue;
        };
        let (expected, actual) = (normalize(expected), normalize(actual));
        if expected == actual {
            identical += 1;
        } else {
            failures.push(format!(
                "{name}/{}: {}",
                file.display(),
                describe_diff(&expected, &actual)
            ));
        }
    }
    identical
}

#[test]
fn export_like_upstream_cli() {
    let Some(cli) = upstream_cli() else {
        eprintln!(
            "librepcb-cli not found (set LIBREPCB_CLI), skipping the comparison with upstream"
        );
        return;
    };
    let mut tmp = tempfile::Builder::new()
        .prefix("librepcb-board-export-")
        .tempdir()
        .unwrap();
    // Set LIBREPCB_KEEP_EXPORT_OUTPUT=1 to inspect the exported files.
    if std::env::var_os("LIBREPCB_KEEP_EXPORT_OUTPUT").is_some_and(|v| v == "1") {
        tmp.disable_cleanup(true);
        println!("exported files are kept in {}", tmp.path().display());
    }
    let mut failures = Vec::new();
    let mut identical = 0;
    let mut projects = Vec::new();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(data_dir().join("projects"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("boards").is_dir())
        .collect();
    dirs.sort();
    for src in dirs {
        let name = src.file_name().unwrap().to_string_lossy().into_owned();
        let Some(lpp) = std::fs::read_dir(&src)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .find(|n| n.ends_with(".lpp"))
        else {
            continue;
        };
        let mut dirs = Vec::new();
        for side in ["upstream", "ours"] {
            let project = tmp.path().join(side).join(&name).join("project");
            let out = tmp.path().join(side).join(&name).join("out");
            file_utils::copy_dir_recursively(
                &FilePath::new(src.canonicalize().unwrap()).unwrap(),
                &FilePath::new(&project).unwrap(),
            )
            .unwrap();
            std::fs::create_dir_all(&out).unwrap();
            dirs.push((project, out));
        }
        export_upstream(&cli, &dirs[0].0, &lpp, &dirs[0].1);
        export_ours(&dirs[1].0, &lpp, &dirs[1].1);
        for output in ["output", "output_merged"] {
            identical += compare(
                &format!("{name}/{output}"),
                &dirs[0].0.join(output),
                &dirs[1].0.join(output),
                &mut failures,
            );
        }
        identical += compare(&name, &dirs[0].1, &dirs[1].1, &mut failures);
        projects.push(name);
    }
    println!("compared projects: {projects:?}, {identical} identical files");
    assert!(identical > 0);
    assert!(
        failures.is_empty(),
        "{} files differ from upstream:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
