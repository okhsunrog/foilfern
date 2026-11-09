//! The MCP server: tool registration (rmcp tool router) on top of the
//! synchronous tool implementations in [`tools`](crate::tools).
//!
//! Every tool runs its core work in `tokio::task::spawn_blocking` through
//! [`McpState::call()`], which locks the session's workspace and project
//! only inside the blocking closure (never across `.await`) and notifies
//! the host afterwards. Results use the envelope of
//! [`outcome`](crate::outcome). `autoroute` with Freerouting runs the
//! router between two locked steps (export, strict import), so other tools
//! and the host application stay responsive meanwhile. The host is asked
//! (without any lock held) before workspace and project lifecycle tools.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, JsonObject,
    ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};

use crate::error::{ToolError, ToolResult};
use crate::host::{HostDecision, McpHost, McpState, ProjectCreateRequest, StandaloneHost};
use crate::outcome::{ToolOutput, error_result};
use crate::session::{Session, absolute_path};
use crate::tools::board_edit::{
    AutorouteArgs, AutoroutePlan, BoardAddArgs, BoardArgs, BoardSetOutlineArgs, DesignRulesSetArgs,
    DeviceAutoPlaceArgs, DevicePlaceArgs, PlaneAddArgs, SpecctraExportArgs, SpecctraImportArgs,
    TraceAddArgs, TraceRemoveArgs, ViaAddArgs,
};
use crate::tools::circuit::{ComponentGetArgs, NetListArgs};
use crate::tools::import::LibraryImportArgs;
use crate::tools::layout::{BoardGetArgs, SchematicGetArgs};
use crate::tools::library::{LibraryElementArgs, LibraryInstallArgs, LibrarySearchArgs};
use crate::tools::mutation::{MutationApplyArgs, MutationSchemaArgs};
use crate::tools::output::{
    DrcArgs, ErcArgs, ExportBomArgs, ExportFabricationArgs, ExportNetlistArgs, ExportPickPlaceArgs,
    JobsRunArgs, RenderArgs,
};
use crate::tools::project::{
    ProjectCloseArgs, ProjectCreateArgs, ProjectOpenArgs, ProjectSaveArgs, WorkspacePathArgs,
};
use crate::tools::schematic_edit::{
    ComponentAddArgs, ComponentRemoveArgs, ComponentUpdateArgs, ConnectArgs, DisconnectArgs,
    NetClassSetArgs, NetRenameArgs, SchematicAddArgs, SchematicTidyArgs, SymbolMoveArgs,
};
use crate::tools::write::UndoArgs;
use crate::tools::{
    board_edit, circuit, import, layout, library, mutation, output, project, schematic_edit, write,
};

/// Instructions sent to the client on initialization (the agent's
/// workflow guide).
pub const INSTRUCTIONS: &str = "LibrePCB MCP server: design schematics and PCBs end to end; \
the files open in upstream LibrePCB. One workspace (libraries) and one project are open at a \
time.\n\
Workflow: workspace_open/workspace_create -> library_install (e.g. [\"LibrePCB Base\"]) or \
library_rescan -> library_search (find parts; multi-word queries like \"resistor 0805\" work) -> \
project_create -> component_add (part = name or device/component UUID; placed automatically) -> \
connect (net + pins, e.g. net \"VCC\", pins [\"J1.1\",\"R1.1\"]; supply symbols like GND/VCC \
force their net name and unconnected ones of the same name are attached automatically) -> \
schematic_tidy (re-arranges the page by connectivity for readability) -> erc_run -> \
board_set_outline (width/height mm) -> device_auto_place (or device_place) -> autoroute -> \
plane_add (net \"GND\", bottom) -> drc_run -> export_fabrication / jobs_run -> project_save.\n\
Addressing: designators (\"R1\"), pins \"R1.1\" (pad name) or \"U1.VCC\" (signal name) or a \
designator alone for single-pin parts (\"GND1\"), net names, schematic/board names or indices; \
UUIDs work everywhere. Unknown arguments are rejected (invalid_argument). Units: millimeters and degrees \
(y upwards). Read tools: project_summary, component_get, netlist, schematic_get, board_get, \
unrouted, render (PNG to look at the design).\n\
Every write is one undo step \"AI: <tool>\" (undo/redo/history) and returns the re-read \
entities, the change events and the new revision; pass expected_revision to avoid lost \
updates. Results: {outcome: complete|partial|failed, revision, result, warnings}; errors have a \
stable kind (not_found, invalid_argument, conflict, stale_revision, no_project, no_workspace, \
io, network, not_available, refused, internal). Nothing is written to disk before project_save. \
When the server runs inside the LibrePCB app, the user sees your edits live and shares the undo \
history; the app may refuse opening or closing projects (kind refused).";

