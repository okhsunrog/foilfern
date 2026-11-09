//! Session and project tools: `server_info`, `workspace_open`,
//! `workspace_create`, `project_create`, `project_open`, `project_save`,
//! `project_close`, `project_summary`.

use std::sync::Arc;

use librepcb_core::fileio::{
    CleanFileNameOptions, FileNameCase, FilePath, FileSystem, RestoreMode, TransactionalDirectory,
    TransactionalFileSystem,
};
use librepcb_core::geometry::Path;
use librepcb_core::project::board::{Board, BoardItem, BoardPolygonData};
use librepcb_core::project::schematic::Schematic;
use librepcb_core::project::{Mutation, Project};
use librepcb_core::types::{ElementName, Layer, Length, Point, UnsignedLength, Uuid};
use librepcb_core::utils::message_logger::{LogLevel, MessageLogger};
use librepcb_import::eagle::EagleProjectImport;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::{OpenProject, Session, SharedProject, absolute_path};
pub use crate::tools::write::check_revision;

/// Arguments of `workspace_open` / `workspace_create`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePathArgs {
    /// Workspace directory (absolute, or relative to the server's working
    /// directory).
    pub path: String,
}

/// Arguments of `project_create`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectCreateArgs {
    /// Project name (also used for the `*.lpp` file name).
    pub name: String,
    /// Project directory (created if needed, must not contain a project).
    /// Default: `<workspace>/projects/<name>`.
    #[serde(default)]
    pub directory: Option<String>,
    /// Author (default: the workspace user name).
    #[serde(default)]
    pub author: Option<String>,
    /// Add a first schematic page (default true).
    #[serde(default = "default_true")]
    pub create_schematic: bool,
    /// Name of the schematic page (default "Main").
    #[serde(default)]
    pub schematic_name: Option<String>,
    /// Add a board with default settings and a 100 x 80 mm outline
    /// (default true).
    #[serde(default = "default_true")]
    pub create_board: bool,
    /// Name of the board (default "default").
    #[serde(default)]
    pub board_name: Option<String>,
    /// Import an EAGLE project: its schematic (`*.sch`). The schematic
    /// pages, board, library elements and nets are imported into the new
    /// project (upstream's new project wizard with EAGLE import);
    /// `create_schematic` and `create_board` are then ignored.
    #[serde(default)]
    pub eagle_schematic: Option<String>,
    /// With `eagle_schematic`: the EAGLE board (`*.brd`) to import too.
    #[serde(default)]
    pub eagle_board: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Arguments of `project_open`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectOpenArgs {
    /// The `*.lpp` project file, or the project directory.
    pub path: String,
}

/// Arguments of `project_save`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSaveArgs {
    /// Fail with `stale_revision` if the project revision differs.
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

/// Arguments of `project_close`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectCloseArgs {
    /// Close even if there are unsaved changes (they are lost).
    #[serde(default)]
    pub discard_changes: bool,
}

/// `server_info`.
pub fn server_info(session: &Session) -> ToolResult<ToolOutput> {
    let workspace = session.workspace.as_ref().map(|ws| {
        json!({
            "path": ws.path().to_native(),
            "libraries_path": ws.libraries_path().to_native(),
        })
    });
    let project = session.project.as_ref().map(|p| {
        json!({
            "path": p.file_path(),
            "name": p.project().metadata().name.as_str(),
            "revision": p.project().revision(),
            "unsaved_changes": p.has_unsaved_changes(),
        })
    });
    let summary = format!(
        "librepcb-mcp {} (file format {}); workspace: {}; project: {}",
        env!("CARGO_PKG_VERSION"),
        librepcb_core::application::file_format_version(),
        session
            .workspace
            .as_ref()
            .map_or("none".to_owned(), |ws| ws.path().to_native()),
        session
            .project
            .as_ref()
            .map_or("none".to_owned(), |p| p.file_path()),
    );
    ToolOutput::new(
        summary,
        json!({
            "name": "librepcb-mcp",
            "version": env!("CARGO_PKG_VERSION"),
            "file_format_version": librepcb_core::application::file_format_version().to_string(),
            "units": { "length": "mm", "angle": "deg" },
            "workspace": workspace,
            "project": project,
            // `null`: the resources embedded into the binary are used.
            "resources_dir": session.resources_dir.as_ref().map(|p| p.to_native()),
        }),
    )
}

