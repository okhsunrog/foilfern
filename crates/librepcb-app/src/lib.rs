//! The LibrePCB graphical application (binary `librepcb`): upstream's Slint
//! UI with a Rust backend.
//!
//! Port of the application part of libs/librepcb/editor (`GuiApplication`,
//! `MainWindow`, `WindowSection`, `WindowTab`, the home, schematic and
//! board tabs, `SlintGraphicsView`, ...) and apps/librepcb/main.cpp. See
//! `docs/ui-design.md` for the design.
//!
//! # Architecture
//!
//! - **UI:** upstream's `libs/librepcb/ui/**` `.slint` files, copied into
//!   `ui/` (see `ui/PROVENANCE.md`) and compiled by the `librepcb-app-ui`
//!   crate (`ui/Cargo.toml`) with the `lang/` translations bundled. The UI
//!   talks to the backend only through the `Data` global (view state as
//!   structs and models) and the `Backend` global (callbacks).
//! - **Backend** (this crate): [`App`] binds both globals. It holds the
//!   workspace, the open projects ([`project::AppProject`], a shared
//!   `librepcb_editor::OpenProject`), the window sections
//!   ([`section::WindowSection`]) and their tabs ([`tabs::Tab`]).
//!   Models written by the UI (tab rows, tree items, notifications, layers)
//!   are [`models::UiModel`]s, which report UI writes asynchronously.
//! - **Scenes:** the schematic and board tabs build `librepcb-scene`
//!   scenes and render them with `librepcb-canvas` (vello_cpu) into a
//!   `slint::Image` for `Backend.render-scene`; [`canvas_view::CanvasView`]
//!   ports upstream's pan/zoom behavior. Scenes are updated incrementally
//!   from the project's change journal (`librepcb_scene::SceneSync`) when
//!   the project changes.
//! - **Embedded MCP server** ([`mcp`]): LibrePCB's MCP server runs inside
//!   the application on the same workspace and project (the project of the
//!   active tab), so agent edits appear live, share the undo stack
//!   ([`history`]: undo/redo/save buttons) and the UI can follow the agent
//!   to the schematic or board it edits.
//! - **Rule checks** ([`rule_check`]): the ERC runs automatically after
//!   changes, the DRC on demand in a worker thread; the rule check panel
//!   lists the messages, approves them and zooms to their location.
//! - **Outputs** ([`outputs`]): PDF, Gerber/Excellon, pick&place, netlist,
//!   BOM, `*.lppz` and output jobs from the menus, in worker threads with
//!   progress notifications.
//! - **Planes** ([`planes`]): the plane fragments of the boards shown in
//!   board tabs are rebuilt in worker threads when they are outdated.
//! - **Editing (M3a):** each schematic and board tab owns an editor state
//!   machine of `librepcb_editor::fsm` and adapts its scene as the FSM's
//!   view ([`tabs::schematic_view`], [`tabs::board_view`]). Pointer, key and
//!   tab action events go to the FSM; afterwards the tab syncs its scene
//!   from the change journal, highlights the selection, updates the
//!   overlays ([`tabs::editing`]) and the tool bar data. Requests of the
//!   FSMs (notifications, context menus, the "add component" dialog) are
//!   handled by the application (`app::tab_editing`, `ui/project/sceneeditor.slint`),
//!   and so are cross-probing and aborting tools of other tabs. Copy/paste
//!   uses the system clipboard with upstream's MIME types ([`clipboard`]).
//! - **Dialogs (M3b):** [`dialogs`] ports upstream's project dialogs
//!   (properties of schematic and board items, board and project setup,
//!   graphics export, output jobs, BOM review, pick&place, move/align) as
//!   form dialogs rendered by `ui/dialogs/formdialog.slint`, and the "add
//!   component" dialog (`ui/dialogs/addcomponentdialog.slint`). They are
//!   shown as overlays of the main window (`app::dialog_host`,
//!   `app::add_component_host`) and apply their changes as one undo group.
//! - **Project lifecycle and workspace (M3c/M4c):** the new project wizard,
//!   the directory lock and autosave restore prompts, autosave, the file
//!   format upgrade notification, the project library tab and updater, the
//!   workspace settings dialog ([`color_schemes`], [`shortcuts`]), the
//!   initialize workspace wizard (shown before the application exists with
//!   [`modal_dialog`]) and the EAGLE/KiCad library import wizards are form
//!   dialogs too (`app::lifecycle`, `app::library_import_host`); wizards
//!   use "Back"/"Next" extra buttons, and dialogs ask the application for
//!   actions with [`dialogs::AppRequest`].
//!
//! # Running
//!
//! ```text
//! cargo run -p librepcb-app -- [--workspace DIR] [--project FILE.lpp] [--mcp[=ADDR]] [FILE.lpp...]
//! ```
//!
//! Without `--workspace`, the workspace is chosen like upstream (see
//! [`startup`]). `--mcp` starts the embedded MCP server on
//! `127.0.0.1:8766` (`http://127.0.0.1:8766/mcp`, streamable HTTP); it can
//! also be started and stopped with the "AI" button in the status bar.
//! Point an MCP client at it, e.g. `claude mcp add --transport http
//! librepcb http://127.0.0.1:8766/mcp`.
//!
//! # Headless screenshots
//!
//! ```text
//! librepcb --screenshot out.png --project X.lpp --tab board [--size 1400x900]
//! ```
//!
//! renders the main window with Slint's software renderer into a PNG
//! without a display ([`screenshot`]). The test `tests/app/screenshot.rs`
//! does the same with an upstream test project; `tests/app/live_mcp.rs`
//! drives the embedded MCP server with an HTTP client while the headless UI
//! follows (and runs the ERC, DRC and exports). Both run Slint's timers and
//! the closures posted with `slint::invoke_from_event_loop()` through
//! [`screenshot::Headless::settle()`] and
//! [`screenshot::Headless::run_until()`].
//!
//! # Debugging the UI with agents
//!
//! Slint 1.18 has an embedded MCP server which lets agents inspect and
//! drive the running UI (element tree, screenshots, clicks, typing). It is
//! compiled in only on request:
//!
//! ```text
//! SLINT_EMIT_DEBUG_INFO=1 SLINT_MCP_PORT=8765 SLINT_BACKEND=headless \
//!     cargo run -p librepcb-app --features ui-debug-mcp -- --project X.lpp
//! ```
//!
//! - `ui-debug-mcp` enables `slint/mcp` (same as `--features slint/mcp`);
//!   never enable it by default.
//! - `SLINT_EMIT_DEBUG_INFO=1` must be set at build time: it embeds the
//!   element metadata needed for introspection (the UI crate is rebuilt
//!   when it changes).
//! - `SLINT_MCP_PORT` starts the server on `http://127.0.0.1:<port>/mcp`
//!   (convention: 8765 for the Slint UI server, 8766 for LibrePCB's own
//!   MCP server when embedded).
//! - `SLINT_BACKEND=headless` uses a windowless software-rendered backend
//!   (works without a display, e.g. in the cloud sandbox).
//!
//! Then talk JSON-RPC over HTTP: `initialize` (returns usage
//! instructions), `tools/list`, and `tools/call` with tools such as
//! `list_windows`, `get_element_tree`, `find_elements_by_id`,
//! `take_screenshot` (base64 PNG), `click_element`, `drag_element`,
//! `dispatch_pointer_scroll` and `dispatch_key_event`. The scene touch
//! areas have the ids `SchematicTab::ta` and `Board2dTab::ta`.
//! `docs/ui-design.md` ("Debugging the UI with agents") has a complete
//! example session. Combined with `--mcp`, an agent can edit the design
//! through LibrePCB's MCP server (port 8766) and watch the result through
//! Slint's (port 8765).

pub mod app;
pub mod canvas_view;
pub mod clipboard;
pub mod color_schemes;
pub mod dialogs;
pub mod file_dialog;
pub mod helpers;
pub mod history;
pub mod icons;
pub mod length_edit;
pub mod libraries;
pub mod library_manager;
pub mod mcp;
pub mod modal_dialog;
pub mod models;
pub mod network;
pub mod notifications;
pub mod open_library;
pub mod outputs;
pub mod planes;
pub mod project;
pub mod rule_check;
pub mod screenshot;
pub mod section;
pub mod shortcuts;
pub mod startup;
pub mod tabs;
pub mod theme;
pub mod validation;
pub mod workspace_models;

pub use app::{App, InitialTab};
pub use librepcb_app_ui as ui;
