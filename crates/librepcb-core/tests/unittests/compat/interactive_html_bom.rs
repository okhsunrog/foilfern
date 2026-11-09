//! Compares the interactive HTML BOMs of all projects in the upstream test
//! data with the ones of the official `librepcb-cli` (output job
//! `interactive_bom` with non-default options and custom attributes).
//!
//! The generation date embedded in the (compressed) BOM data is taken from
//! the upstream file, so the HTML files must be byte-identical.
//!
//! The upstream CLI is taken from the environment variable `LIBREPCB_CLI`,
//! or `librepcb-cli` in `PATH`. Without it, the comparison is skipped (with
//! a message on stderr).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use librepcb_core::export::Timestamp;
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::job::OutputJobList;
use librepcb_core::project::board::ExportInfo;
use librepcb_core::project::{OutputJobRunner, ProjectLoader};
use librepcb_core::serialization::{DeserializeObject, Mode, SExpression};

const JOBS: &str = r#"(librepcb_jobs
 (job 71b1ed0e-5e5b-4c49-a38c-eb0457c30a36 (name "Interactive BOM")
  (type interactive_bom)
  (view_mode top_bottom) (highlight_pin1 all) (dark_mode true)
  (rotation 90.0) (offset_back_rotation true)
  (show_silkscreen true) (show_fabrication false)
  (show_pads true) (show_tracks true) (show_zones true)
  (checkboxes "Sourced,Placed,Checked")
  (component_order "C,R,L,D,U,Y,X,F")
  (custom_attribute "MANUFACTURER")
  (custom_attribute "TOLERANCE")
  (board all)
  (variant all)
  (output "{{BOARD_INDEX}}_{{VARIANT}}.html")
 )
)
"#;

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

fn project_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("LIBREPCB_UPSTREAM_DIR"))
        .join("tests/data/projects")
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

/// Returns the generation date stored in the compressed BOM data of an HTML
/// file.
fn generation_date(html: &str) -> chrono::NaiveDateTime {
    let start = "LZString.decompressFromBase64(\"";
    let begin = html.find(start).expect("no BOM data") + start.len();
    let end = begin + html[begin..].find('"').unwrap();
    let data = lz_str::decompress_from_base64(&html[begin..end]).expect("invalid BOM data");
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf16(&data).unwrap()).unwrap();
    let date = json["metadata"]["date"].as_str().unwrap();
    chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M:%S").unwrap()
}

#[test]
fn interactive_html_bom_like_upstream_cli() {
    let Some(cli) = upstream_cli() else {
        eprintln!(
            "librepcb-cli not found (set LIBREPCB_CLI), skipping the comparison with upstream"
        );
        return;
    };
    let mut tmp = tempfile::Builder::new()
        .prefix("librepcb-ibom-")
        .tempdir()
        .unwrap();
    // Set LIBREPCB_KEEP_EXPORT_OUTPUT=1 to inspect the exported files.
    if std::env::var_os("LIBREPCB_KEEP_EXPORT_OUTPUT").is_some_and(|v| v == "1") {
        tmp.disable_cleanup(true);
        println!("exported files are kept in {}", tmp.path().display());
    }
    let jobs_file = tmp.path().join("jobs.lp");
    std::fs::write(&jobs_file, JOBS).unwrap();
    let jobs = OutputJobList::deserialize(
        &SExpression::parse(JOBS.as_bytes(), None, Mode::LibrePcb).unwrap(),
    )
    .unwrap();
    let jobs: Vec<_> = jobs.iter().cloned().collect();

    let mut failures = Vec::new();
    let mut compared = 0;
    for (index, src) in project_dirs().iter().enumerate() {
        let name = src.file_name().unwrap().to_string_lossy().into_owned();
        let work = tmp.path().join(index.to_string());
        let copy = work.join("project");
        let theirs = work.join("theirs");
        copy_dir(src, &copy);
        let lpp = copy.join(lpp_name(&copy));
        let out = Command::new(&cli)
            .args([
                "open-project",
                "--jobs",
                jobs_file.to_str().unwrap(),
                "--run-jobs",
                "--outdir",
                theirs.to_str().unwrap(),
                lpp.to_str().unwrap(),
            ])
            .env("QT_QPA_PLATFORM", "offscreen")
            .env("LC_ALL", "C")
            .env_remove("LIBREPCB_UPGRADE_UNSTABLE")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let mut files: Vec<_> = std::fs::read_dir(&theirs)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "html"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "{name}: no HTML file written");

        // Upstream takes the current time for each file, so run our jobs once
        // per distinct date.
        let mut dates: Vec<_> = files
            .iter()
            .map(|f| (generation_date(&std::fs::read_to_string(f).unwrap()), f))
            .collect();
        dates.sort();
        let mut ours_by_date = std::collections::BTreeMap::new();
        for (date, file) in dates {
            let ours = ours_by_date.entry(date).or_insert_with(|| {
                let ours = work.join(format!("ours_{}", date.format("%H%M%S")));
                let fs = TransactionalFileSystem::open_rw(&FilePath::new(&copy).unwrap()).unwrap();
                let mut project = ProjectLoader::new()
                    .open(
                        TransactionalDirectory::new(Arc::new(fs), ""),
                        &lpp_name(&copy),
                    )
                    .unwrap();
                let info = ExportInfo {
                    app_version: "2.1.1".to_owned(),
                    creation_date: Timestamp::Local(date),
                };
                let mut runner = OutputJobRunner::new(&mut project, info).unwrap();
                runner.set_output_directory(&FilePath::new(&ours).unwrap());
                runner.run(&jobs).unwrap();
                ours
            });
            let file_name = file.file_name().unwrap();
            let expected = std::fs::read(file).unwrap();
            match std::fs::read(ours.join(file_name)) {
                Ok(actual) if actual == expected => compared += 1,
                Ok(_) => failures.push(format!("{name}: {file_name:?} differs")),
                Err(e) => failures.push(format!("{name}: {file_name:?}: {e}")),
            }
        }
    }
    println!("compared {compared} interactive HTML BOMs");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(compared >= 30, "too few files compared");
}
