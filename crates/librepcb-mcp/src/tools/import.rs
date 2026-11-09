//! Import tool: `library_import` (EAGLE `*.lbr` and KiCad libraries into a
//! LibrePCB library, like upstream's library import wizards). The EAGLE
//! project import is part of `project_create` (see
//! [`project`](crate::tools::project)).
//!
//! The conversion itself lives in `librepcb-import`; this module only maps
//! the wizard steps (source, element selection, options, run) onto one
//! tool call, with `dry_run` listing the elements instead of importing.

use librepcb_core::fileio::FilePath;
use librepcb_core::utils::message_logger::{LogLevel, MessageLogger};
use librepcb_core::workspace::{ElementKind as DbKind, Workspace};
use librepcb_import::eagle::EagleLibraryImport;
use librepcb_import::kicad::{
    ImportState, ImportedElementLookup, KiCadLibraryImport, NoImportedElements,
};
use librepcb_import::{ImportSummary, Progress};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::resolve::{parse_uuid, required_uuid};
use crate::session::{Session, absolute_path};
use crate::tools::library;

/// At most this many elements are listed by a dry run (the count is always
/// returned).
const MAX_LISTED: usize = 500;

/// At most this many log messages are returned.
const MAX_MESSAGES: usize = 200;

/// Format of the libraries to import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    /// An EAGLE library (`*.lbr`).
    Eagle,
    /// KiCad libraries (`*.kicad_sym`, `*.pretty`, `*.kicad_mod`).
    Kicad,
}

/// Arguments of `library_import`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryImportArgs {
    /// Source format.
    pub format: ImportFormat,
    /// EAGLE: the `*.lbr` file. KiCad: a `*.kicad_sym` file, a `*.pretty`
    /// directory, a `*.kicad_mod` file, or a directory which is scanned
    /// together with its direct subdirectories (e.g. the KiCad library
    /// installation directory).
    pub source: String,
    /// KiCad only: directory with the `*.3dshapes` directories (default:
    /// `source`).
    #[serde(default)]
    pub shapes_3d: Option<String>,
    /// Target library: name or UUID of a local library of the workspace, or
    /// the path of a library directory (`*.lplib`). Not needed for a dry
    /// run.
    #[serde(default)]
    pub library: Option<String>,
    /// Only list the elements (with their selection state), import
    /// nothing.
    #[serde(default)]
    pub dry_run: bool,
    /// Elements to import (their dependencies are imported too); default:
    /// all. EAGLE: device, device set (component), package or symbol names.
    /// KiCad: symbol or footprint names, optionally prefixed with the
    /// library name ("Device:R").
    #[serde(default)]
    pub elements: Option<Vec<String>>,
    /// Prefix the names of the imported elements with "EAGLE_" or "KICAD_"
    /// (default false, like upstream's wizards).
    #[serde(default)]
    pub add_name_prefix: Option<bool>,
    /// Add the elements to the categories "EAGLE Import" / "KiCad Import"
    /// (created in the target library if needed; default true, like
    /// upstream's wizards).
    #[serde(default)]
    pub add_to_category: Option<bool>,
    /// UUID of an additional component category for the imported symbols,
    /// components and devices.
    #[serde(default)]
    pub component_category: Option<String>,
    /// UUID of an additional package category for the imported packages.
    #[serde(default)]
    pub package_category: Option<String>,
}

/// `library_import`.
pub fn library_import(session: &Session, args: LibraryImportArgs) -> ToolResult<ToolOutput> {
    let source = absolute_path(&args.source)?;
    if !source.is_existing_file() && !source.is_existing_dir() {
        return Err(ToolError::not_found(format!(
            "\"{}\" does not exist.",
            source.to_native()
        )));
    }
    let workspace = session.workspace.as_deref();
    let destination = match (&args.library, args.dry_run) {
        (Some(lib), _) => target_library(workspace, lib)?,
        // Never written by a dry run.
        (None, true) => source.path_to("__dry_run__"),
        (None, false) => {
            return Err(ToolError::invalid(
                "Pass the target library (name, UUID or directory) or dry_run=true.",
            ));
        }
    };
    let component_category = args
        .component_category
        .as_deref()
        .map(|s| required_uuid(s, "component category"))
        .transpose()?;
    let package_category = args
        .package_category
        .as_deref()
        .map(|s| required_uuid(s, "package category"))
        .transpose()?;
    let log = MessageLogger::new();
    let progress = Progress::new();
    let (elements, summary) = match args.format {
        ImportFormat::Eagle => {
            let mut import = EagleLibraryImport::new(destination.clone());
            let parse_errors = import.open(&source)?;
            for e in parse_errors {
                log.warning(&e);
            }
            import.set_add_name_prefix(args.add_name_prefix.unwrap_or(false));
            import.set_add_to_category(args.add_to_category.unwrap_or(true));
            import.set_component_category(component_category);
            import.set_package_category(package_category);
            select_eagle(&mut import, args.elements.as_deref())?;
            let elements = eagle_elements(&import);
            let summary = (!args.dry_run).then(|| import.run(&log, &progress));
            (elements, summary)
        }
        ImportFormat::Kicad => {
            let lookup: &dyn ImportedElementLookup = match workspace {
                Some(ws) => ws.library_db(),
                None => &NoImportedElements,
            };
            let shapes_3d = args.shapes_3d.as_deref().map(absolute_path).transpose()?;
            let mut import = KiCadLibraryImport::new(destination.clone());
            import.set_add_name_prefix(args.add_name_prefix.unwrap_or(false));
            import.set_add_to_category(args.add_to_category.unwrap_or(true));
            import.set_component_category(component_category);
            import.set_package_category(package_category);
            if !import.scan(&source, shapes_3d.as_ref(), &log, &progress)
                || !import.parse(lookup, &log, &progress)
            {
                return Err(ToolError::invalid(format!(
                    "Failed to read the KiCad libraries: {}",
                    errors(&log).join("; ")
                )));
            }
            select_kicad(&mut import, args.elements.as_deref())?;
            let elements = kicad_elements(&import);
            let summary = if args.dry_run {
                None
            } else {
                let summary = import.import(lookup, &log, &progress);
                if import.state() != ImportState::Imported {
                    return Err(ToolError::internal(format!(
                        "The import failed: {}",
                        errors(&log).join("; ")
                    )));
                }
                summary
            };
            (elements, summary)
        }
    };
    output(workspace, &destination, &log, elements, summary)
}

