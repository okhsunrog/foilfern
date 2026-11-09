//! Compares this `librepcb-cli` with the official one.
//!
//! Each case copies an upstream test project or library (from
//! `LIBREPCB_UPSTREAM_DIR/tests/data`) twice into a temporary directory,
//! runs the official CLI on one copy and ours on the other one (same
//! arguments, same relative paths), and requires identical console output
//! (stdout and stderr merged, since the official CLI wrapper merges them),
//! identical exit codes and identical files afterwards. Excluded from the
//! file comparison are the lines containing the generating software version,
//! the creation date and the Gerber MD5 checksum over them, and the HTML
//! migration logs (they contain date and version). ZIP files (`*.zip`,
//! `*.lppz`) are compared by content, since the entry order differs (see
//! COMPAT.md).
//!
//! The official CLI is taken from the environment variable `LIBREPCB_CLI`,
//! or `librepcb-cli` in `PATH`. Without it, the comparison is skipped (with
//! a message on stderr). Graphics exports (other bytes than Qt's) and
//! unsupported features (STEP, DRC) are not compared.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Runs a CLI with `args` in `cwd` like the upstream tests do (English,
/// separate config directory), with stderr merged into stdout. Returns the
/// exit code and the output.
fn run(cli: &Path, cwd: &Path, args: &[String]) -> (i32, String) {
    let out = Command::new("sh")
        .arg("-c")
        .arg("exec \"$0\" \"$@\" 2>&1")
        .arg(cli)
        .args(args)
        .current_dir(cwd)
        .env("QT_QPA_PLATFORM", "offscreen")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .env("LIBREPCB_CONFIG_DIR", cwd.join("config"))
        .env("LIBREPCB_DISABLE_UNSTABLE_WARNING", "1")
        .env("LIBREPCB_SUPPRESS_DEPRECATION_WARNINGS", "1")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// Copies a directory recursively.
fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Returns all files below `dir` (relative paths with `/`) and their
/// content.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, result: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, result);
            } else {
                let rel = path.strip_prefix(root).unwrap();
                let rel = rel.to_string_lossy().replace('\\', "/");
                result.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(dir, dir, &mut result);
    result
}

/// Removes the lines which legitimately differ (version, dates, checksum).
fn normalize(content: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(content) else {
        return content.to_vec();
    };
    const IGNORED: &[&str] = &[
        "Generation Software",
        "Generation Date",
        "GenerationSoftware",
        "CreationDate",
        "TF.MD5",
    ];
    text.split('\n')
        .filter(|line| !IGNORED.iter().any(|i| line.contains(i)))
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

/// Returns the (normalized) entries of a ZIP file, without migration logs.
fn zip_entries(content: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(content)).unwrap();
    let mut result = BTreeMap::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).unwrap();
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();
        if !file.name().starts_with("logs/") {
            result.insert(file.name().to_owned(), normalize(&data));
        }
    }
    result
}

/// Compares the files of two directories, returns the differences.
fn compare_dirs(upstream: &Path, ours: &Path) -> Vec<String> {
    let a = files(upstream);
    let b = files(ours);
    let mut diffs = Vec::new();
    for name in a
        .keys()
        .chain(b.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        if name.starts_with("config/") || name.contains("/logs/") || name.starts_with("logs/") {
            continue;
        }
        match (a.get(name), b.get(name)) {
            (Some(x), Some(y)) => {
                let equal = if name.ends_with(".zip") || name.ends_with(".lppz") {
                    zip_entries(x) == zip_entries(y)
                } else {
                    normalize(x) == normalize(y)
                };
                if !equal {
                    diffs.push(format!("differs: {name}"));
                }
            }
            (Some(_), None) => diffs.push(format!("missing: {name}")),
            (None, Some(_)) => diffs.push(format!("unexpected: {name}")),
            (None, None) => unreachable!(),
        }
    }
    diffs
}

/// A comparison case.
struct Case {
    /// Name for messages.
    name: &'static str,
    /// Source directory (relative to `tests/data`) and name of the copy.
    source: String,
    target: &'static str,
    /// Whether the project is zipped into `<target>.lppz`.
    as_lppz: bool,
    /// Additional files written into the working directory.
    extra_files: Vec<(&'static str, String)>,
    args: Vec<String>,
}

fn args(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_owned()).collect()
}

