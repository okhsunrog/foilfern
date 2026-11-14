//! Verified assembly and native collection provenance for official KiCad input.

use std::collections::BTreeMap;
use std::sync::Arc;

use foilfern_core::fileio::{FileSystem, TransactionalDirectory, TransactionalFileSystem};
use foilfern_core::library::{BaseMetadata, Library, LibraryBaseElement};
use foilfern_core::serialization::{List, Mode, SExpression};
use foilfern_core::types::{ElementName, Uuid};
use foilfern_network::kicad_source::{Repository, SourceFile};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{BundleInput, OfficialTarget, PreparedOfficial};
use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::session::Session;

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn io(error: std::io::Error) -> ToolError {
    ToolError::new(ErrorKind::Io, error.to_string())
}

pub(super) fn read(file: &SourceFile) -> ToolResult<Vec<u8>> {
    let limit = if matches!(file.revision.repository, Repository::Models) {
        128 * 1024 * 1024
    } else {
        32 * 1024 * 1024
    };
    if std::fs::metadata(&file.file).map_err(io)?.len() > limit {
        return Err(ToolError::invalid(
            "Official source snapshot exceeds its size limit.",
        ));
    }
    let bytes = std::fs::read(&file.file).map_err(io)?;
    if digest(&bytes) != file.sha256 {
        return Err(ToolError::invalid(
            "Official source snapshot checksum changed during acquisition.",
        ));
    }
    Ok(bytes)
}

pub(super) fn override_footprint(
    root: &mut SExpression,
    symbol: &str,
    name: &str,
) -> ToolResult<()> {
    super::identifier(name)?;
    let root = root
        .as_list_mut()
        .ok_or_else(|| ToolError::invalid("Missing symbol library root."))?;
    let node = root
        .children_mut()
        .iter_mut()
        .find(|node| {
            node.as_list().is_some_and(|list| list.name() == "symbol")
                && node
                    .child_value::<String>("@0")
                    .is_ok_and(|name| name == symbol)
        })
        .ok_or_else(|| {
            ToolError::not_found("Requested symbol disappeared during bundle assembly.")
        })?;
    let list = node
        .as_list_mut()
        .ok_or_else(|| ToolError::invalid("Invalid symbol node."))?;
    if let Some(property) = list.children_mut().iter_mut().find(|node| {
        node.as_list().is_some_and(|list| list.name() == "property")
            && node
                .child_value::<String>("@0")
                .is_ok_and(|key| key.eq_ignore_ascii_case("footprint"))
    }) {
        property
            .child_mut("@1")
            .ok_or_else(|| ToolError::invalid("Footprint property has no value."))?
            .set_value(name)
            .map_err(|e| ToolError::invalid(e.to_string()))?;
    } else {
        let mut property = List::new("property");
        property.append_value("Footprint");
        property.append_value(name);
        list.push(SExpression::List(property));
    }
    Ok(())
}

