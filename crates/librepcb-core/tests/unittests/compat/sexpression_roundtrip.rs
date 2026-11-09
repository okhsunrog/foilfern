//! Parses every S-expression file in `../LibrePCB/tests/data` and checks that
//! re-serializing it yields byte-identical output where this is expected.
//!
//! Which files must round-trip:
//!
//! - LibrePCB files (`*.lp`) in the *current* file format. Upstream
//!   determines the file format of a library element, project or workspace
//!   data directory by the version file `.librepcb-<type>` (e.g.
//!   `.librepcb-sym`, `.librepcb-project`, `.librepcb-data`) in its root
//!   directory, whose first line is the version (`VersionFile`). The format
//!   of a `.lp` file is therefore given by the version file in the nearest
//!   ancestor directory; the current format is `LIBREPCB_FILE_FORMAT_VERSION`
//!   = 2 (see upstream `CMakeLists.txt`).
//! - `.lp` files without any version file: only a unit test fixture which is
//!   written by the current serializer (`BoardPlaneFragmentsBuilderTest`).
//!
//! Excluded from the byte-identical check (but they must still parse):
//!
//! - `.lp` files in older file formats (0.1 and 1). They are only read after
//!   file format migration, and were written by older serializers whose
//!   formatting may differ (most of them happen to be identical anyway).
//! - The files in [`EXCLUDED`], see the reasons given there.
//! - Files of other tools (KiCad `.kicad_sym`/`.kicad_mod`, FreeRouting
//!   `.ses` sessions), which are parsed in permissive mode only.

use std::path::{Path, PathBuf};

use librepcb_core::serialization::{Mode, SExpression};
use librepcb_core::types::Version;

/// Upstream `LIBREPCB_FILE_FORMAT_VERSION`.
const CURRENT_FILE_FORMAT: &str = "2";

/// Files (relative to the test data directory) which cannot round-trip even
/// with upstream, with the reason.
const EXCLUDED: &[(&str, &str)] = &[
    (
        "workspaces/Empty Workspace/data/settings.lp",
        "hand-written `;` comment, comments are dropped when parsing",
    ),
    (
        "unittests/librepcbproject/BoardSpecctraExportTest/expected.dsn",
        "contains the token `\"` (`(string_quote \")`), which the parser reads \
         as start of a string, so the file cannot be re-read as written",
    ),
];

/// Upstream test data directory (see `LIBREPCB_UPSTREAM_DIR` in
/// `.cargo/config.toml`).
fn data_dir() -> PathBuf {
    Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files(&path, files);
        } else {
            files.push(path);
        }
    }
}

/// Returns the file format version given by the version file in the nearest
/// ancestor directory (upstream `VersionFile::fromByteArray()`), if any.
fn file_format_version(file: &Path, root: &Path) -> Option<Version> {
    for dir in file.ancestors().skip(1).take_while(|d| d.starts_with(root)) {
        let version_file = std::fs::read_dir(dir).ok()?.find_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with(".librepcb-") && path.is_file()).then_some(path)
        });
        if let Some(path) = version_file {
            let content = std::fs::read_to_string(&path).unwrap();
            let first_line = content.split('\n').next().unwrap_or_default();
            return Some(
                first_line
                    .parse()
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display())),
            );
        }
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
enum Expectation {
    /// Must parse and re-serialize byte-identically in the given mode.
    Roundtrip(Mode),
    /// Must parse in the given mode; formatting may differ.
    ParseOnly(Mode, &'static str),
}

fn expectation(file: &Path, root: &Path) -> Option<Expectation> {
    let current: Version = CURRENT_FILE_FORMAT.parse().unwrap();
    let rel = file.strip_prefix(root).ok()?;
    let excluded = EXCLUDED
        .iter()
        .find(|(path, _)| rel == Path::new(path))
        .map(|(_, reason)| *reason);
    let expectation = match file.extension()?.to_str()? {
        "lp" => Some(match file_format_version(file, root) {
            Some(version) if version == current => Expectation::Roundtrip(Mode::LibrePcb),
            Some(_) => Expectation::ParseOnly(Mode::LibrePcb, "older file format"),
            None => Expectation::Roundtrip(Mode::LibrePcb),
        }),
        "dsn" => Some(Expectation::Roundtrip(Mode::Permissive)),
        "ses" | "kicad_sym" | "kicad_mod" => {
            Some(Expectation::ParseOnly(Mode::Permissive, "other tool"))
        }
        _ => None,
    }?;
    Some(match (expectation, excluded) {
        (Expectation::Roundtrip(mode), Some(reason)) => Expectation::ParseOnly(mode, reason),
        (expectation, _) => expectation,
    })
}

#[test]
fn roundtrip_test_data() {
    let root = data_dir()
        .canonicalize()
        .expect("upstream test data not found (set LIBREPCB_UPSTREAM_DIR)");
    let mut files = Vec::new();
    collect_files(&root, &mut files);

    let mut roundtrip_ok = 0;
    let mut parse_only_ok = 0;
    let mut parse_only_identical = 0;
    let mut failures = Vec::new();
    for file in &files {
        let Some(expectation) = expectation(file, &root) else {
            continue;
        };
        let rel = file.strip_prefix(&root).unwrap().display().to_string();
        let content = std::fs::read(file).unwrap();
        let mode = match expectation {
            Expectation::Roundtrip(mode) | Expectation::ParseOnly(mode, _) => mode,
        };
        let parsed = match SExpression::parse(&content, Some(file), mode) {
            Ok(parsed) => parsed,
            Err(e) => {
                failures.push(format!("{rel}: parse error: {e}"));
                continue;
            }
        };
        let output = parsed.to_byte_array(mode);
        match expectation {
            Expectation::Roundtrip(_) => match output {
                Ok(output) if output == content => roundtrip_ok += 1,
                Ok(output) => {
                    let pos = output
                        .iter()
                        .zip(&content)
                        .position(|(a, b)| a != b)
                        .unwrap_or(output.len().min(content.len()));
                    failures.push(format!("{rel}: output differs at byte {pos}"));
                }
                Err(e) => failures.push(format!("{rel}: serialize error: {e}")),
            },
            Expectation::ParseOnly(_, reason) => {
                parse_only_ok += 1;
                if output.is_ok_and(|o| o == content) {
                    parse_only_identical += 1;
                } else {
                    println!("not identical ({reason}): {rel}");
                }
            }
        }
    }

    println!(
        "round-trip identical: {roundtrip_ok}, parse-only: {parse_only_ok} \
         (of which {parse_only_identical} happen to be identical too)"
    );
    assert!(failures.is_empty(), "failures:\n{}", failures.join("\n"));
    assert!(roundtrip_ok > 200, "too few files checked: {roundtrip_ok}");
}
