//! Library tools: `library_list`, `library_rescan`, `library_install`,
//! `library_search`, `library_element`.
//!
//! With a workspace, elements are found through the workspace library
//! database (the SQLite index shared with upstream); the project library
//! is searched as well. Without a workspace, only the project library is
//! available.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::types::Uuid;
use librepcb_core::workspace::{ElementKind, LibraryDb, ScanOutcome, SearchQuery, Workspace};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::resolve::required_uuid;
use crate::session::Session;
use crate::units::{deg, mm};
use crate::views;

/// Kind of a library element, as used in tool arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LibKind {
    /// Components (the circuit part: signals, symbols).
    Component,
    /// Devices (component + package + pad-signal map, what is placed on a
    /// board).
    Device,
    /// Packages (footprints and pads).
    Package,
    /// Symbols (schematic graphics and pins).
    Symbol,
}

impl From<LibKind> for ElementKind {
    fn from(k: LibKind) -> Self {
        match k {
            LibKind::Component => ElementKind::Component,
            LibKind::Device => ElementKind::Device,
            LibKind::Package => ElementKind::Package,
            LibKind::Symbol => ElementKind::Symbol,
        }
    }
}

/// Arguments of `library_search`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibrarySearchArgs {
    /// Keyword searched in names and keywords of all languages (also in
    /// part numbers of devices); a UUID matches exactly. Whole-string
    /// matches come first (`match: "whole"`); if there are fewer than
    /// `limit`, elements containing all words of the query in their name,
    /// keywords, description or (devices) component/package name follow
    /// (`match: "all_words"`), e.g. "resistor 0805".
    pub query: String,
    /// Restrict to one kind (default: components, devices, packages and
    /// symbols).
    #[serde(default)]
    pub kind: Option<LibKind>,
    /// Maximum number of results (default 25).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Arguments of `library_element`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryElementArgs {
    /// UUID of the element.
    pub uuid: String,
    /// Kind of the element (default: detected).
    #[serde(default)]
    pub kind: Option<LibKind>,
}

/// Arguments of `library_install`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LibraryInstallArgs {
    /// Names or UUIDs of the official libraries to install (dependencies
    /// are installed too), e.g. `["LibrePCB Base"]`. Empty: only list the
    /// available libraries. If a ZIP download fails (e.g. blocked by a
    /// proxy), the library repository is cloned with `git` if available.
    #[serde(default)]
    pub libraries: Vec<String>,
}

fn locale_order(ws: &Workspace) -> Vec<String> {
    ws.settings().library_locale_order.get().clone()
}

/// `library_list`.
pub fn library_list(session: &Session) -> ToolResult<ToolOutput> {
    let ws = session.workspace()?;
    let db = ws.library_db();
    let locales = locale_order(ws);
    let mut libs = Vec::new();
    for (version, dir) in db.all(ElementKind::Library, None, None)? {
        let Some(info) = db.metadata(ElementKind::Library, &dir)? else {
            continue;
        };
        let tr = db
            .translations(ElementKind::Library, &dir, &locales)?
            .unwrap_or_default();
        let source = if dir.is_located_in_dir(&ws.remote_libraries_path()) {
            "remote"
        } else {
            "local"
        };
        libs.push(json!({
            "uuid": info.uuid,
            "name": tr.name,
            "description": tr.description,
            "version": version.to_string(),
            "deprecated": info.deprecated,
            "source": source,
            "directory": dir.to_native(),
        }));
    }
    libs.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    ToolOutput::new(
        format!("{} libraries installed.", libs.len()),
        json!({ "libraries": libs }),
    )
}

/// Rescans the workspace libraries (blocking).
pub fn rescan(ws: &Workspace) -> ToolResult<usize> {
    let abort = AtomicBool::new(false);
    match ws.library_db().rescan(&abort, &mut |_| {})? {
        ScanOutcome::Succeeded { element_count } => Ok(element_count),
        ScanOutcome::Aborted => Err(ToolError::internal("library scan aborted")),
    }
}

/// `library_rescan`.
pub fn library_rescan(session: &Session) -> ToolResult<ToolOutput> {
    let ws = session.workspace()?;
    let count = rescan(ws)?;
    let libraries = ws.library_db().all(ElementKind::Library, None, None)?.len();
    ToolOutput::new(
        format!("Scanned {libraries} libraries with {count} elements."),
        json!({ "library_count": libraries, "element_count": count }),
    )
}