/// Resolves the target library (name/UUID of a local workspace library or
/// a library directory).
fn target_library(workspace: Option<&Workspace>, key: &str) -> ToolResult<FilePath> {
    let is_library = |dir: &FilePath| dir.path_to("library.lp").is_existing_file();
    if let Some(ws) = workspace {
        let db = ws.library_db();
        let local = ws.local_libraries_path();
        let uuid = parse_uuid(key.trim());
        let locales = ws.settings().library_locale_order.get().clone();
        for (_, dir) in db.all(DbKind::Library, None, None)? {
            if !dir.is_located_in_dir(&local) {
                continue;
            }
            let Some(info) = db.metadata(DbKind::Library, &dir)? else {
                continue;
            };
            let name = db
                .translations(DbKind::Library, &dir, &locales)?
                .map(|t| t.name)
                .unwrap_or_default();
            if Some(info.uuid) == uuid || name == key.trim() {
                return Ok(dir);
            }
        }
    }
    let looks_like_path = key.contains('/') || key.contains('\\') || key.ends_with(".lplib");
    if looks_like_path {
        let dir = absolute_path(key)?;
        if is_library(&dir) {
            return Ok(dir);
        }
        return Err(ToolError::not_found(format!(
            "\"{}\" is not a LibrePCB library directory.",
            dir.to_native()
        )));
    }
    Err(ToolError::not_found(format!(
        "No local library \"{key}\" in the workspace (pass the name or UUID of a library in \
         <workspace>/data/libraries/local, or a library directory)."
    )))
}

fn not_found(names: &[String]) -> ToolError {
    ToolError::not_found(format!(
        "Elements not found in the source library: {}",
        names.join(", ")
    ))
}

