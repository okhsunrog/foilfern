//! Resolve official KiCad symbols and their exact footprint/model dependencies
//! before entering the synchronous native importer. Source text is data only.

mod bundle;

use std::collections::BTreeSet;

use foilfern_core::fileio::FilePath;
use foilfern_core::serialization::{Mode, SExpression};
use foilfern_core::utils::message_logger::{LogLevel, MessageLogger};
use foilfern_import::kicad::types::{KiCadFootprint, KiCadSymbol};
use foilfern_network::kicad_source::{OfficialKiCad, Repository, SourceFile};
use foilfern_network::part_source::{CachePolicy, Error as SourceError};
use foilfern_network::{ClientInfo, NetworkAccessManager};
use serde_json::{Value, json};

use super::{ImportFormat, LibraryImportArgs};
use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::Session;

/// Workspace identity and cache locations captured before acquisition.
#[derive(Debug, Clone)]
pub struct OfficialTarget {
    data: FilePath,
    cache: FilePath,
    library: FilePath,
}

/// A prepared immutable source bundle, ready for native import or dry run.
#[derive(Debug)]
pub struct PreparedOfficial {
    args: LibraryImportArgs,
    bundle: FilePath,
    provenance: Value,
    warnings: Vec<String>,
}

pub(super) struct BundleInput {
    args: LibraryImportArgs,
    library: String,
    symbol: String,
    root: SExpression,
    files: Vec<(String, SourceFile)>,
    sources: Vec<SourceFile>,
    warnings: Vec<String>,
}

/// Captures the original workspace without retaining its session lock.
pub fn target(session: &Session) -> ToolResult<OfficialTarget> {
    let workspace = session.workspace()?;
    Ok(OfficialTarget {
        data: workspace.data_path().clone(),
        cache: workspace.data_path().path_to("cache/official-kicad"),
        library: workspace
            .local_libraries_path()
            .path_to("KiCadImports.lplib"),
    })
}

/// Official endpoints and ordinary Foilfern HTTP identity.
pub fn provider() -> ToolResult<OfficialKiCad> {
    let manager = NetworkAccessManager::new(ClientInfo {
        app_version: env!("CARGO_PKG_VERSION").into(),
        git_revision: String::new(),
        file_format_version: foilfern_core::application::file_format_version(),
        locale: "en_US".into(),
    })?;
    OfficialKiCad::new(manager).map_err(source_error)
}

fn source_error(error: SourceError) -> ToolError {
    let kind = match &error {
        SourceError::Http(error) if error.status().is_some_and(|status| status.as_u16() == 404) => {
            ErrorKind::NotFound
        }
        SourceError::Http(_) => ErrorKind::Network,
        SourceError::Io(_) | SourceError::File(_) => ErrorKind::Io,
        SourceError::Offline(_) | SourceError::NoMatch(_) => ErrorKind::NotFound,
        SourceError::Cancelled => ErrorKind::Cancelled,
        SourceError::Worker(_) => ErrorKind::Internal,
        SourceError::Ambiguous { .. } => ErrorKind::Conflict,
        SourceError::Invalid(_) | SourceError::Json(_) => ErrorKind::InvalidArgument,
    };
    ToolError::new(kind, error.to_string())
}

fn missing(error: &SourceError) -> bool {
    matches!(error,SourceError::Http(error) if error.status().is_some_and(|status|status.as_u16()==404))
}

fn component(value: &str) -> ToolResult<()> {
    if value.is_empty()
        || value.len() > 200
        || value.chars().any(char::is_control)
        || value.contains(['/', '\\', ':'])
        || matches!(value, "." | "..")
    {
        return Err(ToolError::invalid(
            "Official library/symbol names must be exact path components.",
        ));
    }
    Ok(())
}

fn identifier(value: &str) -> ToolResult<(String, String)> {
    let (library, name) = value
        .split_once(':')
        .ok_or_else(|| ToolError::invalid("Pass an exact Library:Name identifier."))?;
    component(library)?;
    component(name)?;
    Ok((library.into(), name.into()))
}