/// What `library_install` needs from the session (taken before the async
/// download, so the session lock is not held while downloading).
#[derive(Debug, Clone)]
pub struct InstallTarget {
    /// API server URL.
    pub api_url: String,
    /// `<workspace>/data/libraries/remote`.
    pub remote_dir: FilePath,
    /// Installed library directories by UUID.
    pub existing: HashMap<Uuid, HashSet<FilePath>>,
}

/// Prepares `library_install`.
pub fn install_target(session: &Session) -> ToolResult<InstallTarget> {
    let ws = session.workspace()?;
    let api_url = ws
        .settings()
        .api_endpoints
        .get()
        .iter()
        .find(|e| e.use_for_libraries)
        .map(|e| e.url.clone())
        .unwrap_or_else(|| librepcb_core::workspace::OFFICIAL_API_URL.to_owned());
    Ok(InstallTarget {
        api_url,
        remote_dir: ws.remote_libraries_path(),
        existing: librepcb_network::installed_library_dirs(ws.library_db())?,
    })
}

/// Downloads and installs libraries (async part of `library_install`).
/// Returns the available libraries (JSON) and the installed ones.
pub async fn install_download(
    target: &InstallTarget,
    queries: &[String],
) -> ToolResult<(Vec<Value>, Vec<librepcb_network::InstalledLibrary>)> {
    use librepcb_network::{ApiEndpoint, ClientInfo, NetworkAccessManager, Url};
    let url = Url::parse(&target.api_url)
        .map_err(|e| ToolError::invalid(format!("invalid API URL {}: {e}", target.api_url)))?;
    let nam = NetworkAccessManager::new(ClientInfo {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        git_revision: String::new(),
        file_format_version: librepcb_core::application::file_format_version(),
        locale: "en_US".to_owned(),
    })?;
    let endpoint = ApiEndpoint::new(url);
    let available = librepcb_network::fetch_library_list(&nam, &endpoint).await?;
    let list: Vec<Value> = available
        .iter()
        .map(|l| {
            json!({
                "uuid": l.uuid,
                "name": l.name,
                "description": l.description,
                "version": l.version.to_string(),
                "recommended": l.recommended,
            })
        })
        .collect();
    if queries.is_empty() {
        return Ok((list, Vec::new()));
    }
    let selected = librepcb_network::select_libraries(&available, queries, true)?;
    let installed =
        librepcb_network::install_libraries(&nam, &selected, &target.remote_dir, &target.existing)
            .await?;
    Ok((list, installed))
}

/// Finishes `library_install` (rescan, blocking).
pub fn install_finish(
    session: &Session,
    available: Vec<Value>,
    installed: Vec<librepcb_network::InstalledLibrary>,
) -> ToolResult<ToolOutput> {
    if installed.is_empty() {
        return ToolOutput::new(
            format!("{} libraries available for installation.", available.len()),
            json!({ "available": available, "installed": [] }),
        );
    }
    let count = rescan(session.workspace()?)?;
    let names: Vec<String> = installed
        .iter()
        .map(|l| format!("{} v{} ({})", l.name, l.version, l.method.as_str()))
        .collect();
    let installed: Vec<Value> = installed
        .iter()
        .map(|l| {
            json!({
                "uuid": l.uuid,
                "name": l.name,
                "version": l.version.to_string(),
                "directory": l.directory.to_native(),
                // "zip" (download) or "git" (repository clone after a
                // failed download).
                "method": l.method.as_str(),
            })
        })
        .collect();
    ToolOutput::new(
        format!(
            "Installed {}; the index now has {count} elements.",
            names.join(", ")
        ),
        json!({ "installed": installed, "element_count": count }),
    )
}