/// Zips a directory (like `shutil.make_archive()` in the upstream tests).
fn zip_dir(src: &Path, dst: &Path) {
    let file = std::fs::File::create(dst).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    for (name, content) in files(src) {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, &content).unwrap();
    }
    zip.finish().unwrap();
}

fn prepare(case: &Case, dir: &Path) {
    let src = data_dir().join(&case.source);
    std::fs::create_dir_all(dir).unwrap();
    if case.as_lppz {
        zip_dir(&src, &dir.join(format!("{}.lppz", case.target)));
    } else {
        copy_dir(&src, &dir.join(case.target));
    }
    for (name, content) in &case.extra_files {
        std::fs::write(dir.join(name), content).unwrap();
    }
}

/// Jobs with the supported job types of "Project With Two Boards".
const SUPPORTED_JOBS: &[&str] = &[
    "Gerber/Excellon",
    "Pick&Place CSV",
    "Pick&Place X3",
    "Netlist",
    "BOM",
    "Custom File",
    "Project Data",
    "Project Archive",
];

/// Custom jobs file with all supported job types and an archive job.
const CUSTOM_JOBS: &str = r#"(librepcb_jobs
 (job ee12ed43-dad6-409b-b57a-92ce95e03ca0 (name "Gerber/Excellon")
  (type gerber_excellon)
  (board all)
  (outlines (suffix "OUTLINES.gbr"))
  (copper_top (suffix "COPPER-TOP.gbr"))
  (copper_inner (suffix "COPPER-IN{{CU_LAYER}}.gbr"))
  (copper_bot (suffix "COPPER-BOTTOM.gbr"))
  (soldermask_top (suffix "SOLDERMASK-TOP.gbr"))
  (soldermask_bot (suffix "SOLDERMASK-BOTTOM.gbr"))
  (silkscreen_top (suffix "SILKSCREEN-TOP.gbr"))
  (silkscreen_bot (suffix "SILKSCREEN-BOTTOM.gbr"))
  (solderpaste_top (create true) (suffix "SOLDERPASTE-TOP.gbr"))
  (solderpaste_bot (create false) (suffix "SOLDERPASTE-BOTTOM.gbr"))
  (drills (merge true) (suffix_pth "DRILLS-PTH.drl") (suffix_npth "DRILLS-NPTH.drl")
   (suffix_merged "DRILLS.drl") (suffix_buried "DRILLS-{{START_LAYER}}-{{END_LAYER}}.drl")
   (g85_slots false)
  )
  (output "gbr/{{BOARD}}/{{PROJECT}}_")
 )
 (job 6bf1fc47-1c52-4819-8709-489c21ef5b6e (name "Pick&Place CSV")
  (type pnp) (comment false)
  (tht true) (smt false) (mixed true) (fiducial false) (other true)
  (board all)
  (variant all)
  (top (create true) (output "asm/{{BOARD}}_{{VARIANT}}_TOP.csv"))
  (bottom (create true) (output "asm/{{BOARD}}_{{VARIANT}}_BOT.csv"))
  (both (create true) (output "asm/{{BOARD}}_{{VARIANT}}_BOTH.csv"))
 )
 (job 1f97479b-81a9-42b7-9e74-d6105a0ae811 (name "BOM")
  (type bom)
  (custom_attributes "MPN" "SUPPLIER[]")
  (board all)
  (variant all)
  (output "asm/{{BOARD or 'none'}}_BOM_{{VARIANT}}.csv")
 )
 (job 0aaff25b-cde0-4d98-9c76-c3d2501ae3f3 (name "Custom File")
  (type copy) (substitute_variables false)
  (board all)
  (variant all)
  (input "./resources/template.txt")
  (output "copy/{{BOARD}}_{{VARIANT}}.txt")
 )
 (job f448a955-9c13-43e2-a17c-d694e6169993 (name "ZIP")
  (type archive)
  (input 6bf1fc47-1c52-4819-8709-489c21ef5b6e (destination "pnp"))
  (input ee12ed43-dad6-409b-b57a-92ce95e03ca0 (destination ""))
  (input 1f97479b-81a9-42b7-9e74-d6105a0ae811 (destination "a/b"))
  (output "{{PROJECT}}_{{VERSION}}.zip")
 )
)
"#;