/// `workspace_open` / `workspace_create`.
pub fn workspace_open(
    session: &mut Session,
    args: WorkspacePathArgs,
    create: bool,
) -> ToolResult<ToolOutput> {
    let ws = session.open_workspace(&args.path, create)?;
    workspace_output(ws, create)
}

/// The result of `workspace_open` / `workspace_create` for `ws`.
pub fn workspace_output(
    ws: &librepcb_core::workspace::Workspace,
    create: bool,
) -> ToolResult<ToolOutput> {
    let libraries = ws
        .library_db()
        .all(librepcb_core::workspace::ElementKind::Library, None, None)?
        .len();
    let path = ws.path().to_native();
    ToolOutput::new(
        format!(
            "Workspace {} {path} ({libraries} libraries indexed).",
            if create { "ready at" } else { "opened:" }
        ),
        json!({
            "path": path,
            "projects_path": ws.projects_path().to_native(),
            "libraries_path": ws.libraries_path().to_native(),
            "library_count": libraries,
        }),
    )
}

/// `project_create`.
pub fn project_create(session: &mut Session, args: ProjectCreateArgs) -> ToolResult<ToolOutput> {
    session.ensure_no_project()?;
    let name = ElementName::new(args.name.trim())?;
    let file_base = FilePath::clean_file_name(
        name.as_str(),
        CleanFileNameOptions::new(true, FileNameCase::Keep),
        120,
    );
    if file_base.is_empty() {
        return Err(ToolError::invalid(format!(
            "The project name \"{}\" gives no valid file name.",
            name.as_str()
        )));
    }
    let dir = match &args.directory {
        Some(d) => absolute_path(d)?,
        None => session
            .workspace
            .as_ref()
            .map(|ws| ws.projects_path().path_to(&file_base))
            .ok_or_else(|| {
                ToolError::invalid("No workspace is open, pass the project directory.")
            })?,
    };
    let file_name = format!("{file_base}.lpp");
    if dir.path_to(&file_name).is_existing_file()
        || dir.path_to(".librepcb-project").is_existing_file()
    {
        return Err(ToolError::new(
            ErrorKind::Conflict,
            format!(
                "The directory \"{}\" already contains a LibrePCB project.",
                dir.to_native()
            ),
        ));
    }
    let (eagle, parse_warnings) = open_eagle_project(&args)?;
    let dir_existed = dir.is_existing_dir();
    let result = create_project_files(session, &args, &name, &dir, &file_name, eagle.as_ref());
    if result.is_err() && !dir_existed {
        // Like upstream's wizard: remove the directory again on failure.
        let _ = std::fs::remove_dir_all(dir.as_path());
    }
    let (project, fs, mut warnings) = result?;
    warnings.extend(parse_warnings);
    let summary = format!(
        "Created project \"{}\" at {} ({} schematic page(s), {} board(s)).",
        name.as_str(),
        project
            .file_path()
            .map(|p| p.to_native())
            .unwrap_or_default(),
        project.schematics().len(),
        project.boards().len()
    );
    session.set_project(project, fs);
    let open = session.project()?;
    Ok(ToolOutput::new(summary, summary_json(open))?.warnings(warnings))
}

type CreatedProject = (Project, Arc<TransactionalFileSystem>, Vec<String>);

/// Opens the EAGLE project of `project_create` (if any), before anything
/// is created.
fn open_eagle_project(
    args: &ProjectCreateArgs,
) -> ToolResult<(Option<EagleProjectImport>, Vec<String>)> {
    let Some(sch) = &args.eagle_schematic else {
        if args.eagle_board.is_some() {
            return Err(ToolError::invalid(
                "eagle_board needs eagle_schematic as well.",
            ));
        }
        return Ok((None, Vec::new()));
    };
    let sch = absolute_path(sch)?;
    let brd = args.eagle_board.as_deref().map(absolute_path).transpose()?;
    let mut import = EagleProjectImport::default();
    let parse_warnings = import.open(&sch, brd.as_ref())?;
    Ok((Some(import), parse_warnings))
}