/// Message prefix of rmcp's argument deserialization errors.
const DESERIALIZE_ERROR_PREFIX: &str = "failed to deserialize parameters:";

tokio::task_local! {
    /// The name of the tool being called (for the host's change notices).
    static CURRENT_TOOL: String;
}

/// The LibrePCB MCP server (cheap to clone; clones share the state).
#[derive(Clone)]
pub struct LibrePcbMcp {
    state: McpState,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for LibrePcbMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibrePcbMcp").finish_non_exhaustive()
    }
}

impl LibrePcbMcp {
    /// Creates a standalone server (host: [`StandaloneHost`]) on `session`.
    pub fn new(session: Session) -> Self {
        Self::with_state(McpState::from_session(session, Arc::new(StandaloneHost)))
    }

    /// Creates a server on a shared state (e.g. one per HTTP client
    /// session, all working on the same project, or embedded in a host
    /// application).
    pub fn with_state(state: McpState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// Returns the shared state.
    pub fn state(&self) -> &McpState {
        &self.state
    }

    /// Returns the names of all tools.
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.into_owned())
            .collect()
    }

    /// Checks the argument names of a call of `tool` against its input
    /// schema: unknown arguments fail with `invalid_argument` (naming the
    /// unknown key and listing the valid ones) instead of being silently
    /// ignored. Unknown tools pass (the router reports them).
    pub fn check_arguments(&self, tool: &str, arguments: Option<&JsonObject>) -> ToolResult<()> {
        let (Some(tool), Some(arguments)) = (self.tool_router.get(tool), arguments) else {
            return Ok(());
        };
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(|p| p.as_object());
        let is_valid = |key: &str| properties.is_some_and(|p| p.contains_key(key));
        let unknown: Vec<&str> = arguments
            .keys()
            .map(String::as_str)
            .filter(|k| !is_valid(k))
            .collect();
        if unknown.is_empty() {
            return Ok(());
        }
        let valid: Vec<&str> = properties
            .map(|p| p.keys().map(String::as_str).collect())
            .unwrap_or_default();
        Err(ToolError::invalid(format!(
            "Unknown argument{} {} for tool {}; valid arguments: {}.",
            if unknown.len() > 1 { "s" } else { "" },
            unknown
                .iter()
                .map(|k| format!("\"{k}\""))
                .collect::<Vec<_>>()
                .join(", "),
            tool.name,
            if valid.is_empty() {
                "none".to_owned()
            } else {
                valid.join(", ")
            }
        )))
    }

    /// The revision of the open project, if any.
    async fn current_revision(&self) -> Option<u64> {
        self.blocking(|s| Ok(s.project.as_ref().map(|p| p.project().revision())))
            .await
            .ok()
            .flatten()
    }

    /// Runs `f` on the locked session in a blocking thread (see
    /// [`McpState::call()`]).
    async fn blocking<T, F>(&self, f: F) -> ToolResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Session) -> ToolResult<T> + Send + 'static,
    {
        let state = self.state.clone();
        let tool = CURRENT_TOOL.try_with(Clone::clone).unwrap_or_default();
        tokio::task::spawn_blocking(move || state.call(&tool, f))
            .await
            .map_err(|e| ToolError::internal(format!("tool failed: {e}")))?
    }

    /// Asks the host in a blocking thread (no lock held).
    async fn ask_host<T, F>(&self, f: F) -> ToolResult<HostDecision<T>>
    where
        T: Send + 'static,
        F: FnOnce(&dyn McpHost) -> HostDecision<T> + Send + 'static,
    {
        let host = Arc::clone(self.state.host());
        tokio::task::spawn_blocking(move || f(host.as_ref()))
            .await
            .map_err(|e| ToolError::internal(format!("host request failed: {e}")))
    }

    /// Returns `e` as tool result (with the current revision).
    async fn fail(&self, e: ToolError) -> Result<CallToolResult, McpError> {
        self.run(move |_| Err(e)).await
    }

    /// `workspace_open` / `workspace_create`, asking the host first.
    async fn workspace_open_or_create(
        &self,
        args: WorkspacePathArgs,
        create: bool,
    ) -> Result<CallToolResult, McpError> {
        let path = match absolute_path(&args.path) {
            Ok(path) => path,
            Err(e) => return self.fail(e).await,
        };
        match self
            .ask_host(move |h| h.open_workspace(&path, create))
            .await
        {
            Ok(HostDecision::Allow) => {
                self.run(move |s| project::workspace_open(s, args, create))
                    .await
            }
            Ok(HostDecision::Delegated(ws)) => {
                self.run(move |s| {
                    s.attach_workspace(ws);
                    project::workspace_output(s.workspace()?, create)
                })
                .await
            }
            Ok(HostDecision::Refuse(message)) => self.fail(ToolError::refused(message)).await,
            Err(e) => self.fail(e).await,
        }
    }

    /// Runs a tool implementation and builds the MCP result.
    async fn run<F>(&self, f: F) -> Result<CallToolResult, McpError>
    where
        F: FnOnce(&mut Session) -> ToolResult<ToolOutput> + Send + 'static,
    {
        let result = self
            .blocking(move |s| {
                let revision = |s: &Session| s.project.as_ref().map(|p| p.project().revision());
                Ok(match f(s) {
                    Ok(mut out) => {
                        out.revision = revision(s);
                        out.into_call_tool_result()
                    }
                    Err(e) => error_result(&e, revision(s)),
                })
            })
            .await;
        Ok(result.unwrap_or_else(|e| error_result(&e, None)))
    }
}

