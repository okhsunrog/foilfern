//! Deserializes the footprints, footprint pads, package pads and 3D models
//! of all current-format package files (`package.lp` in libraries and
//! project libraries) of `../LibrePCB/tests/data` and checks that
//! re-serializing them yields byte-identical output. The geometry of the
//! footprint pads is smoke-tested as well.

use std::collections::BTreeMap;
use std::path::{Path as FsPath, PathBuf};

use librepcb_core::geometry::{PadGeometry, PadShape};
use librepcb_core::library::pkg::{Footprint, FootprintPad, PackageModel, PackagePad};
use librepcb_core::serialization::{
    DeserializeObject, List, Mode, Result, SExpression, SerializeObject,
};

/// Upstream `LIBREPCB_FILE_FORMAT_VERSION`.
const CURRENT_FILE_FORMAT: &str = "2";

/// Returns the upstream test data directory (see `LIBREPCB_UPSTREAM_DIR` in
/// `.cargo/config.toml`).
fn data_dir() -> PathBuf {
    PathBuf::from(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

/// Collects all `package.lp` files in current-format package directories
/// (containing a `.librepcb-pkg` version file).
fn collect_package_files(dir: &FsPath, files: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_package_files(&path, files);
        } else if path.file_name().is_some_and(|n| n == "package.lp") {
            let version_file = path.with_file_name(".librepcb-pkg");
            let version = std::fs::read_to_string(version_file).unwrap_or_default();
            if version.split('\n').next() == Some(CURRENT_FILE_FORMAT) {
                files.push(path);
            }
        }
    }
}

fn to_string(node: &SExpression) -> String {
    node.to_string_with_mode(Mode::LibrePcb).unwrap()
}

fn roundtrip<T: DeserializeObject + SerializeObject>(node: &SExpression) -> Result<String> {
    let obj = T::deserialize(node)?;
    let mut list = List::new(node.name()?);
    obj.serialize(&mut list);
    Ok(to_string(&SExpression::List(list)))
}

/// Smoke test of the pad geometry of real footprint pads: every pad has a
/// valid copper area, and its stop mask / solder paste preview geometries
/// can be built.
fn check_pad_geometry(node: &SExpression) -> impl FnOnce(String) -> Result<String> + '_ {
    move |output| {
        let pad = FootprintPad::deserialize(node)?;
        let pad = pad.pad();
        let geometry = pad.geometry();
        let outlines = geometry.to_outlines().expect("pad outline failed");
        assert!(!outlines.is_empty(), "pad without copper:\n{output}");
        assert_eq!(geometry.to_hole_outlines().is_empty(), !pad.is_tht());
        if pad.shape() == PadShape::Custom {
            assert!(PadGeometry::is_valid_custom_outline(
                pad.custom_shape_outline()
            ));
        }
        for geometries in pad.build_preview_geometries().values() {
            for g in geometries {
                g.to_outlines().expect("pad outline failed");
            }
        }
        Ok(output)
    }
}

/// Returns the type name and the re-serialized node, or `None` if the node
/// is none of the checked objects.
fn check_node(parent: &str, node: &SExpression) -> Option<(&'static str, Result<String>)> {
    Some(match (parent, node.name().ok()?) {
        ("librepcb_package", "pad") => ("package pad", roundtrip::<PackagePad>(node)),
        ("librepcb_package", "3d_model") => ("3d model", roundtrip::<PackageModel>(node)),
        ("librepcb_package", "footprint") => ("footprint", roundtrip::<Footprint>(node)),
        ("footprint", "pad") => {
            let has_holes = node.children_named("hole").next().is_some();
            let has_outline = node.children_named("vertex").next().is_some();
            let type_name = match (has_holes, has_outline) {
                (true, true) => "footprint pad (THT, custom outline)",
                (true, false) => "footprint pad (THT)",
                (false, true) => "footprint pad (SMT, custom outline)",
                (false, false) => "footprint pad (SMT)",
            };
            (
                type_name,
                roundtrip::<FootprintPad>(node).and_then(check_pad_geometry(node)),
            )
        }
        _ => return None,
    })
}

fn walk(
    node: &SExpression,
    parent: &str,
    rel: &str,
    counts: &mut BTreeMap<&'static str, usize>,
    failures: &mut Vec<String>,
) {
    if let Some((type_name, result)) = check_node(parent, node) {
        *counts.entry(type_name).or_default() += 1;
        match result {
            Ok(output) => {
                let expected = to_string(node);
                if output != expected {
                    failures.push(format!(
                        "{rel}: {type_name} differs:\n{expected}\n!=\n{output}"
                    ));
                }
            }
            Err(e) => failures.push(format!("{rel}: {type_name}: {e}\n{}", to_string(node))),
        }
    }
    let Ok(name) = node.name() else {
        return;
    };
    for child in node.children().iter().filter(|c| c.is_list()) {
        walk(child, name, rel, counts, failures);
    }
}

#[test]
fn package_roundtrip_test_data() {
    let root = data_dir()
        .canonicalize()
        .expect("upstream test data not found (expected at ../LibrePCB/tests/data)");
    let mut files = Vec::new();
    collect_package_files(&root, &mut files);
    assert!(
        files.len() >= 20,
        "only {} package files found",
        files.len()
    );

    let mut counts = BTreeMap::new();
    let mut failures = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(&root).unwrap().display().to_string();
        let content = std::fs::read(file).unwrap();
        let tree = SExpression::parse(&content, Some(file), Mode::LibrePcb).unwrap();
        walk(&tree, "", &rel, &mut counts, &mut failures);
    }

    println!(
        "checked objects in {} package files: {counts:?}",
        files.len()
    );
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    for type_name in [
        "package pad",
        "footprint",
        "footprint pad (SMT)",
        "footprint pad (THT)",
    ] {
        assert!(
            counts.get(type_name).is_some_and(|c| *c > 0),
            "no {type_name} found in test data"
        );
    }
}