fn create_project_files(
    session: &Session,
    args: &ProjectCreateArgs,
    name: &ElementName,
    dir: &FilePath,
    file_name: &str,
    eagle: Option<&EagleProjectImport>,
) -> ToolResult<CreatedProject> {
    let mut warnings = Vec::new();
    let mut handler = |_: &FilePath, _: librepcb_core::fileio::LockStatus, _: &str| Ok(false);
    let fs = Arc::new(TransactionalFileSystem::open(
        dir,
        true,
        RestoreMode::No,
        Some(&mut handler),
    )?);
    let mut root = TransactionalDirectory::new(Arc::clone(&fs), "");

    // Stroke fonts (upstream `Project::create()` copies the application's
    // fontobene fonts into the project): from the resources directory, else
    // the fonts embedded into the binary.
    let (fonts, _) = librepcb_scene::resources::stroke_font_files()
        .map_err(|e| ToolError::new(ErrorKind::Io, e.to_string()))?;
    for (file, content) in fonts {
        root.write(&format!("resources/fontobene/{file}"), &content)?;
    }

    let mut project = Project::create(root, file_name, Uuid::new_random)?;

    // Metadata and settings (upstream `NewProjectWizard::createProject()`).
    let mut metadata = project.metadata().clone();
    metadata.name = name.clone();
    metadata.author = match &args.author {
        Some(a) => a.clone(),
        None => session
            .workspace
            .as_ref()
            .map(|ws| ws.settings().user_name.get().clone())
            .unwrap_or_default(),
    };
    project.apply(Mutation::SetProjectMetadata(metadata))?;
    if let Some(ws) = &session.workspace {
        let mut settings = project.settings().clone();
        settings.locale_order = ws.settings().library_locale_order.get().clone();
        settings.norm_order = ws.settings().library_norm_order.get().clone();
        project.apply(Mutation::SetProjectSettings(settings))?;
    }

    let dir_name = |name: &str| {
        FilePath::clean_file_name(
            name,
            CleanFileNameOptions::new(true, FileNameCase::Lower),
            120,
        )
    };
    if let Some(import) = eagle {
        // Upstream `NewProjectWizard::createProject()` with EAGLE import:
        // no initial schematic and board.
        let log = MessageLogger::new();
        let result = import.import(&mut project, &log);
        warnings.extend(
            log.messages()
                .into_iter()
                .filter(|m| m.level >= LogLevel::Warning)
                .map(|m| format!("{}: {}", m.level, m.message)),
        );
        result?;
    } else if args.create_schematic {
        let name = ElementName::new(args.schematic_name.as_deref().unwrap_or("Main").trim())?;
        let dir = dir_name(name.as_str());
        if dir.is_empty() {
            return Err(ToolError::invalid("invalid schematic name"));
        }
        project.add_schematic(Schematic::new(Uuid::new_random(), name, dir), None)?;
    }
    if eagle.is_none() && args.create_board {
        let name = ElementName::new(args.board_name.as_deref().unwrap_or("default").trim())?;
        let dir = dir_name(name.as_str());
        if dir.is_empty() {
            return Err(ToolError::invalid("invalid board name"));
        }
        let board = project.add_board(Board::new(Uuid::new_random(), name, dir), None)?;
        // Upstream `Board::addDefaultContent()`: 100x80mm outline (1/2
        // Eurocard size).
        project.add_board_item(
            board,
            BoardItem::Polygon(BoardPolygonData::new(
                Uuid::new_random(),
                Layer::BOARD_OUTLINES,
                UnsignedLength::new(Length::new(0))?,
                Path::rect(
                    Point::from_nm(0, 0),
                    Point::from_nm(100_000_000, 80_000_000),
                ),
                false,
                false,
                false,
            )),
        )?;
    }
    project.save()?;
    fs.save()?;
    Ok((project, fs, warnings))
}

/// `project_open`.
pub fn project_open(session: &mut Session, args: ProjectOpenArgs) -> ToolResult<ToolOutput> {
    let open = session.open_project(&args.path)?;
    opened_output(open, "Opened")
}

/// `project_open` / `project_create` delegated to the host application,
/// which opened (`verb`: "Opened") or created the project itself.
pub fn project_attach(
    session: &mut Session,
    project: SharedProject,
    verb: &str,
) -> ToolResult<ToolOutput> {
    let open = session.attach_project(project)?;
    opened_output(open, verb)
}

