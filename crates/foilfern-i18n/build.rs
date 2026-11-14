//! Compiles the gettext catalogs in `lang/<lang>/LC_MESSAGES/librepcb.po`
//! into compact blobs embedded into the library (see `src/compile.rs`).

use std::fmt::Write as _;
use std::path::PathBuf;

#[allow(dead_code)]
#[path = "src/plural.rs"]
mod plural;

#[path = "src/compile.rs"]
mod compile;

/// Catalog file name (gettext domain) inside `LC_MESSAGES`.
const DOMAIN: &str = "librepcb";

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo"));
    let lang_dir = manifest_dir.join("../../lang");
    println!("cargo::rerun-if-changed={}", lang_dir.display());

    let mut languages: Vec<(String, PathBuf)> = Vec::new();
    match std::fs::read_dir(&lang_dir) {
        Ok(dir) => {
            for entry in dir.flatten() {
                let po = entry
                    .path()
                    .join("LC_MESSAGES")
                    .join(format!("{DOMAIN}.po"));
                if po.is_file() {
                    println!("cargo::rerun-if-changed={}", po.display());
                    languages.push((entry.file_name().to_string_lossy().into_owned(), po));
                }
            }
        }
        Err(e) => println!(
            "cargo::warning=no translations in {}: {e}",
            lang_dir.display()
        ),
    }
    languages.sort();

    let mut code = String::from("/// Embedded catalogs: (language tag, compiled catalog).\n");
    code.push_str("pub(crate) static CATALOG_DATA: &[(&str, &[u8])] = &[\n");
    for (tag, po) in &languages {
        let text = std::fs::read_to_string(po)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", po.display()));
        let compiled = compile::compile(&text).unwrap_or_else(|e| panic!("{}: {e}", po.display()));
        for w in compiled.warnings {
            println!("cargo::warning={}: {w}", po.display());
        }
        let file = out_dir.join(format!("{tag}.catalog"));
        std::fs::write(&file, compiled.blob).expect("write catalog blob");
        let _ = writeln!(
            code,
            "    ({tag:?}, include_bytes!({:?})),",
            file.display().to_string()
        );
    }
    code.push_str("];\n");
    std::fs::write(out_dir.join("catalogs.rs"), code).expect("write catalogs.rs");
}