pub(super) fn assemble(
    target: &OfficialTarget,
    mut input: BundleInput,
) -> ToolResult<PreparedOfficial> {
    let mut contents = BTreeMap::new();
    contents.insert(
        format!("{}.kicad_sym", input.library),
        input
            .root
            .to_string_with_mode(Mode::Permissive)
            .map_err(|e| ToolError::internal(e.to_string()))?
            .into_bytes(),
    );
    for (path, file) in &input.files {
        contents.insert(path.clone(), read(file)?);
    }
    let sources=input.sources.iter().map(|file|json!({"repository":file.revision.repository.name(),"reference":file.revision.reference,"commit":file.revision.commit,"path":file.path,"url":file.source_url,"sha256":file.sha256})).collect::<Vec<_>>();
    let assembled = contents
        .iter()
        .map(|(path, bytes)| json!({"path":path,"sha256":digest(bytes)}))
        .collect::<Vec<_>>();
    let mut provenance = json!({"schema":1,"provider":"KiCad","symbol":format!("{}:{}",input.library,input.symbol),"footprint_override":input.args.official_footprint,"license_url":"https://www.kicad.org/libraries/license/","sources":sources,"assembled":assembled,"model_alignment_verified":false});
    let id =
        digest(&serde_json::to_vec(&provenance).map_err(|e| ToolError::internal(e.to_string()))?);
    provenance["bundle_id"] = json!(id);
    let bundle = target.cache.path_to(&format!("bundles-v1/{id}"));
    let fs = Arc::new(TransactionalFileSystem::open_rw(&bundle)?);
    let mut directory = TransactionalDirectory::new(Arc::clone(&fs), "");
    for (path, bytes) in contents {
        directory.write(&path, &bytes)?;
    }
    directory.write(
        "kicad-source.json",
        &serde_json::to_vec(&provenance).map_err(|e| ToolError::internal(e.to_string()))?,
    )?;
    fs.save()?;
    input.args.source = bundle.to_native();
    if input.args.shapes_3d.is_none() {
        input.args.shapes_3d = Some(bundle.to_native());
    }
    if input.args.elements.is_none() {
        input.args.elements = Some(vec![format!("{}:{}", input.library, input.symbol)]);
    }
    input.args.official_release = None;
    input.args.official_footprint = None;
    input.args.source_cache = None;
    Ok(PreparedOfficial {
        args: input.args,
        bundle,
        provenance,
        warnings: input.warnings,
    })
}

pub(super) fn prepare_library(
    session: &Session,
    target: &OfficialTarget,
    prepared: &mut PreparedOfficial,
) -> ToolResult<()> {
    let path = match prepared.args.library.as_deref() {
        Some(key) => super::super::target_library(Some(session.workspace()?), key)?,
        None => target.library.clone(),
    };
    let fs = Arc::new(TransactionalFileSystem::open_rw(&path)?);
    let mut directory = TransactionalDirectory::new(Arc::clone(&fs), "");
    if directory.file_exists("library.lp") {
        Library::open(TransactionalDirectory::new(Arc::clone(&fs), ""))?;
    } else {
        if directory.files("").iter().any(|name| name != ".lock") || !directory.dirs("").is_empty()
        {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                "Refusing to replace a nonempty library directory.",
            ));
        }
        let mut library = Library::new(BaseMetadata::new(
            Uuid::new_random(),
            "0.1"
                .parse()
                .map_err(|e| ToolError::internal(format!("Library version: {e}")))?,
            "KiCad contributors",
            std::time::SystemTime::now().into(),
            ElementName::new("KiCad Imports")?,
            "Imported official KiCad library data; source revisions and license documents are retained in sources/.",
            "kicad,import",
        ))?;
        library.save_to(&mut directory)?;
    }
    let bundle = TransactionalDirectory::new(
        Arc::new(TransactionalFileSystem::open_ro(&prepared.bundle)?),
        "",
    );
    let archive = format!("sources/{}", prepared.bundle.file_name());
    for name in bundle
        .files("")
        .into_iter()
        .filter(|name| name == "kicad-source.json" || name.starts_with("LICENSE-"))
    {
        directory.write(&format!("{archive}/{name}"), &bundle.read(&name)?)?;
    }
    fs.save()?;
    prepared.args.library = Some(path.to_native());
    Ok(())
}

pub(super) fn devices(session: &Session, selected: &str) -> ToolResult<Vec<serde_json::Value>> {
    use foilfern_core::library::dev::Device;
    use foilfern_core::workspace::ElementKind;
    let (library, name) = super::identifier(selected)?;
    let workspace = session.workspace()?;
    let key = format!("KiCadImport::{library}::{name}");
    let mut details = Vec::new();
    for uuid in workspace
        .library_db()
        .generated(ElementKind::Device, &key)?
    {
        let path = workspace
            .library_db()
            .latest(ElementKind::Device, uuid)?
            .ok_or_else(|| ToolError::not_found("Imported official device disappeared."))?;
        let device = Device::open(TransactionalDirectory::new(
            Arc::new(TransactionalFileSystem::open_ro(&path)?),
            "",
        ))?;
        details.push(crate::tools::library::workspace_device_details(workspace, &device)?.0);
    }
    Ok(details)
}