fn cases() -> Vec<Case> {
    let two_boards = "projects/Project With Two Boards";
    let two_boards_lpp = "Project With Two Boards/Project With Two Boards.lpp";
    let mut supported_jobs = Vec::new();
    for job in SUPPORTED_JOBS {
        supported_jobs.push("--run-job".to_owned());
        supported_jobs.push((*job).to_owned());
    }
    let deprecated_exports = |project: &str| {
        args(&[
            "open-project",
            "--erc",
            "--export-bom=bom/{{VARIANT}}.csv",
            "--export-board-bom=bom/{{BOARD}}_{{VARIANT}}.csv",
            "--bom-attributes=MPN, SUPPLIER[]",
            "--export-pnp-top=pnp/{{BOARD}}_{{VARIANT}}_top.csv",
            "--export-pnp-bottom=pnp/{{BOARD}}_{{VARIANT}}_bot.gbr",
            "--export-netlist=netlist/{{BOARD}}.d356",
            "--export-pcb-fabrication-data",
            "--pcb-fabrication-settings=fab.lp",
            project,
        ])
    };
    let fab_settings = r#"(fabrication_output_settings
 (base_path "./fab/{{BOARD_DIRNAME}}/{{PROJECT}}")
 (outlines (suffix "_OUTLINES.gbr"))
 (copper_top (suffix "_COPPER-TOP.gbr"))
 (copper_inner (suffix "_COPPER-IN{{CU_LAYER}}.gbr"))
 (copper_bot (suffix "_COPPER-BOTTOM.gbr"))
 (soldermask_top (suffix "_SOLDERMASK-TOP.gbr"))
 (soldermask_bot (suffix "_SOLDERMASK-BOTTOM.gbr"))
 (silkscreen_top (suffix "_SILKSCREEN-TOP.gbr"))
 (silkscreen_bot (suffix "_SILKSCREEN-BOTTOM.gbr"))
 (drills (merge false) (suffix_pth "_DRILLS-PTH.drl") (suffix_npth "_DRILLS-NPTH.drl")
  (suffix_merged "_DRILLS.drl") (suffix_buried "_DRILLS-{{START_LAYER}}-{{END_LAYER}}.drl")
  (g85_slots false)
 )
 (solderpaste_top (create true) (suffix "_SOLDERPASTE-TOP.gbr"))
 (solderpaste_bot (create true) (suffix "_SOLDERPASTE-BOTTOM.gbr"))
)
"#;
    let mut cases = vec![
        Case {
            name: "supported jobs",
            source: two_boards.to_owned(),
            target: "Project With Two Boards",
            as_lppz: false,
            extra_files: Vec::new(),
            args: [
                args(&["open-project"]),
                supported_jobs.clone(),
                args(&[two_boards_lpp]),
            ]
            .concat(),
        },
        Case {
            name: "supported jobs of *.lppz",
            source: two_boards.to_owned(),
            target: "Project With Two Boards",
            as_lppz: true,
            extra_files: Vec::new(),
            args: [
                args(&["open-project"]),
                supported_jobs,
                args(&["Project With Two Boards.lppz"]),
            ]
            .concat(),
        },
        Case {
            name: "custom jobs with archive",
            source: two_boards.to_owned(),
            target: "Project With Two Boards",
            as_lppz: false,
            extra_files: vec![("jobs.lp", CUSTOM_JOBS.to_owned())],
            args: args(&[
                "open-project",
                "--run-jobs",
                "--jobs=jobs.lp",
                "--outdir=out",
                two_boards_lpp,
            ]),
        },
        Case {
            name: "migration, ERC and save (v0.1)",
            source: "projects/v0.1".to_owned(),
            target: "p",
            as_lppz: false,
            extra_files: Vec::new(),
            args: args(&["open-project", "--erc", "--save", "p/test project.lpp"]),
        },
        Case {
            name: "migration and save of *.lppz (v1)",
            source: "projects/v1".to_owned(),
            target: "p",
            as_lppz: true,
            extra_files: Vec::new(),
            args: args(&[
                "open-project",
                "--set-default-variant=AV",
                "--board-index=0",
                "--remove-other-boards",
                "--save",
                "p.lppz",
            ]),
        },
        Case {
            name: "library check",
            source: "libraries/Populated Library.lplib".to_owned(),
            target: "lib.lplib",
            as_lppz: false,
            extra_files: Vec::new(),
            args: args(&["open-library", "--all", "--check", "--strict", "lib.lplib"]),
        },
        Case {
            name: "library migration and save",
            source: "libraries/v0.1.lplib".to_owned(),
            target: "lib.lplib",
            as_lppz: false,
            extra_files: Vec::new(),
            args: args(&["open-library", "--all", "--check", "--save", "lib.lplib"]),
        },
    ];
    for project in [
        "DRC",
        "Gerber Test",
        "Nested Planes",
        "Project With Two Boards",
    ] {
        let lpp = std::fs::read_dir(data_dir().join("projects").join(project))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .find(|n| n.ends_with(".lpp"))
            .unwrap();
        cases.push(Case {
            name: "deprecated exports",
            source: format!("projects/{project}"),
            target: "p",
            as_lppz: false,
            extra_files: vec![("fab.lp", fab_settings.to_owned())],
            args: deprecated_exports(&format!("p/{lpp}")),
        });
    }
    cases
}