#[tool_router]
impl LibrePcbMcp {
    // --- Session / project -------------------------------------------------

    #[tool(
        description = "Server version, file format, units, open workspace and project (with \
                       revision and unsaved state)."
    )]
    async fn server_info(&self) -> Result<CallToolResult, McpError> {
        self.run(|s| project::server_info(s)).await
    }

    #[tool(
        description = "Open an existing LibrePCB workspace (libraries, settings). Replaces the \
                       current workspace."
    )]
    async fn workspace_open(
        &self,
        Parameters(args): Parameters<WorkspacePathArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.workspace_open_or_create(args, false).await
    }

    #[tool(
        description = "Create a new workspace in an empty (or new) directory and open it; an \
                       existing workspace is just opened."
    )]
    async fn workspace_create(
        &self,
        Parameters(args): Parameters<WorkspacePathArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.workspace_open_or_create(args, true).await
    }

    #[tool(
        description = "Create a new project (like upstream's new project wizard: default \
                       assembly variant and net class, stroke fonts, optional first schematic \
                       page \"Main\" and board \"default\" with a 100x80 mm outline), save it \
                       and open it. With eagle_schematic (and eagle_board), an EAGLE project \
                       is imported instead (pages, board, nets, library elements)."
    )]
    async fn project_create(
        &self,
        Parameters(args): Parameters<ProjectCreateArgs>,
    ) -> Result<CallToolResult, McpError> {
        if let Err(e) = self.blocking(|s| s.ensure_no_project()).await {
            return self.fail(e).await;
        }
        let request = ProjectCreateRequest {
            name: args.name.clone(),
            directory: args.directory.clone(),
            eagle_schematic: args.eagle_schematic.clone(),
            eagle_board: args.eagle_board.clone(),
        };
        match self.ask_host(move |h| h.create_project(&request)).await {
            Ok(HostDecision::Allow) => self.run(move |s| project::project_create(s, args)).await,
            Ok(HostDecision::Delegated(p)) => {
                self.run(move |s| project::project_attach(s, p, "Created"))
                    .await
            }
            Ok(HostDecision::Refuse(message)) => self.fail(ToolError::refused(message)).await,
            Err(e) => self.fail(e).await,
        }
    }

    #[tool(
        description = "Open a project (*.lpp file or its directory) and lock it like LibrePCB \
                       does. Older file formats are upgraded in memory."
    )]
    async fn project_open(
        &self,
        Parameters(args): Parameters<ProjectOpenArgs>,
    ) -> Result<CallToolResult, McpError> {
        let path = match self.blocking(|s| s.ensure_no_project()).await {
            Ok(()) => absolute_path(&args.path),
            Err(e) => Err(e),
        };
        let path = match path {
            Ok(path) => path,
            Err(e) => return self.fail(e).await,
        };
        match self.ask_host(move |h| h.open_project(&path)).await {
            Ok(HostDecision::Allow) => self.run(move |s| project::project_open(s, args)).await,
            Ok(HostDecision::Delegated(p)) => {
                self.run(move |s| project::project_attach(s, p, "Opened"))
                    .await
            }
            Ok(HostDecision::Refuse(message)) => self.fail(ToolError::refused(message)).await,
            Err(e) => self.fail(e).await,
        }
    }

    #[tool(description = "Save the open project to disk (upstream file format).")]
    async fn project_save(
        &self,
        Parameters(args): Parameters<ProjectSaveArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| project::project_save(s, args)).await
    }

    #[tool(
        description = "Close the open project and release its lock (fails with unsaved \
                       changes unless discard_changes=true)."
    )]
    async fn project_close(
        &self,
        Parameters(args): Parameters<ProjectCloseArgs>,
    ) -> Result<CallToolResult, McpError> {
        let discard = args.discard_changes;
        let project = self
            .blocking(move |s| {
                s.check_close_project(discard)?;
                s.shared_project().ok_or_else(ToolError::no_project)
            })
            .await;
        let project = match project {
            Ok(p) => p,
            Err(e) => return self.fail(e).await,
        };
        match self
            .ask_host(move |h| h.close_project(&project, discard))
            .await
        {
            Ok(HostDecision::Allow | HostDecision::Delegated(())) => {
                self.run(move |s| project::project_close(s, args)).await
            }
            Ok(HostDecision::Refuse(message)) => self.fail(ToolError::refused(message)).await,
            Err(e) => self.fail(e).await,
        }
    }

    #[tool(
        description = "Overview of the open project: metadata, revision, unsaved changes, \
                       counts, assembly variants, net classes, schematic pages and boards."
    )]
    async fn project_summary(&self) -> Result<CallToolResult, McpError> {
        self.run(|s| project::project_summary(s)).await
    }

    #[tool(description = "Undo the last write step(s) (each write tool is one step).")]
    async fn undo(
        &self,
        Parameters(args): Parameters<UndoArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| write::undo_redo(s, args, false)).await
    }

    #[tool(description = "Redo undone write step(s).")]
    async fn redo(
        &self,
        Parameters(args): Parameters<UndoArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| write::undo_redo(s, args, true)).await
    }

    #[tool(
        description = "The undo history (\"AI: <tool>\" steps, done or undone) and whether \
                       there are unsaved changes."
    )]
    async fn history(&self) -> Result<CallToolResult, McpError> {
        self.run(|s| write::history(s)).await
    }

    // --- Library -------------------------------------------------------------

    #[tool(description = "List the libraries installed in the workspace.")]
    async fn library_list(&self) -> Result<CallToolResult, McpError> {
        self.run(|s| library::library_list(s)).await
    }

    #[tool(
        description = "Rescan the workspace libraries into the library index (after adding \
                       library directories to <workspace>/data/libraries/local)."
    )]
    async fn library_rescan(&self) -> Result<CallToolResult, McpError> {
        self.run(|s| library::library_rescan(s)).await
    }

    #[tool(
        description = "Install official libraries from api.librepcb.org into the workspace \
                       (with dependencies) and rescan; without names, list the available \
                       libraries. Falls back to a git clone of the library repository if the \
                       ZIP download fails (result: method zip|git)."
    )]
    async fn library_install(
        &self,
        Parameters(args): Parameters<LibraryInstallArgs>,
    ) -> Result<CallToolResult, McpError> {
        let target = match self.blocking(|s| library::install_target(s)).await {
            Ok(t) => t,
            Err(e) => return self.run(move |_| Err(e)).await,
        };
        match library::install_download(&target, &args.libraries).await {
            Ok((available, installed)) => {
                self.run(move |s| library::install_finish(s, available, installed))
                    .await
            }
            Err(e) => self.run(move |_| Err(e)).await,
        }
    }

    #[tool(
        description = "Import an EAGLE library (*.lbr) or KiCad libraries (*.kicad_sym, \
                       *.pretty) into a local LibrePCB library of the workspace (like \
                       upstream's import wizards; the index is rescanned). dry_run=true lists \
                       the elements; elements selects some (dependencies follow)."
    )]
    async fn library_import(
        &self,
        Parameters(args): Parameters<LibraryImportArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| import::library_import(s, args)).await
    }

    #[tool(
        description = "Search library elements (components, devices, packages, symbols) by \
                       keyword or part number in the workspace libraries and the project \
                       library: whole-string matches first, then elements containing all words \
                       (e.g. \"resistor 0805\"). Devices are components with a footprint (what \
                       component_add needs for the board)."
    )]
    async fn library_search(
        &self,
        Parameters(args): Parameters<LibrarySearchArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| library::library_search(s, args)).await
    }

    #[tool(
        description = "Details of a library element: component signals, gates and symbol \
                       variants (pin to signal map); device pad to signal map, parts, package \
                       footprints with pad positions and sizes; symbol pins."
    )]
    async fn library_element(
        &self,
        Parameters(args): Parameters<LibraryElementArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| library::library_element(s, args)).await
    }

    // --- Circuit & schematic ---------------------------------------------------

    #[tool(description = "List all components: designator, value, library component, placement.")]
    async fn component_list(&self) -> Result<CallToolResult, McpError> {
        self.run(|s| circuit::component_list(s)).await
    }

    #[tool(
        description = "Details of a component: signals with nets (pin names for connect), \
                       placed symbols with pin positions, devices with pad positions."
    )]
    async fn component_get(
        &self,
        Parameters(args): Parameters<ComponentGetArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| circuit::component_get(s, args)).await
    }

    #[tool(
        description = "Add a component to the circuit and place all its symbols on a \
                       schematic page (free place if no position). `part`: device or \
                       component UUID or a name like \"Resistor\" / \"LED 3mm\" / \"GND\" \
                       (supply symbols). Library elements are copied into the project \
                       library. Returns the designator and the pins."
    )]
    async fn component_add(
        &self,
        Parameters(args): Parameters<ComponentAddArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| schematic_edit::component_add(s, args))
            .await
    }

    #[tool(
        description = "Remove a component with its symbols, wires at its pins and its devices \
                       on all boards."
    )]
    async fn component_remove(
        &self,
        Parameters(args): Parameters<ComponentRemoveArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| schematic_edit::component_remove(s, args))
            .await
    }

    #[tool(description = "Change designator, value and/or attributes of a component.")]
    async fn component_update(
        &self,
        Parameters(args): Parameters<ComponentUpdateArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| schematic_edit::component_update(s, args))
            .await
    }

    #[tool(
        description = "Move, rotate or mirror a symbol (gate) on its schematic page, or move \
                       it to a free place (auto=true). Wires follow."
    )]
    async fn symbol_move(
        &self,
        Parameters(args): Parameters<SymbolMoveArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| schematic_edit::symbol_move(s, args))
            .await
    }

    #[tool(
        description = "Connect pins to a net (created or merged as needed) and draw the \
                       schematic: nearby pins are wired, distant pins get a stub wire with a \
                       net label placed clear of other items. Pins: \"R1.1\" (pad), \"U1.VCC\" \
                       (signal) or \"GND1\" (single-pin part). Unconnected supply symbols \
                       whose net name equals the net are attached (placed next to a pin). \
                       Call once per net with all its pins."
    )]
    async fn connect(
        &self,
        Parameters(args): Parameters<ConnectArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| schematic_edit::connect(s, args)).await
    }

    #[tool(
        description = "Disconnect pins from their nets (removes the wires at the pins and \
                       traces at their pads)."
    )]
    async fn disconnect(
        &self,
        Parameters(args): Parameters<DisconnectArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| schematic_edit::disconnect(s, args)).await
    }

    #[tool(description = "Rename a net (optionally merging it into an existing net).")]
    async fn net_rename(
        &self,
        Parameters(args): Parameters<NetRenameArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| schematic_edit::net_rename(s, args)).await
    }

    #[tool(
        description = "Create or edit a net class (default trace width, via drill) and \
                       assign nets to it (e.g. wider power traces)."
    )]
    async fn net_class_set(
        &self,
        Parameters(args): Parameters<NetClassSetArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| schematic_edit::net_class_set(s, args))
            .await
    }

    #[tool(description = "Nets with their pins (R1.1 style), pads and routing items.")]
    async fn net_list(
        &self,
        Parameters(args): Parameters<NetListArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| circuit::net_list(s, args)).await
    }

    #[tool(description = "Compact netlist: net name -> pins, plus unconnected pins.")]
    async fn netlist(&self) -> Result<CallToolResult, McpError> {
        self.run(|s| circuit::netlist(s)).await
    }

    #[tool(description = "List the schematic pages.")]
    async fn schematic_list(&self) -> Result<CallToolResult, McpError> {
        self.run(|s| layout::schematic_list(s)).await
    }

    #[tool(
        description = "All items of a schematic page with coordinates: symbols with pins, net \
                       segments (junctions, lines, labels), texts."
    )]
    async fn schematic_get(
        &self,
        Parameters(args): Parameters<SchematicGetArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| layout::schematic_get(s, args)).await
    }

    #[tool(description = "Add a schematic page.")]
    async fn schematic_add(
        &self,
        Parameters(args): Parameters<SchematicAddArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| schematic_edit::schematic_add(s, args))
            .await
    }

    #[tool(
        description = "Re-arrange a schematic page for readability: places the symbols by \
                       connectivity (connected parts next to each other, room for labels), \
                       supply symbols next to their pins, and draws all nets again (direct \
                       wires where short, else net labels). Nets and boards are unchanged. \
                       Run it after connecting all nets."
    )]
    async fn schematic_tidy(
        &self,
        Parameters(args): Parameters<SchematicTidyArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| schematic_edit::schematic_tidy(s, args))
            .await
    }

    // --- Board -----------------------------------------------------------------

    #[tool(
        description = "A board: outline, devices with pads, traces, vias, planes and the \
                       unrouted connections (air wires)."
    )]
    async fn board_get(
        &self,
        Parameters(args): Parameters<BoardGetArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| layout::board_get(s, args)).await
    }

    #[tool(
        description = "Add a board (default settings, 100x80 mm outline), or a copy of an \
                       existing board (copy_from: settings and all items)."
    )]
    async fn board_add(
        &self,
        Parameters(args): Parameters<BoardAddArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::board_add(s, args)).await
    }

    #[tool(
        description = "Replace the board outline: rectangle (width, height in mm, optional \
                       origin = bottom left corner and corner_radius) or polygon."
    )]
    async fn board_set_outline(
        &self,
        Parameters(args): Parameters<BoardSetOutlineArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::board_set_outline(s, args))
            .await
    }

    #[tool(
        description = "Place (or move) the device of a component on a board: position (mm), \
                       rotation (deg), side top/bottom, optional device/footprint UUID to \
                       change the footprint."
    )]
    async fn device_place(
        &self,
        Parameters(args): Parameters<DevicePlaceArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::device_place(s, args)).await
    }

    #[tool(
        description = "Place all unplaced devices (or the given ones) inside the board \
                       outline by connectivity (connected devices close together, short air \
                       wires, connectors at the board edge, spread over the board), without \
                       overlapping footprints, keeping spacing (mm) and the board edge \
                       clearance. Deterministic."
    )]
    async fn device_auto_place(
        &self,
        Parameters(args): Parameters<DeviceAutoPlaceArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::device_auto_place(s, args))
            .await
    }

    #[tool(
        description = "Draw a trace between pads (\"R1.1\"), vias/junctions (UUID) or points \
                       (mm) over optional corner points, on one copper layer."
    )]
    async fn trace_add(
        &self,
        Parameters(args): Parameters<TraceAddArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::trace_add(s, args)).await
    }

    #[tool(description = "Add a through-hole via of a net (traces at its position connect).")]
    async fn via_add(
        &self,
        Parameters(args): Parameters<ViaAddArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::via_add(s, args)).await
    }

    #[tool(description = "Remove traces and vias: of nets, by UUID, or all (all=true).")]
    async fn trace_remove(
        &self,
        Parameters(args): Parameters<TraceRemoveArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::trace_remove(s, args)).await
    }

    #[tool(
        description = "Add a copper plane (pour) of a net, e.g. GND on the bottom layer; \
                       default outline: the whole board. Filled by planes_rebuild, drc_run \
                       and exports."
    )]
    async fn plane_add(
        &self,
        Parameters(args): Parameters<PlaneAddArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::plane_add(s, args)).await
    }

    #[tool(
        description = "Set board design rules (default trace width, via drill) and DRC \
                       minimums (clearances, widths, drills) in mm, or the inner layer count."
    )]
    async fn design_rules_set(
        &self,
        Parameters(args): Parameters<DesignRulesSetArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::design_rules_set(s, args))
            .await
    }

    #[tool(description = "Fill the copper planes of a board (derived data, not an undo step).")]
    async fn planes_rebuild(
        &self,
        Parameters(args): Parameters<BoardArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::planes_rebuild(s, args)).await
    }

    #[tool(description = "The unrouted connections (air wires) and unplaced devices of a board.")]
    async fn unrouted(
        &self,
        Parameters(args): Parameters<BoardArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::unrouted(s, args)).await
    }

    #[tool(
        description = "Route the unrouted connections of a board. backend: auto (default: \
                       Freerouting if installed, else builtin), builtin (grid router; \
                       supports nets/layers filters), freerouting. Place devices first."
    )]
    async fn autoroute(
        &self,
        Parameters(args): Parameters<AutorouteArgs>,
    ) -> Result<CallToolResult, McpError> {
        // Prepare under the lock; Freerouting then runs without the lock.
        let expected = args.expected_revision;
        let plan = match self
            .blocking(move |s| board_edit::autoroute_prepare(s, args))
            .await
        {
            Ok(plan) => plan,
            Err(e) => return self.run(move |_| Err(e)).await,
        };
        match plan {
            AutoroutePlan::Builtin(command, notes) => {
                self.run(move |s| board_edit::autoroute_builtin(s, *command, expected, notes))
                    .await
            }
            AutoroutePlan::Freerouting(job) => {
                let (job, output) = match tokio::task::spawn_blocking(move || {
                    let output = job.run();
                    (job, output)
                })
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        let e = ToolError::internal(format!("Freerouting task failed: {e}"));
                        return self.run(move |_| Err(e)).await;
                    }
                };
                self.run(move |s| board_edit::autoroute_freerouting_finish(s, *job, output))
                    .await
            }
        }
    }

    #[tool(description = "Export a board as Specctra DSN file (for external autorouters).")]
    async fn specctra_export(
        &self,
        Parameters(args): Parameters<SpecctraExportArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::specctra_export(s, args))
            .await
    }

    #[tool(
        description = "Import a Specctra session (*.ses) of an external autorouter: replaces \
                       the traces and vias of the board."
    )]
    async fn specctra_import(
        &self,
        Parameters(args): Parameters<SpecctraImportArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| board_edit::specctra_import(s, args))
            .await
    }

    // --- Checks and outputs ------------------------------------------------------

    #[tool(
        description = "Run the electrical rule check; returns ran=true and the messages \
                       (severity, text, location)."
    )]
    async fn erc_run(
        &self,
        Parameters(args): Parameters<ErcArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| output::erc_run(s, args)).await
    }

    #[tool(
        description = "Run the design rule check of a board (planes and air wires are rebuilt \
                       first); returns ran=true, counts and messages (severity, approved, \
                       location)."
    )]
    async fn drc_run(
        &self,
        Parameters(args): Parameters<DrcArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| output::drc_run(s, args)).await
    }

    #[tool(
        description = "Export Gerber and Excellon fabrication files of a board (planes are \
                       rebuilt first)."
    )]
    async fn export_fabrication(
        &self,
        Parameters(args): Parameters<ExportFabricationArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| output::export_fabrication(s, args)).await
    }

    #[tool(description = "Export the bill of materials as CSV (also returned inline).")]
    async fn export_bom(
        &self,
        Parameters(args): Parameters<ExportBomArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| output::export_bom(s, args)).await
    }

    #[tool(description = "Export pick&place data of a board as CSV.")]
    async fn export_pick_place(
        &self,
        Parameters(args): Parameters<ExportPickPlaceArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| output::export_pick_place(s, args)).await
    }

    #[tool(description = "Export the IPC-D-356A netlist of a board.")]
    async fn export_netlist(
        &self,
        Parameters(args): Parameters<ExportNetlistArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| output::export_netlist(s, args)).await
    }

    #[tool(description = "List the output jobs of the project.")]
    async fn jobs_list(&self) -> Result<CallToolResult, McpError> {
        self.run(|s| output::jobs_list(s)).await
    }

    #[tool(
        description = "Run output jobs of the project (default: all; Gerber, BOM, pick&place, \
                       ...) like upstream's output jobs dialog; returns the written files."
    )]
    async fn jobs_run(
        &self,
        Parameters(args): Parameters<JobsRunArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| output::jobs_run(s, args)).await
    }

    #[tool(
        description = "Render a schematic page or a board side as PNG image, to look at the \
                       design (target defaults to board if board or side is given)."
    )]
    async fn render(
        &self,
        Parameters(args): Parameters<RenderArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| output::render(s, args)).await
    }

    // --- Escape hatch -------------------------------------------------------------

    #[tool(
        description = "Apply a raw core Mutation (serde JSON, integer nanometers) with full \
                       validation as one undo step; returns the new revision and the change \
                       events. Prefer the intent-level tools; see mutation_schema."
    )]
    async fn mutation_apply(
        &self,
        Parameters(args): Parameters<MutationApplyArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| mutation::mutation_apply(s, args)).await
    }

    #[tool(
        description = "Describe the Mutation JSON format: conventions, variants, examples, \
                       and the prefilled update mutation of an existing entity."
    )]
    async fn mutation_schema(
        &self,
        Parameters(args): Parameters<MutationSchemaArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |s| mutation::mutation_schema(s, args)).await
    }
}

#[tool_handler]
impl ServerHandler for LibrePcbMcp {
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // Argument errors are tool errors with kind `invalid_argument` (in
        // the result envelope), not protocol errors.
        let name = request.name.clone();
        if let Err(e) = self.check_arguments(&name, request.arguments.as_ref()) {
            return Ok(error_result(&e, self.current_revision().await).into());
        }
        let context = ToolCallContext::new(self, request, context);
        let response = CURRENT_TOOL
            .scope(name.to_string(), self.tool_router.call(context))
            .await?;
        // rmcp reports argument deserialization failures as plain error
        // results; give them the envelope with `invalid_argument`.
        if let CallToolResponse::Complete(result) = &response
            && result.is_error == Some(true)
            && result.structured_content.is_none()
            && let Some(message) = result
                .content
                .first()
                .and_then(|c| c.as_text())
                .and_then(|t| t.text.strip_prefix(DESERIALIZE_ERROR_PREFIX))
        {
            let e = ToolError::invalid(format!(
                "Invalid arguments for tool {name}: {}",
                message.trim()
            ));
            return Ok(error_result(&e, self.current_revision().await).into());
        }
        Ok(response)
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "librepcb-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}
