//! Exact supplier part acquisition, outside the session lock until installation.

use std::sync::Arc;

use foilfern_core::fileio::{
    FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem,
};
use foilfern_core::library::dev::Device;
use foilfern_core::library::pkg::Package;
use foilfern_core::library::{BaseMetadata, Library, LibraryBaseElement};
use foilfern_core::types::{ElementName, Uuid};
use foilfern_core::workspace::ElementKind;
use foilfern_import::easyeda::{self, NativePart};
use foilfern_network::part_source::{CachePolicy, EasyEdaProvider, PartSource};
use foilfern_network::{ClientInfo, NetworkAccessManager};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::library::{self, LibKind, LibraryElementArgs};
use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::Session;

/// Supplier snapshot selection.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PartCache {
    /// Reuse verified snapshots; fetch missing data.
    #[default]
    PreferCache,
    /// Use verified snapshots only, without HTTP.
    Offline,
    /// Fetch new snapshots, keeping old versions. Existing native elements are
    /// never overwritten; a changed source requires a separate review.
    Refresh,
}

impl From<PartCache> for CachePolicy {
    fn from(value: PartCache) -> Self {
        match value {
            PartCache::PreferCache => Self::PreferCache,
            PartCache::Offline => Self::Offline,
            PartCache::Refresh => Self::Refresh,
        }
    }
}

/// Arguments of `part_import`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PartImportArgs {
    /// Exact supplier ID, for example C8734. Imports symbol, pin mapping,
    /// footprint, MPN/manufacturer/LCSC and the referenced STEP when available.
    #[serde(default)]
    pub lcsc: Option<String>,
    /// Exact manufacturer part number, instead of lcsc. Zero, ambiguous or
    /// possibly truncated index results fail; the CAD source MPN is rechecked.
    #[serde(default)]
    pub mpn: Option<String>,
    /// Source/model cache policy (default prefer_cache).
    #[serde(default)]
    pub cache: PartCache,
    /// Refuse installation unless a referenced STEP model validates.
    #[serde(default)]
    pub require_model: bool,
}

/// Snapshot of the workspace paths before downloading.
#[derive(Debug, Clone)]
pub struct ImportTarget {
    data: FilePath,
    library: FilePath,
    cache: FilePath,
}

/// Converted acquisition, not yet installed.
#[derive(Debug)]
pub struct AcquiredPart {
    source: PartSource,
    native: NativePart,
    require_model: bool,
}

/// Captures paths without holding the session during HTTP or CAD conversion.
pub fn target(session: &Session) -> ToolResult<ImportTarget> {
    let ws = session.workspace()?;
    Ok(ImportTarget {
        data: ws.data_path().clone(),
        library: ws.local_libraries_path().path_to("SupplierParts.lplib"),
        cache: ws.data_path().path_to("cache/supplier-parts"),
    })
}

/// Official provider using the same client identity as library downloads.
pub fn provider() -> ToolResult<EasyEdaProvider> {
    let manager = NetworkAccessManager::new(ClientInfo {
        app_version: env!("CARGO_PKG_VERSION").into(),
        git_revision: String::new(),
        file_format_version: foilfern_core::application::file_format_version(),
        locale: "en_US".into(),
    })?;
    EasyEdaProvider::new(manager).map_err(source_error)
}

fn source_error(error: foilfern_network::part_source::Error) -> ToolError {
    use foilfern_network::part_source::Error;
    let kind = match &error {
        Error::Invalid(_) | Error::Json(_) => ErrorKind::InvalidArgument,
        Error::Offline(_) | Error::NoMatch(_) => ErrorKind::NotFound,
        Error::Ambiguous { .. } => ErrorKind::Conflict,
        Error::Cancelled => ErrorKind::Cancelled,
        Error::Io(_) | Error::File(_) => ErrorKind::Io,
        Error::Worker(_) => ErrorKind::Internal,
        Error::Http(_) => ErrorKind::Network,
    };
    ToolError::new(kind, error.to_string())
}