fn symbol_node(root: &SExpression, name: &str) -> ToolResult<SExpression> {
    if root
        .as_list()
        .is_none_or(|list| list.name() != "kicad_symbol_lib")
    {
        return Err(ToolError::invalid(
            "Official source is not a KiCad symbol library.",
        ));
    }
    let matches = root
        .children_named("symbol")
        .filter(|node| {
            node.child_value::<String>("@0")
                .is_ok_and(|value| value == name)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [node] => Ok((*node).clone()),
        [] => Err(ToolError::not_found(format!(
            "Official symbol {name:?} is absent from the pinned source."
        ))),
        _ => Err(ToolError::new(
            ErrorKind::Conflict,
            "Duplicate exact symbol names in official source.",
        )),
    }
}

async fn parsed_symbol(
    file: SourceFile,
    name: String,
) -> ToolResult<(SExpression, SExpression, KiCadSymbol)> {
    tokio::task::spawn_blocking(move || {
        let bytes = bundle::read(&file)?;
        let root = SExpression::parse(&bytes, Some(&file.file), Mode::Permissive)
            .map_err(|e| ToolError::invalid(e.to_string()))?;
        let node = symbol_node(&root, &name)?;
        let log = MessageLogger::new();
        let symbol = KiCadSymbol::parse(&node, &log)?;
        if let Some(error) = log
            .messages()
            .into_iter()
            .find(|message| message.level == LogLevel::Critical)
        {
            return Err(ToolError::invalid(error.message));
        }
        Ok((root, node, symbol))
    })
    .await
    .map_err(|e| ToolError::internal(e.to_string()))?
}

/// Acquires an exact symbol, parent chain, footprint, available STEP and license
/// documents. All parsing/CAD work runs on workers outside the session lock.
pub async fn prepare(
    target: &OfficialTarget,
    args: LibraryImportArgs,
    provider: OfficialKiCad,
) -> ToolResult<PreparedOfficial> {
    if args.format != ImportFormat::Kicad {
        return Err(ToolError::invalid("Official sources require format=kicad."));
    }
    let (library, symbol) = identifier(
        args.source
            .strip_prefix("official:")
            .ok_or_else(|| ToolError::invalid("Expected official:Library:Symbol source."))?,
    )?;
    if let Some(footprint) = &args.official_footprint {
        identifier(footprint)?;
    }
    if args.elements.as_ref().is_some_and(|elements| {
        elements
            .iter()
            .any(|name| name != &symbol && name != &format!("{library}:{symbol}"))
    }) {
        return Err(ToolError::invalid(
            "An official source selects one exact symbol; elements must name that symbol or be omitted.",
        ));
    }
    let release = args.official_release.as_deref().unwrap_or("10.0.8");
    let policy: CachePolicy = args.source_cache.unwrap_or_default().into();
    let revision = provider
        .resolve(
            Repository::Symbols,
            release,
            target.cache.as_path(),
            policy,
            Default::default(),
        )
        .await
        .map_err(source_error)?;
    let license = provider
        .file(
            &revision,
            "LICENSE.md",
            target.cache.as_path(),
            policy,
            Default::default(),
        )
        .await
        .map_err(source_error)?;
    let mut files = vec![("LICENSE-kicad-symbols.md".into(), license.clone())];
    let mut sources = vec![license];
    let mut inherited = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = symbol.clone();
    let mut merged = None;
    let mut aggregate: Option<SourceFile> = None;
    let mut footprint = args.official_footprint.clone();
    loop {
        if seen.len() >= 32 || !seen.insert(current.clone()) {
            return Err(ToolError::invalid(
                "Cyclic or excessive KiCad symbol inheritance.",
            ));
        }
        component(&current)?;
        let source = if let Some(file) = &aggregate {
            file.clone()
        } else {
            match provider
                .file(
                    &revision,
                    &format!("{library}.kicad_symdir/{current}.kicad_sym"),
                    target.cache.as_path(),
                    policy,
                    Default::default(),
                )
                .await
            {
                Ok(file) => file,
                Err(error) if missing(&error) => {
                    let file = provider
                        .file(
                            &revision,
                            &format!("{library}.kicad_sym"),
                            target.cache.as_path(),
                            policy,
                            Default::default(),
                        )
                        .await
                        .map_err(source_error)?;
                    aggregate = Some(file.clone());
                    file
                }
                Err(error) => return Err(source_error(error)),
            }
        };
        if !sources
            .iter()
            .any(|old| old.source_url == source.source_url)
        {
            sources.push(source.clone());
        }
        let (root, node, parsed) = parsed_symbol(source, current.clone()).await?;
        if merged.is_none() {
            merged = Some(root);
        }
        if footprint.is_none() {
            footprint = parsed
                .properties
                .iter()
                .find(|property| property.key.eq_ignore_ascii_case("footprint"))
                .map(|property| property.value.clone());
        }
        inherited.push(node);
        if parsed.extends.is_empty() {
            break;
        }
        current = parsed.extends;
    }
    if inherited.len() > 2 {
        return Err(ToolError::invalid(
            "The native KiCad importer supports one inheritance level; this source requires a deeper chain.",
        ));
    }
    let mut root = merged.ok_or_else(|| ToolError::internal("Missing parsed symbol root."))?;
    let root_list = root
        .as_list_mut()
        .ok_or_else(|| ToolError::internal("Missing symbol library list."))?;
    root_list
        .children_mut()
        .retain(|node| node.as_list().is_none_or(|list| list.name() != "symbol"));
    // The native import processes components before dependent devices in file
    // order, so a fetched parent must precede its derived symbol.
    root_list.children_mut().extend(inherited.into_iter().rev());
    // The native importer resolves device selection before inherited symbol
    // properties. Materialize the effective property for that selection step.
    if let Some(name) = footprint.as_ref().filter(|name| !name.is_empty()) {
        bundle::override_footprint(&mut root, &symbol, name)?;
    }
    let mut warnings = Vec::new();
    if let Some(footprint) = footprint.filter(|name| !name.is_empty()) {
        let (fp_library, fp_name) = identifier(&footprint)?;
        let fp_revision = provider
            .resolve(
                Repository::Footprints,
                release,
                target.cache.as_path(),
                policy,
                Default::default(),
            )
            .await
            .map_err(source_error)?;
        let license = provider
            .file(
                &fp_revision,
                "LICENSE.md",
                target.cache.as_path(),
                policy,
                Default::default(),
            )
            .await
            .map_err(source_error)?;
        files.push(("LICENSE-kicad-footprints.md".into(), license.clone()));
        sources.push(license);
        let fp_path = format!("{fp_library}.pretty/{fp_name}.kicad_mod");
        let file = provider
            .file(
                &fp_revision,
                &fp_path,
                target.cache.as_path(),
                policy,
                Default::default(),
            )
            .await
            .map_err(source_error)?;
        let parse_file = file.clone();
        let model_paths = tokio::task::spawn_blocking(move || {
            let bytes = bundle::read(&parse_file)?;
            let log = MessageLogger::new();
            let footprint = KiCadFootprint::parse_file(&bytes, &parse_file.file, &log)?;
            Ok::<_, ToolError>(
                footprint
                    .models
                    .into_iter()
                    .map(|model| model.path)
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .map_err(|e| ToolError::internal(e.to_string()))??;
        files.push((fp_path, file.clone()));
        sources.push(file);
        if args.shapes_3d.is_none() && !model_paths.is_empty() {
            match acquire_models(&provider, release, target, policy, model_paths).await {
                Ok((models, model_warnings)) => {
                    for (path, file) in models {
                        sources.push(file.clone());
                        files.push((path, file));
                    }
                    warnings.extend(model_warnings);
                }
                Err(error) => warnings.push(format!("Official STEP assets unavailable: {error}")),
            }
        }
    } else {
        warnings.push("Official symbol has no default footprint; pass official_footprint to select an exact package.".into());
    }
    let target = target.clone();
    tokio::task::spawn_blocking(move || {
        bundle::assemble(
            &target,
            BundleInput {
                args,
                library,
                symbol,
                root,
                files,
                sources,
                warnings,
            },
        )
    })
    .await
    .map_err(|e| ToolError::internal(e.to_string()))?
}

async fn acquire_models(
    provider: &OfficialKiCad,
    release: &str,
    target: &OfficialTarget,
    policy: CachePolicy,
    paths: Vec<String>,
) -> ToolResult<(Vec<(String, SourceFile)>, Vec<String>)> {
    if paths.len() > 16 {
        return Err(ToolError::invalid(
            "Footprint has too many model references.",
        ));
    }
    let revision = provider
        .resolve(
            Repository::Models,
            release,
            target.cache.as_path(),
            policy,
            Default::default(),
        )
        .await
        .map_err(source_error)?;
    let license = provider
        .file(
            &revision,
            "LICENSE.md",
            target.cache.as_path(),
            policy,
            Default::default(),
        )
        .await
        .map_err(source_error)?;
    let mut files = vec![("LICENSE-kicad-packages3D.md".into(), license)];
    let mut warnings = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        let parts = path.split('/').collect::<Vec<_>>();
        let Some([directory, name]) = parts.get(parts.len().saturating_sub(2)..) else {
            warnings.push(format!("Unsupported official model path {path:?}."));
            continue;
        };
        component(directory)?;
        component(name)?;
        if !directory.ends_with(".3dshapes") || !(name.ends_with(".step") || name.ends_with(".wrl"))
        {
            warnings.push(format!("Unsupported official model path {path:?}."));
            continue;
        }
        let path = format!(
            "{directory}/{}",
            name.strip_suffix(".wrl")
                .map_or_else(|| (*name).to_owned(), |stem| format!("{stem}.step"))
        );
        if !seen.insert(path.clone()) {
            continue;
        }
        match provider
            .file(
                &revision,
                &path,
                target.cache.as_path(),
                policy,
                Default::default(),
            )
            .await
        {
            Ok(file) => {
                let validate = file.clone();
                match tokio::task::spawn_blocking(move || {
                    let model = foilfern_step::StepModel::from_step(&bundle::read(&validate)?)
                        .map_err(|e| ToolError::invalid(e.to_string()))?;
                    if model.solid_count() == 0 {
                        return Err(ToolError::invalid("Official STEP has no solids."));
                    }
                    Ok::<_, ToolError>(())
                })
                .await
                .map_err(|e| ToolError::internal(e.to_string()))?
                {
                    Ok(()) => files.push((path, file)),
                    Err(error) => {
                        warnings.push(format!("Official model {path:?} rejected: {error}"))
                    }
                }
            }
            Err(error) => warnings.push(format!("Official model {path:?} unavailable: {error}")),
        }
    }
    Ok((files, warnings))
}

/// Rechecks the workspace, prepares a local library when needed, and invokes
/// the same native import path as ordinary KiCad files. Dry runs create no library.
pub fn finish(
    session: &Session,
    target: OfficialTarget,
    mut prepared: PreparedOfficial,
) -> ToolResult<ToolOutput> {
    if session.workspace()?.data_path() != &target.data {
        return Err(ToolError::new(
            ErrorKind::Conflict,
            "Workspace changed during official source acquisition.",
        ));
    }
    if !prepared.args.dry_run {
        bundle::prepare_library(session, &target, &mut prepared)?;
    }
    let dry_run = prepared.args.dry_run;
    let selected = prepared.provenance["symbol"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let mut output = super::library_import(session, prepared.args)?;
    if !dry_run {
        match bundle::devices(session, &selected) {
            Ok(devices) => {
                if let [device] = devices.as_slice() {
                    output.result["add_component"] = json!({"device":device["uuid"]});
                }
                output.result["devices"] = json!(devices);
            }
            Err(error) => {
                output = output
                    .partial()
                    .warn(format!("Could not read installed device details: {error}"))
            }
        }
    }
    output.result["official_source"] = prepared.provenance;
    if !prepared.warnings.is_empty() {
        output = output.partial().warnings(prepared.warnings);
    }
    Ok(output)
}