#[test]
fn cli_like_upstream_cli() {
    let Some(upstream) = upstream_cli() else {
        eprintln!(
            "librepcb-cli not found (set LIBREPCB_CLI), skipping the comparison with upstream"
        );
        return;
    };
    let ours = PathBuf::from(env!("CARGO_BIN_EXE_librepcb-cli"));
    let tmp = tempfile::Builder::new()
        .prefix("librepcb-cli-compare-")
        .tempdir()
        .unwrap();
    let mut failures = Vec::new();
    for (i, case) in cases().iter().enumerate() {
        let root = tmp.path().join(i.to_string());
        let upstream_dir = root.join("upstream");
        let ours_dir = root.join("ours");
        prepare(case, &upstream_dir);
        prepare(case, &ours_dir);
        let expected = run(&upstream, &upstream_dir, &case.args);
        let actual = run(&ours, &ours_dir, &case.args);
        let label = format!("{} ({})", case.name, case.source);
        if expected != actual {
            failures.push(format!(
                "{label}: console output differs\n--- upstream (exit {}):\n{}\n--- ours (exit {}):\n{}",
                expected.0, expected.1, actual.0, actual.1
            ));
        }
        for diff in compare_dirs(&upstream_dir, &ours_dir) {
            failures.push(format!("{label}: {diff}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Parser errors (e.g. several unknown options) are reported like
/// `QCommandLineParser` does; the executable paths in the usage lines are
/// normalized.
#[test]
fn parser_errors_like_upstream_cli() {
    let Some(upstream) = upstream_cli() else {
        eprintln!(
            "librepcb-cli not found (set LIBREPCB_CLI), skipping the comparison with upstream"
        );
        return;
    };
    let ours = PathBuf::from(env!("CARGO_BIN_EXE_librepcb-cli"));
    let tmp = tempfile::tempdir().unwrap();
    let normalize = |(code, out): (i32, String)| -> (i32, String) {
        let mut out = out;
        let paths: Vec<String> = out
            .split_whitespace()
            .filter(|w| w.ends_with("/librepcb-cli"))
            .map(str::to_owned)
            .collect();
        for path in paths {
            out = out.replace(&path, "librepcb-cli");
        }
        (code, out)
    };
    let mut failures = Vec::new();
    for case in [
        &["open-library", "-abc", "--foo", "x"][..],
        &["open-library", "--foo", "--bar=1", "x"],
        &["--foo", "-x"],
        &["-abc"],
        &["open-project", "-v", "--erc", "--bar", "-q", "p.lpp"],
        &["open-project", "--foo", "--board"],
        &["open-project", "--board", "--foo", "p.lpp", "--", "--bar"],
        &["open-project", "--erc=1", "--foo", "p.lpp"],
        &["open-project", "-xvh", "p.lpp"],
    ] {
        let case = args(case);
        let expected = normalize(run(&upstream, tmp.path(), &case));
        let actual = normalize(run(&ours, tmp.path(), &case));
        if expected != actual {
            failures.push(format!(
                "{case:?}:\n--- upstream (exit {}):\n{}\n--- ours (exit {}):\n{}",
                expected.0, expected.1, actual.0, actual.1
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