/// Selects the given elements (all if `None`).
fn select_eagle(import: &mut EagleLibraryImport, elements: Option<&[String]>) -> ToolResult<()> {
    let Some(elements) = elements else {
        import.set_all_checked(true);
        return Ok(());
    };
    import.set_all_checked(false);
    let mut missing = Vec::new();
    for name in elements {
        if import.devices().iter().any(|d| &d.display_name == name) {
            import.set_device_checked(name, true);
        } else if import.components().iter().any(|c| &c.display_name == name) {
            import.set_component_checked(name, true);
        } else if import.packages().iter().any(|p| &p.display_name == name) {
            import.set_package_checked(name, true);
        } else if import.symbols().iter().any(|s| &s.display_name == name) {
            import.set_symbol_checked(name, true);
        } else {
            missing.push(name.clone());
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(not_found(&missing))
    }
}

/// Selects the given elements (all if `None`).
fn select_kicad(import: &mut KiCadLibraryImport, elements: Option<&[String]>) -> ToolResult<()> {
    let Some(elements) = elements else {
        import.set_all_checked(true);
        return Ok(());
    };
    import.set_all_checked(false);
    let mut missing = Vec::new();
    for key in elements {
        let (lib, name) = match key.split_once(':') {
            Some((lib, name)) => (Some(lib), name),
            None => (None, key.as_str()),
        };
        let matches_lib = |l: &str| lib.is_none_or(|lib| lib == l);
        // Symbols: the device if there is a footprint, else the component.
        let symbols: Vec<(String, bool)> = import
            .result()
            .symbol_libs
            .iter()
            .filter(|l| matches_lib(l.file.complete_basename()))
            .flat_map(|l| {
                l.symbols.iter().filter(|s| s.name == name).map(|s| {
                    (
                        l.file.complete_basename().to_owned(),
                        !s.pkg_generated_by.is_empty(),
                    )
                })
            })
            .collect();
        let footprints: Vec<String> = import
            .result()
            .footprint_libs
            .iter()
            .filter(|l| matches_lib(l.dir.complete_basename()))
            .filter(|l| l.footprints.iter().any(|f| f.name == name))
            .map(|l| l.dir.complete_basename().to_owned())
            .collect();
        if symbols.is_empty() && footprints.is_empty() {
            missing.push(key.clone());
        }
        for (lib, has_footprint) in symbols {
            if has_footprint {
                import.set_device_checked(&lib, name, true);
            } else {
                import.set_component_checked(&lib, name, true);
            }
        }
        for lib in footprints {
            import.set_package_checked(&lib, name, true);
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(not_found(&missing))
    }
}

fn eagle_elements(import: &EagleLibraryImport) -> Vec<Value> {
    let item = |kind: &str, name: &str, checked: bool| json!({ "kind": kind, "name": name, "selected": checked });
    let mut out = Vec::new();
    out.extend(
        import
            .devices()
            .iter()
            .map(|d| item("device", &d.display_name, d.check_state.is_imported())),
    );
    out.extend(
        import
            .components()
            .iter()
            .map(|c| item("component", &c.display_name, c.check_state.is_imported())),
    );
    out.extend(
        import
            .packages()
            .iter()
            .map(|p| item("package", &p.display_name, p.check_state.is_imported())),
    );
    out.extend(
        import
            .symbols()
            .iter()
            .map(|s| item("symbol", &s.display_name, s.check_state.is_imported())),
    );
    out
}

fn kicad_elements(import: &KiCadLibraryImport) -> Vec<Value> {
    let mut out = Vec::new();
    for lib in &import.result().symbol_libs {
        let lib_name = lib.file.complete_basename();
        for sym in &lib.symbols {
            out.push(json!({
                "kind": "symbol",
                "library": lib_name,
                "name": sym.name,
                "gates": sym.gates.len(),
                "has_footprint": !sym.pkg_generated_by.is_empty(),
                "extends": (!sym.extends.is_empty()).then_some(&sym.extends),
                "selected": sym.dev_checked.is_imported()
                    || sym.cmp_checked.is_imported()
                    || sym.sym_checked.is_imported(),
                "already_imported": sym.dev_already_imported || sym.cmp_already_imported,
            }));
        }
    }
    for lib in &import.result().footprint_libs {
        let lib_name = lib.dir.complete_basename();
        for fpt in &lib.footprints {
            out.push(json!({
                "kind": "footprint",
                "library": lib_name,
                "name": fpt.name,
                "selected": fpt.checked.is_imported(),
                "already_imported": fpt.already_imported,
            }));
        }
    }
    out
}

/// Error messages of the log.
fn errors(log: &MessageLogger<'_>) -> Vec<String> {
    log.messages()
        .into_iter()
        .filter(|m| m.level == LogLevel::Critical)
        .map(|m| m.message)
        .collect()
}

fn output(
    workspace: Option<&Workspace>,
    destination: &FilePath,
    log: &MessageLogger<'_>,
    mut elements: Vec<Value>,
    summary: Option<ImportSummary>,
) -> ToolResult<ToolOutput> {
    let messages = log.messages();
    let warnings: Vec<String> = messages
        .iter()
        .filter(|m| m.level >= LogLevel::Warning)
        .map(|m| format!("{}: {}", m.level, m.message))
        .take(MAX_MESSAGES)
        .collect();
    let element_count = elements.len();
    let selected = elements.iter().filter(|e| e["selected"] == true).count();
    let Some(summary) = summary else {
        elements.truncate(MAX_LISTED);
        return Ok(ToolOutput::new(
            format!("{element_count} elements found, {selected} would be imported."),
            json!({
                "element_count": element_count,
                "selected_count": selected,
                "elements": elements,
            }),
        )?
        .warnings(warnings));
    };
    // Make the new elements available (library_search, component_add).
    let element_count_in_index = match workspace {
        Some(ws) if destination.is_located_in_dir(ws.libraries_path()) => {
            Some(library::rescan(ws)?)
        }
        _ => None,
    };
    let failed = summary.processed - summary.imported;
    let out = ToolOutput::new(
        format!(
            "Imported {} of {} elements into {}{}.",
            summary.imported,
            summary.total,
            destination.to_native(),
            if failed > 0 {
                format!(" ({failed} failed, see warnings)")
            } else {
                String::new()
            }
        ),
        json!({
            "library": destination.to_native(),
            "total": summary.total,
            "imported": summary.imported,
            "failed": failed,
            "index_element_count": element_count_in_index,
            "log": messages
                .iter()
                .take(MAX_MESSAGES)
                .map(|m| format!("{}: {}", m.level, m.message))
                .collect::<Vec<_>>(),
        }),
    )?
    .warnings(warnings);
    Ok(if summary.imported < summary.total {
        out.partial()
    } else {
        out
    })
}
