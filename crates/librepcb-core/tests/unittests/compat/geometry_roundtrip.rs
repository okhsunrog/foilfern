//! Deserializes all geometry objects found in the current-format `.lp` files
//! of `../LibrePCB/tests/data` (library elements, schematics, boards) and
//! checks that re-serializing them yields byte-identical output.
//!
//! Objects are recognized by their list name and the name of the parent list
//! (e.g. `hole` in `pad` is a [`PadHole`], otherwise a [`Hole`]). Pads are
//! only deserialized here (serialization belongs to footprint/board pads;
//! footprint pads are round-tripped in `package_roundtrip.rs`).

use std::collections::BTreeMap;
use std::path::{Path as FsPath, PathBuf};

use librepcb_core::geometry::{
    Circle, Hole, Image, Junction, NetLabel, NetLine, Pad, PadHole, Polygon, StrokeText, Text,
    Trace, Vertex, Via, Zone,
};
use librepcb_core::serialization::{
    DeserializeObject, List, Mode, Result, SExpression, SerializeObject,
};

/// Upstream `LIBREPCB_FILE_FORMAT_VERSION`.
const CURRENT_FILE_FORMAT: &str = "2";

/// Upstream test data directory (see `LIBREPCB_UPSTREAM_DIR` in
/// `.cargo/config.toml`).
fn data_dir() -> PathBuf {
    FsPath::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")
}

fn collect_files(dir: &FsPath, files: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files(&path, files);
        } else if path.extension().is_some_and(|e| e == "lp") {
            files.push(path);
        }
    }
}

/// Returns whether the file is in the current file format (version file in
/// the nearest ancestor directory, see `sexpression_roundtrip.rs`).
fn is_current_format(file: &FsPath, root: &FsPath) -> bool {
    for dir in file.ancestors().skip(1).take_while(|d| d.starts_with(root)) {
        let version_file = std::fs::read_dir(dir).unwrap().find_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?.to_owned();
            (name.starts_with(".librepcb-") && path.is_file()).then_some(path)
        });
        if let Some(path) = version_file {
            let content = std::fs::read_to_string(path).unwrap();
            return content.split('\n').next() == Some(CURRENT_FILE_FORMAT);
        }
    }
    true
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

/// Returns the type name and the re-serialized node (`None` for
/// deserialize-only types), or `None` if the node is no geometry object.
fn check_node(parent: &str, node: &SExpression) -> Option<(&'static str, Result<Option<String>>)> {
    let name = node.name().ok()?;
    let serialized = |r: Result<String>| r.map(Some);
    // Parents whose children are plain geometry objects. Board level objects
    // (`librepcb_board`, `device`) are wrappers with additional attributes
    // (e.g. `lock`), and `text` in `pin` or the DRC approvals (`approved`,
    // `object`, `drill`) are no geometry objects.
    let geometry_parent = matches!(
        parent,
        "librepcb_symbol" | "librepcb_schematic" | "symbol" | "footprint"
    );
    Some(match (parent, name) {
        (_, "vertex") => ("vertex", serialized(roundtrip::<Vertex>(node))),
        ("pad", "hole") => ("pad hole", serialized(roundtrip::<PadHole>(node))),
        (_, _) if !geometry_parent && parent != "netsegment" => return None,
        (_, "circle") => ("circle", serialized(roundtrip::<Circle>(node))),
        (_, "polygon") => ("polygon", serialized(roundtrip::<Polygon>(node))),
        (_, "stroke_text") => ("stroke_text", serialized(roundtrip::<StrokeText>(node))),
        (_, "text") => ("text", serialized(roundtrip::<Text>(node))),
        (_, "image") => ("image", serialized(roundtrip::<Image>(node))),
        (_, "hole") => ("hole", serialized(roundtrip::<Hole>(node))),
        ("footprint", "zone") => ("zone", serialized(roundtrip::<Zone>(node))),
        ("footprint" | "netsegment", "pad") => ("pad", Pad::deserialize(node).map(|_| None)),
        ("netsegment", "junction") => ("junction", serialized(roundtrip::<Junction>(node))),
        ("netsegment", "label") => ("net label", serialized(roundtrip::<NetLabel>(node))),
        ("netsegment", "line") => ("net line", serialized(roundtrip::<NetLine>(node))),
        ("netsegment", "via") => ("via", serialized(roundtrip::<Via>(node))),
        ("netsegment", "trace") => ("trace", serialized(roundtrip::<Trace>(node))),
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
            Ok(Some(output)) => {
                let expected = to_string(node);
                if output != expected {
                    failures.push(format!(
                        "{rel}: {type_name} in {parent} differs:\n{expected}\n!=\n{output}"
                    ));
                }
            }
            Ok(None) => {}
            Err(e) => failures.push(format!(
                "{rel}: {type_name} in {parent}: {e}\n{}",
                to_string(node)
            )),
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
fn geometry_roundtrip_test_data() {
    let root = data_dir()
        .canonicalize()
        .expect("upstream test data not found (set LIBREPCB_UPSTREAM_DIR)");
    let mut files = Vec::new();
    collect_files(&root, &mut files);

    let mut counts = BTreeMap::new();
    let mut failures = Vec::new();
    for file in files.iter().filter(|f| is_current_format(f, &root)) {
        let rel = file.strip_prefix(&root).unwrap().display().to_string();
        let content = std::fs::read(file).unwrap();
        let tree = SExpression::parse(&content, Some(file), Mode::LibrePcb).unwrap();
        walk(&tree, "", &rel, &mut counts, &mut failures);
    }

    println!("checked geometry objects: {counts:?}");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    for type_name in [
        "vertex",
        "circle",
        "polygon",
        "stroke_text",
        "text",
        "pad hole",
        "hole",
        "pad",
        "junction",
        "net label",
        "net line",
        "via",
        "trace",
    ] {
        assert!(
            counts.get(type_name).is_some_and(|c| *c > 0),
            "no {type_name} found in test data"
        );
    }
}