/// Downloads outside the session lock. The provider parameter permits local
/// HTTP tests without changing production endpoints or process environment.
pub async fn acquire(
    target: &ImportTarget,
    args: PartImportArgs,
    provider: EasyEdaProvider,
) -> ToolResult<AcquiredPart> {
    let source = match (&args.lcsc, &args.mpn) {
        (Some(lcsc), None) => {
            provider
                .acquire(
                    lcsc,
                    target.cache.as_path(),
                    args.cache.into(),
                    Default::default(),
                )
                .await
        }
        (None, Some(mpn)) => {
            provider
                .acquire_mpn(
                    mpn,
                    target.cache.as_path(),
                    args.cache.into(),
                    Default::default(),
                )
                .await
        }
        _ => return Err(ToolError::invalid("Pass exactly one of lcsc or mpn.")),
    }
    .map_err(source_error)?;
    let reference =
        easyeda::model_reference(&source.raw).map_err(|e| ToolError::invalid(e.to_string()))?;
    let mut warnings = Vec::new();
    let asset = if let Some(reference) = &reference {
        match provider
            .step_asset(
                &reference.provider_uuid,
                target.cache.as_path(),
                args.cache.into(),
                Default::default(),
            )
            .await
        {
            Ok(asset) => Some(asset),
            Err(error) if !args.require_model => {
                warnings.push(format!("STEP asset unavailable: {error}"));
                None
            }
            Err(error) => return Err(source_error(error)),
        }
    } else if args.require_model {
        return Err(ToolError::not_found(
            "The supplier source has no STEP model reference.",
        ));
    } else {
        None
    };
    tokio::task::spawn_blocking(move || {
        let mut native = easyeda::convert(&source.raw, &source.lcsc, &source.sha256)
            .map_err(|e| ToolError::invalid(e.to_string()))?;
        if let (Some(reference), Some(asset)) = (reference, asset) {
            let file = FilePath::new(&asset.file)
                .ok_or_else(|| ToolError::invalid("Invalid STEP cache path."))?;
            if let Err(error) = native.attach_step(&source.lcsc, &reference, &file, &asset.sha256) {
                if args.require_model {
                    return Err(ToolError::invalid(error.to_string()));
                }
                warnings.push(format!("STEP attachment failed: {error}"));
            }
        }
        native.warnings.extend(warnings);
        Ok(AcquiredPart {
            source,
            native,
            require_model: args.require_model,
        })
    })
    .await
    .map_err(|e| ToolError::internal(e.to_string()))?
}