fn opened_output(open: &OpenProject, verb: &str) -> ToolResult<ToolOutput> {
    let upgraded = open.upgraded;
    let summary = format!(
        "{verb} project \"{}\" ({}).",
        open.project().metadata().name.as_str(),
        open.file_path()
    );
    let out = ToolOutput::new(summary, summary_json(open))?;
    Ok(if upgraded {
        out.warn(
            "The project was upgraded to the current file format; project_save writes it in \
             the new format (older LibrePCB versions cannot open it then).",
        )
    } else {
        out
    })
}

/// `project_save`.
pub fn project_save(session: &mut Session, args: ProjectSaveArgs) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    check_revision(open.project(), args.expected_revision)?;
    open.save()?;
    ToolOutput::new(
        format!("Saved {}.", open.file_path()),
        json!({ "path": open.file_path(), "saved_revision": open.saved_revision }),
    )
}

/// `project_close`.
pub fn project_close(session: &mut Session, args: ProjectCloseArgs) -> ToolResult<ToolOutput> {
    let path = session.project()?.file_path();
    session.close_project(args.discard_changes)?;
    ToolOutput::new(format!("Closed {path}."), json!({ "path": path }))
}

/// `project_summary`.
pub fn project_summary(session: &Session) -> ToolResult<ToolOutput> {
    let open = session.project()?;
    let p = open.project();
    let summary = format!(
        "{} (rev {}): {} components, {} nets, {} schematic page(s), {} board(s){}",
        p.metadata().name.as_str(),
        p.revision(),
        p.circuit().component_instances().len(),
        p.circuit().net_signals().len(),
        p.schematics().len(),
        p.boards().len(),
        if open.has_unsaved_changes() {
            ", unsaved changes"
        } else {
            ""
        }
    );
    ToolOutput::new(summary, summary_json(open))
}

fn summary_json(open: &OpenProject) -> serde_json::Value {
    let p = open.project();
    let md = p.metadata();
    let circuit = p.circuit();
    let schematics: Vec<_> = p
        .schematics()
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "index": i,
                "uuid": s.uuid(),
                "name": s.name().as_str(),
                "symbols": s.symbols().len(),
                "net_segments": s.net_segments().len(),
            })
        })
        .collect();
    let boards: Vec<_> = p
        .boards()
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let (traces, vias) = b.net_segments().values().fold((0, 0), |(t, v), s| {
                (t + s.traces().len(), v + s.vias().len())
            });
            json!({
                "index": i,
                "uuid": b.uuid(),
                "name": b.name().as_str(),
                "copper_layers": b.copper_layers().len(),
                "devices": b.devices().len(),
                "traces": traces,
                "vias": vias,
                "planes": b.planes().len(),
            })
        })
        .collect();
    let unplaced_devices: usize = p
        .boards()
        .first()
        .map(|b| {
            circuit
                .component_instances()
                .iter()
                .filter(|(id, c)| {
                    b.device(**id).is_none()
                        && p.library()
                            .component(&c.lib_component())
                            .is_some_and(|lc| !lc.schematic_only())
                })
                .count()
        })
        .unwrap_or(0);
    json!({
        "path": open.file_path(),
        "uuid": p.uuid(),
        "name": md.name.as_str(),
        "author": md.author,
        "version": md.version.as_str(),
        "created": md.created.to_rfc3339(),
        "revision": p.revision(),
        "saved_revision": open.saved_revision,
        "unsaved_changes": open.has_unsaved_changes(),
        "counts": {
            "components": circuit.component_instances().len(),
            "nets": circuit.net_signals().len(),
            "net_classes": circuit.net_classes().len(),
            "unplaced_devices_on_primary_board": unplaced_devices,
            "library": {
                "components": p.library().components().len(),
                "devices": p.library().devices().len(),
                "packages": p.library().packages().len(),
                "symbols": p.library().symbols().len(),
            },
        },
        "assembly_variants": circuit
            .assembly_variants()
            .iter()
            .map(|av| json!({ "uuid": av.uuid(), "name": av.name().as_str(), "description": av.description() }))
            .collect::<Vec<_>>(),
        "net_classes": circuit
            .net_classes()
            .values()
            .map(|nc| json!({ "uuid": nc.uuid(), "name": nc.name().as_str() }))
            .collect::<Vec<_>>(),
        "schematics": schematics,
        "boards": boards,
    })
}