/// `library_search`.
pub fn library_search(session: &Session, args: LibrarySearchArgs) -> ToolResult<ToolOutput> {
    let limit = args.limit.unwrap_or(25).clamp(1, 500);
    let kinds: Vec<ElementKind> = match args.kind {
        Some(k) => vec![k.into()],
        None => vec![
            ElementKind::Component,
            ElementKind::Device,
            ElementKind::Package,
            ElementKind::Symbol,
        ],
    };
    let mut results = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(ws) = &session.workspace {
        let db = ws.library_db();
        let mut query = SearchQuery {
            keyword: args.query.clone(),
            kinds: kinds.clone(),
            include_parts: true,
            locale_order: locale_order(ws),
            limit: Some(limit),
        };
        // First pass: upstream semantics (the whole string), exact names
        // first.
        let mut summaries = db.search(&query)?;
        let needle = args.query.trim().to_lowercase();
        summaries.sort_by_key(|s| {
            let name = s.name.to_lowercase();
            if name == needle {
                0
            } else if name.contains(&needle) {
                1
            } else {
                2
            }
        });
        let mut matches: Vec<(&str, _)> = summaries.into_iter().map(|s| ("whole", s)).collect();
        // Second pass: all words of a multi-word query ("resistor 0805").
        if matches.len() < limit {
            query.limit = None;
            let whole: HashSet<(ElementKind, Uuid)> =
                matches.iter().map(|(_, s)| (s.kind, s.uuid)).collect();
            let more = db.search_all_tokens(&SearchQuery {
                limit: Some(limit + whole.len()),
                ..query
            })?;
            matches.extend(
                more.into_iter()
                    .filter(|s| !whole.contains(&(s.kind, s.uuid)))
                    .map(|s| ("all_words", s)),
            );
            matches.truncate(limit);
        }
        for (how, s) in matches {
            seen.insert((s.kind, s.uuid));
            let mut o = json!({
                "kind": s.kind,
                "uuid": s.uuid,
                "name": s.name,
                "description": first_line(&s.description),
                "keywords": s.keywords,
                "version": s.version.to_string(),
                "deprecated": s.deprecated,
                "source": "workspace",
                "match": how,
            });
            if let Some(dev) = s.device {
                o["component"] = element_ref(db, ElementKind::Component, dev.component_uuid, ws);
                o["package"] = element_ref(db, ElementKind::Package, dev.package_uuid, ws);
            }
            results.push(o);
        }
    }
    // Project library.
    if let Some(open) = &session.project {
        let lib = open.project().library();
        let needle = args.query.trim().to_lowercase();
        let tokens: Vec<&str> = needle.split_whitespace().collect();
        let matches = |uuid: &Uuid, name: &str, description: &str| {
            let text = format!("{name}\n{description}").to_lowercase();
            needle.is_empty()
                || name.to_lowercase().contains(&needle)
                || tokens.iter().all(|t| text.contains(t))
                || uuid.to_string() == needle
        };
        let mut push = |kind: ElementKind, uuid: Uuid, name: &str, description: &str| {
            if results.len() < limit
                && !seen.contains(&(kind, uuid))
                && matches(&uuid, name, description)
            {
                results.push(json!({
                    "kind": kind,
                    "uuid": uuid,
                    "name": name,
                    "description": first_line(description),
                    "source": "project",
                }));
            }
        };
        for kind in &kinds {
            match kind {
                ElementKind::Component => lib.components().values().for_each(|e| {
                    push(
                        *kind,
                        e.metadata().uuid(),
                        e.metadata().name().as_str(),
                        default_description(e),
                    )
                }),
                ElementKind::Device => lib.devices().values().for_each(|e| {
                    push(
                        *kind,
                        e.metadata().uuid(),
                        e.metadata().name().as_str(),
                        default_description(e),
                    )
                }),
                ElementKind::Package => lib.packages().values().for_each(|e| {
                    push(
                        *kind,
                        e.metadata().uuid(),
                        e.metadata().name().as_str(),
                        default_description(e),
                    )
                }),
                ElementKind::Symbol => lib.symbols().values().for_each(|e| {
                    push(
                        *kind,
                        e.metadata().uuid(),
                        e.metadata().name().as_str(),
                        default_description(e),
                    )
                }),
                _ => {}
            }
        }
    }
    let out = ToolOutput::new(
        format!("{} element(s) found for \"{}\".", results.len(), args.query),
        json!({ "elements": results }),
    )?;
    Ok(if session.workspace.is_none() {
        out.warn("No workspace is open: only the project library was searched.")
    } else {
        out
    })
}

fn default_description<E: LibraryBaseElement>(e: &E) -> &str {
    e.metadata().descriptions().default_value()
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

fn element_ref(db: &LibraryDb, kind: ElementKind, uuid: Uuid, ws: &Workspace) -> Value {
    let name = db
        .element_summary(kind, uuid, &locale_order(ws))
        .ok()
        .flatten()
        .map(|s| s.name);
    json!({ "uuid": uuid, "name": name })
}

/// A library element loaded from the workspace or the project library.
enum Loaded<'a, T> {
    Owned(T),
    Borrowed(&'a T),
}

impl<T> std::ops::Deref for Loaded<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Loaded::Owned(t) => t,
            Loaded::Borrowed(t) => t,
        }
    }
}