/// Atomically installs a native library/device and returns ready-to-use details.
/// Replays retain manual edits; changed sources never overwrite native elements.
pub fn finish(
    session: &Session,
    target: ImportTarget,
    mut acquired: AcquiredPart,
) -> ToolResult<ToolOutput> {
    let ws = session.workspace()?;
    if ws.data_path() != &target.data {
        return Err(ToolError::new(
            ErrorKind::Conflict,
            "Workspace changed during acquisition.",
        ));
    }
    library::rescan(ws)?;
    let generated = format!("FoilfernEasyEDA::{}::device", acquired.source.lcsc);
    let existing = ws.library_db().generated(ElementKind::Device, &generated)?;
    if existing.len() > 1 {
        return Err(ToolError::new(
            ErrorKind::Conflict,
            "Multiple native devices have this supplier identity.",
        ));
    }
    let reused = !existing.is_empty();
    let uuid = if let Some(uuid) = existing.first() {
        let path = ws
            .library_db()
            .latest(ElementKind::Device, *uuid)?
            .ok_or_else(|| ToolError::not_found("Imported device disappeared."))?;
        let directory =
            TransactionalDirectory::new(Arc::new(TransactionalFileSystem::open_ro(&path)?), "");
        let device = Device::open(directory)?;
        let hash = device
            .parts()
            .iter()
            .flat_map(|p| p.attributes().iter())
            .find(|a| a.key().as_str() == "SOURCE_SHA256")
            .map(|a| a.value());
        if hash != Some(acquired.source.sha256.as_str()) {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                "Supplier source differs from the native device. Existing elements were preserved; review the snapshots before updating.",
            ));
        }
        *uuid
    } else {
        let create = if target.library.is_existing_dir() {
            let directory = TransactionalDirectory::new(
                Arc::new(TransactionalFileSystem::open_ro(&target.library)?),
                "",
            );
            Library::open(directory)?;
            None
        } else {
            Some(Library::new(BaseMetadata::new(
                Uuid::new_random(),
                "0.1"
                    .parse()
                    .map_err(|e| ToolError::internal(format!("Library version: {e}")))?,
                "Foilfern supplier import",
                acquired.native.device.metadata().created(),
                ElementName::new("Supplier Parts")?,
                "Imported supplier parts; original source attribution is retained per element.",
                "lcsc,easyeda",
            ))?)
        };
        acquired
            .native
            .install_with_library(&target.library, create)
            .map_err(|e| ToolError::invalid(e.to_string()))?;
        library::rescan(ws)?;
        acquired.native.device.metadata().uuid()
    };
    // Report the installed package, since a no-op replay may preserve manual
    // model edits rather than the newly converted temporary package.
    let dev_path = ws
        .library_db()
        .latest(ElementKind::Device, uuid)?
        .ok_or_else(|| ToolError::not_found("Installed device missing from index."))?;
    let dev = Device::open(TransactionalDirectory::new(
        Arc::new(TransactionalFileSystem::open_ro(&dev_path)?),
        "",
    ))?;
    let pkg_path = ws
        .library_db()
        .latest(ElementKind::Package, dev.package_uuid())?
        .ok_or_else(|| ToolError::not_found("Installed package missing from index."))?;
    let package = Package::open(TransactionalDirectory::new(
        Arc::new(TransactionalFileSystem::open_ro(&pkg_path)?),
        "",
    ))?;
    if reused {
        for model in acquired.native.package.models().iter() {
            let provenance = format!("easyeda-model-{}.json", model.uuid());
            if package.directory().file_exists(&provenance) {
                let old: serde_json::Value =
                    serde_json::from_slice(&package.directory().read(&provenance)?).map_err(
                        |e| ToolError::invalid(format!("Invalid installed model provenance: {e}")),
                    )?;
                let new: serde_json::Value =
                    serde_json::from_slice(&acquired.native.package.directory().read(&provenance)?)
                        .map_err(|e| ToolError::internal(e.to_string()))?;
                if old["sha256"] != new["sha256"] {
                    return Err(ToolError::new(
                        ErrorKind::Conflict,
                        "Supplier STEP asset changed. Existing model bytes were preserved; review the snapshots before updating.",
                    ));
                }
            }
        }
    }
    let model_available = package
        .models()
        .iter()
        .any(|m| pkg_path.path_to(&m.file_name()).is_existing_file());
    if acquired.require_model && !model_available {
        return Err(ToolError::new(
            ErrorKind::Conflict,
            "Existing native package has no model; replay preserved it. Review the package before adding a model.",
        ));
    }
    let detail = library::library_element(
        session,
        LibraryElementArgs {
            uuid: uuid.to_string(),
            kind: Some(LibKind::Device),
        },
    )?;
    let component = library::library_element(
        session,
        LibraryElementArgs {
            uuid: dev.component_uuid().to_string(),
            kind: Some(LibKind::Component),
        },
    )?;
    let part = dev.parts().iter().find(|p| {
        p.attributes()
            .iter()
            .any(|a| a.key().as_str() == "LCSC" && a.value() == acquired.source.lcsc)
    });
    let ready = part.map(|p| json!({"mpn":p.mpn().as_str(),"manufacturer":p.manufacturer().as_str(),"attributes":{"LCSC":acquired.source.lcsc}}));
    let mut output = ToolOutput::new(format!("{} {} ({uuid}).", if reused { "Reused" } else { "Imported" }, acquired.source.mpn), json!({
        "device":detail.result,"component":component.result,"reused":reused,
        "source":{"lcsc":acquired.source.lcsc,"mpn":acquired.source.mpn,"manufacturer":acquired.source.manufacturer,"url":acquired.source.source_url,"sha256":acquired.source.sha256,"snapshot":acquired.source.snapshot,"from_cache":acquired.source.from_cache,"attribution":acquired.source.attribution},
        "model":{"available":model_available,"alignment_verified":false},
        "add_component":{"device":uuid,"part":ready}
    }))?.warnings(acquired.native.warnings);
    if !model_available {
        output = output.partial();
    }
    Ok(output)
}