/// Element lookup in the workspace library database and the project
/// library.
pub struct ElementSource<'a> {
    session: &'a Session,
}

impl<'a> ElementSource<'a> {
    /// Creates a lookup over the session's workspace and project.
    pub fn new(session: &'a Session) -> Self {
        Self { session }
    }

    fn workspace_dir(&self, kind: ElementKind, uuid: Uuid) -> ToolResult<Option<FilePath>> {
        match &self.session.workspace {
            Some(ws) => Ok(ws.library_db().element_dir(kind, uuid)?),
            None => Ok(None),
        }
    }

    fn open<E: LibraryBaseElement>(dir: &FilePath) -> ToolResult<E> {
        let fs = TransactionalFileSystem::open_ro(dir)?;
        Ok(E::open(TransactionalDirectory::new(Arc::new(fs), ""))?)
    }

    fn load<E: LibraryBaseElement>(
        &self,
        kind: ElementKind,
        uuid: Uuid,
        project: impl Fn(&'a librepcb_core::project::ProjectLibrary) -> Option<&'a E>,
    ) -> ToolResult<Option<(Loaded<'a, E>, &'static str)>> {
        if let Some(open) = &self.session.project
            && let Some(e) = project(open.project().library())
        {
            return Ok(Some((Loaded::Borrowed(e), "project")));
        }
        match self.workspace_dir(kind, uuid)? {
            Some(dir) => Ok(Some((Loaded::Owned(Self::open(&dir)?), "workspace"))),
            None => Ok(None),
        }
    }

    fn component(&self, uuid: Uuid) -> ToolResult<Option<(Loaded<'a, Component>, &'static str)>> {
        self.load(ElementKind::Component, uuid, move |l| l.component(&uuid))
    }
    fn device(&self, uuid: Uuid) -> ToolResult<Option<(Loaded<'a, Device>, &'static str)>> {
        self.load(ElementKind::Device, uuid, move |l| l.device(&uuid))
    }
    fn package(&self, uuid: Uuid) -> ToolResult<Option<(Loaded<'a, Package>, &'static str)>> {
        self.load(ElementKind::Package, uuid, move |l| l.package(&uuid))
    }
    fn symbol(&self, uuid: Uuid) -> ToolResult<Option<(Loaded<'a, Symbol>, &'static str)>> {
        self.load(ElementKind::Symbol, uuid, move |l| l.symbol(&uuid))
    }

    /// Name of an element (for references), if it can be found.
    fn name(&self, kind: ElementKind, uuid: Uuid) -> Option<String> {
        if let Some(open) = &self.session.project {
            let lib = open.project().library();
            let name = match kind {
                ElementKind::Component => lib
                    .component(&uuid)
                    .map(|e| e.metadata().name().as_str().to_owned()),
                ElementKind::Device => lib
                    .device(&uuid)
                    .map(|e| e.metadata().name().as_str().to_owned()),
                ElementKind::Package => lib
                    .package(&uuid)
                    .map(|e| e.metadata().name().as_str().to_owned()),
                ElementKind::Symbol => lib
                    .symbol(&uuid)
                    .map(|e| e.metadata().name().as_str().to_owned()),
                _ => None,
            };
            if name.is_some() {
                return name;
            }
        }
        let ws = self.session.workspace.as_ref()?;
        ws.library_db()
            .element_summary(kind, uuid, &locale_order(ws))
            .ok()
            .flatten()
            .map(|s| s.name)
    }
}

/// `library_element`.
pub fn library_element(session: &Session, args: LibraryElementArgs) -> ToolResult<ToolOutput> {
    let uuid = required_uuid(&args.uuid, "element")?;
    let src = ElementSource::new(session);
    let kinds: Vec<LibKind> = match args.kind {
        Some(k) => vec![k],
        None => vec![
            LibKind::Component,
            LibKind::Device,
            LibKind::Package,
            LibKind::Symbol,
        ],
    };
    for kind in kinds {
        let detail = match kind {
            LibKind::Component => src
                .component(uuid)?
                .map(|(c, s)| component_detail(&src, &c, s)),
            LibKind::Device => src.device(uuid)?.map(|(d, s)| device_detail(&src, &d, s)),
            LibKind::Package => src.package(uuid)?.map(|(p, s)| {
                let mut v = element_header(&*p, "package", s);
                v["assembly_type"] = json!(p.assembly_type().to_str());
                v["pads"] = json!(
                    p.pads()
                        .iter()
                        .map(|pad| {
                            let mut o = default_pad_geometry(&p, pad.uuid());
                            o["uuid"] = json!(pad.uuid());
                            o["name"] = json!(pad.name().as_str());
                            o
                        })
                        .collect::<Vec<_>>()
                );
                v["footprints"] = footprints(&p);
                Ok(v)
            }),
            LibKind::Symbol => src.symbol(uuid)?.map(|(sym, s)| {
                let mut v = element_header(&*sym, "symbol", s);
                v["pins"] = symbol_pins(&sym);
                Ok(v)
            }),
        };
        if let Some(detail) = detail {
            let detail = detail?;
            let summary = format!(
                "{} \"{}\" ({uuid}).",
                detail["kind"].as_str().unwrap_or_default(),
                detail["name"].as_str().unwrap_or_default()
            );
            return ToolOutput::new(summary, detail);
        }
    }
    Err(ToolError::new(
        ErrorKind::NotFound,
        format!(
            "No library element with the UUID {uuid} found{}.",
            if session.workspace.is_none() {
                " (no workspace open: only the project library was searched)"
            } else {
                ""
            }
        ),
    ))
}

fn element_header<E: LibraryBaseElement>(e: &E, kind: &str, source: &str) -> Value {
    let md = e.metadata();
    json!({
        "kind": kind,
        "uuid": md.uuid(),
        "name": md.name().as_str(),
        "description": md.descriptions().default_value(),
        "keywords": md.keywords().default_value(),
        "version": md.version().to_string(),
        "author": md.author(),
        "deprecated": md.is_deprecated(),
        "source": source,
    })
}

fn symbol_pins(sym: &Symbol) -> Value {
    Value::Array(
        sym.pins()
            .iter()
            .map(|p| {
                json!({
                    "uuid": p.uuid(),
                    "name": p.name().as_str(),
                    "position": views::point(p.position()),
                    "rotation": deg(p.rotation()),
                    "length": mm(p.length()),
                })
            })
            .collect(),
    )
}

/// The geometry of a package pad in the default (first) footprint, or an
/// empty object if the footprint has no such pad.
fn default_pad_geometry(pkg: &Package, pad_uuid: Uuid) -> Value {
    pkg.footprints()
        .iter()
        .next()
        .and_then(|fp| {
            fp.pads()
                .iter()
                .find(|fpad| fpad.package_pad_uuid() == Some(pad_uuid))
        })
        .map(|fpad| {
            let pad = fpad.pad();
            views::pad_geometry(pad, pad.position(), pad.rotation())
        })
        .unwrap_or_else(|| json!({}))
}

fn footprints(pkg: &Package) -> Value {
    Value::Array(
        pkg.footprints()
            .iter()
            .map(|fp| {
                let pads: Vec<Value> = fp
                    .pads()
                    .iter()
                    .map(|fpad| {
                        let pad = fpad.pad();
                        let mut v = views::pad_geometry(pad, pad.position(), pad.rotation());
                        v["uuid"] = json!(fpad.uuid());
                        v["name"] = json!(
                            fpad.package_pad_uuid()
                                .and_then(|u| pkg.pads().by_uuid(&u))
                                .map(|p| p.name().as_str())
                        );
                        v
                    })
                    .collect();
                json!({
                    "uuid": fp.uuid(),
                    "name": fp.names().default_value().as_str(),
                    "pads": pads,
                })
            })
            .collect(),
    )
}

fn component_detail(src: &ElementSource<'_>, c: &Component, source: &str) -> ToolResult<Value> {
    let mut v = element_header(c, "component", source);
    v["schematic_only"] = json!(c.schematic_only());
    v["default_value"] = json!(c.default_value());
    v["prefix"] = json!(c.prefixes().default_value().as_str());
    v["attributes"] = views::attributes(c.attributes());
    v["signals"] = Value::Array(
        c.signals()
            .iter()
            .map(|s| {
                json!({
                    "uuid": s.uuid(),
                    "name": s.name().as_str(),
                    "role": s.role().to_str(),
                    "required": s.is_required(),
                    "forced_net_name": (!s.forced_net_name().is_empty()).then(|| s.forced_net_name()),
                })
            })
            .collect(),
    );
    let mut variants = Vec::new();
    for var in c.symbol_variants().iter() {
        let mut gates = Vec::new();
        for gate in var.symbol_items().iter() {
            let symbol = src.symbol(gate.symbol_uuid()).ok().flatten();
            let pins: Vec<Value> = gate
                .pin_signal_map()
                .iter()
                .map(|m| {
                    let pin_name = symbol.as_ref().and_then(|(s, _)| {
                        s.pins()
                            .by_uuid(&m.pin_uuid())
                            .map(|p| p.name().as_str().to_owned())
                    });
                    let signal_name = m
                        .signal_uuid()
                        .and_then(|u| c.signals().by_uuid(&u))
                        .map(|s| s.name().as_str().to_owned());
                    json!({
                        "pin": m.pin_uuid(),
                        "pin_name": pin_name,
                        "signal": m.signal_uuid(),
                        "signal_name": signal_name,
                    })
                })
                .collect();
            gates.push(json!({
                "uuid": gate.uuid(),
                "symbol": {
                    "uuid": gate.symbol_uuid(),
                    "name": symbol.as_ref().map(|(s, _)| s.metadata().name().as_str().to_owned()),
                },
                "suffix": gate.suffix().as_str(),
                "position": views::point(gate.symbol_position()),
                "rotation": deg(gate.symbol_rotation()),
                "required": gate.is_required(),
                "pins": pins,
            }));
        }
        variants.push(json!({
            "uuid": var.uuid(),
            "name": var.name().as_str(),
            "norm": var.norm(),
            "gates": gates,
        }));
    }
    v["symbol_variants"] = Value::Array(variants);
    // Devices implementing this component.
    let mut devices = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(open) = &src.session.project {
        for d in open
            .project()
            .library()
            .devices_of_component(&c.metadata().uuid())
        {
            seen.insert(d.metadata().uuid());
            devices.push(json!({ "uuid": d.metadata().uuid(), "name": d.metadata().name().as_str(), "source": "project" }));
        }
    }
    if let Some(ws) = &src.session.workspace {
        for uuid in ws.library_db().component_devices(c.metadata().uuid())? {
            if seen.insert(uuid) {
                devices.push(json!({
                    "uuid": uuid,
                    "name": src.name(ElementKind::Device, uuid),
                    "source": "workspace",
                }));
            }
        }
    }
    v["devices"] = Value::Array(devices);
    Ok(v)
}

fn device_detail(src: &ElementSource<'_>, d: &Device, source: &str) -> ToolResult<Value> {
    let mut v = element_header(d, "device", source);
    let component = src.component(d.component_uuid())?;
    let package = src.package(d.package_uuid())?;
    v["component"] = json!({
        "uuid": d.component_uuid(),
        "name": component.as_ref().map(|(c, _)| c.metadata().name().as_str().to_owned()),
    });
    v["package"] = json!({
        "uuid": d.package_uuid(),
        "name": package.as_ref().map(|(p, _)| p.metadata().name().as_str().to_owned()),
    });
    v["attributes"] = views::attributes(d.attributes());
    v["parts"] = Value::Array(
        d.parts()
            .iter()
            .map(|p| {
                json!({
                    "mpn": p.mpn().as_str(),
                    "manufacturer": p.manufacturer().as_str(),
                    "attributes": views::attributes(p.attributes()),
                })
            })
            .collect(),
    );
    v["pads"] = Value::Array(
        d.pad_signal_map()
            .iter()
            .map(|m| {
                let pad_name = package.as_ref().and_then(|(p, _)| {
                    p.pads()
                        .by_uuid(&m.pad_uuid())
                        .map(|p| p.name().as_str().to_owned())
                });
                let signal_name = m.signal_uuid().and_then(|u| {
                    component.as_ref().and_then(|(c, _)| {
                        c.signals()
                            .by_uuid(&u)
                            .map(|s| s.name().as_str().to_owned())
                    })
                });
                // Geometry of the default footprint (all footprints are
                // listed under `footprints`).
                let mut o = package
                    .as_ref()
                    .map(|(p, _)| default_pad_geometry(p, m.pad_uuid()))
                    .unwrap_or_else(|| json!({}));
                for (key, value) in [
                    ("pad", json!(m.pad_uuid())),
                    ("pad_name", json!(pad_name)),
                    ("signal", json!(m.signal_uuid())),
                    ("signal_name", json!(signal_name)),
                    ("optional", json!(m.is_optional())),
                ] {
                    o[key] = value;
                }
                o
            })
            .collect(),
    );
    if let Some((p, _)) = &package {
        v["footprints"] = footprints(p);
    }
    Ok(v)
}
