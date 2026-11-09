# UI design (M2 viewer → M3 editor)

Design of the graphical application `crates/librepcb-app` (binary
`librepcb`). Status: agreed with the user (copy and port upstream's `.slint` files;
embedded MCP server in M2).

## Key finding: upstream already has a Slint UI

Upstream LibrePCB 2.x replaced most of its Qt Widgets UI with Slint:
`libs/librepcb/ui/` holds 122 `.slint` files (~34k lines): main window,
window sections and tabs, home tab, schematic and board 2D tabs with their
tool bars, library editors, panels (documents, layers, ERC/DRC, place
devices, order), widgets and themes. The C++ side (`libs/librepcb/editor`)
is a backend behind a narrow API (`ui/api/*.slint`):

- `global Data`: all view state as Slint structs and models (sections →
  tabs → `SchematicTabData`, `Board2dTabData`, ...; projects, libraries,
  notifications, rule check results).
- `global Backend`: callbacks — `trigger(Action)`, `trigger-section`,
  `trigger-tab(section, tab, TabAction)`, and the canvas protocol:
  `render-scene(section, width, height, scene, frame) -> image`,
  `scene-pointer-event`, `scene-scrolled`, `scene-key-pressed/released`.

The canvas protocol is exactly our architecture: the backend renders the
scene into a `slint::Image` (upstream: tiny-skia via `SlintGraphicsView`;
here: `librepcb-canvas` with vello_cpu) and receives pointer/key events.

Slint has first-class Rust support, so the UI port is: **take upstream's
`.slint` files and port the C++ backend to Rust.** The look, the layout
and the translations (the `.slint` strings use `@tr` with upstream
contexts, which the existing `lang/` catalogs already contain) come for
free; the work is the backend.

What upstream still does with Qt Widgets: 72 `.ui` dialogs (property
dialogs, board setup, output jobs, BOM review, ...) and the 3D view. Those
need new Slint dialogs (M3, batched per area) and a new 3D renderer (M5).

## Decisions

1. **UI sources:** copy `libs/librepcb/ui/**` at the pinned upstream
   revision into `crates/librepcb-app/ui/` as ported source (GPL like the
   rest), with a `PROVENANCE.md` naming the revision. Changes are allowed
   (they are our code from then on); keep a diff-friendly layout so later
   upstream UI changes can be merged by hand. Alternative rejected:
   compiling directly from the upstream checkout (the app would depend on
   an external checkout at build time and could not diverge).
2. **Crate layout:**
   - `crates/librepcb-app` (bin `librepcb`): the backend (`app.rs`:
     application and main window, `section.rs`, `tabs/`: home, schematic,
     board 2D, library tabs later), models, panels, and glue to the canvas.
     The `.slint` files in `crates/librepcb-app/ui/` are compiled by the
     separate crate `librepcb-app-ui` (`ui/Cargo.toml`, `build.rs` with
     `slint-build`): the generated code is tens of MB of Rust, so keeping
     it apart means backend changes do not recompile it.
   - Scene building stays in `librepcb-scene` (already used by render and
     graphics export); it gains **incremental updates**: apply
     `project::Change` events to a built scene (update/insert/remove the
     items of one symbol, segment, device, ...) instead of rebuilding.
   - Interaction logic (editor state machines: select, draw wire, add
     component, draw trace, ...) is UI-toolkit independent and goes into
     `librepcb-editor` (`fsm/` module, ported from
     `editor/project/{schematic,board}/fsm`) so it can be tested headless.
     The app only translates Slint events to FSM events and FSM outputs
     (overlays, cursor, tool bar data) to `Data`.
3. **State and threading:** one UI thread owns the open projects as
   `librepcb_editor::SharedProject` (`Arc<parking_lot::Mutex<OpenProject>>`,
   an `OpenProject` holds the `ProjectEditor` and the directory lock); the
   embedded MCP server gets the same handles via `McpState::set_project()`. Long jobs
   (DRC, plane fragments, exports, library scan, Freerouting) run on worker
   threads on snapshots (as in core) and post results back with
   `slint::invoke_from_event_loop`. After each command the tab pulls
   `Project::changes_since(cursor)` and updates its scene (or rebuilds on
   `Resync`).
4. **Embedded MCP ("watch the agent"):** the app can start the MCP server
   (`librepcb-mcp` library, streamable HTTP on localhost, on a tokio
   runtime in a background thread) sharing the same `ProjectEditor`
   mutex; every MCP write bumps the revision and wakes the UI, which
   applies the journal to the visible scenes. Agent edits appear live and
   are in the same undo history ("AI: ..." groups). This is the headline
   feature of M2 and needs only a small refactor of `librepcb-mcp`'s
   session to accept an externally owned editor.
5. **Rendering:** vello_cpu into a `SharedPixelBuffer` per canvas (works
   with every Slint renderer, incl. software); GPU vello later. Upstream's
   `frame` counter property triggers repaints.
6. **Testing:** backend logic unit-tested headless; UI tests with Slint's
   testing backend (`slint::testing` / `i-slint-backend-testing`) driving
   the real component tree; screenshot tests with the software renderer
   (`take_snapshot`) compared loosely (hashes are not stable across
   versions). Screenshots are also how agents without a display check
   their work.

## Milestones

**M2 — viewer** (this wave):
- M2a skeleton: crate, copied `.slint` files compiling with slint-build,
  `Data`/`Backend` bound in Rust, main window, home tab (recent projects,
  open/new project), window sections and tabs, notifications, about
  panel, themes, translations via `librepcb-i18n`.
- M2b project viewing: open a project (read-only first), documents panel,
  schematic tab and board 2D tab rendering through `librepcb-scene`,
  navigation (pan/zoom/fit like upstream), grid, layers panel with
  visibility, selection highlight and hover info (read-only select tool).
- M2c live updates (**done**): incremental scene updates from the change
  journal; embedded MCP server (`crates/librepcb-app/src/mcp.rs`) started
  with `--mcp[=ADDR]` or the "AI" button in the status bar (popup: enable
  the server, "show what the agent edits"; the status bar is where
  upstream shows global non-modal state); change notices update the
  scenes immediately (250 ms polling kept as fallback), the UI follows
  the agent's focus to the touched schematic/board, agent
  `project_open`/`project_close` open/close tabs (delegated),
  `project_create` is adopted; the agent works on the project of the
  active tab; undo/redo/save buttons work for agent and UI edits; status
  bar indicator and a notification when an agent edits. Test:
  `tests/app/live_mcp.rs` (rmcp HTTP client against the headless app).
- M2d checks and outputs (**mostly done**): ERC (automatic after
  changes) and DRC (full or quick, worker thread with progress
  notification) in upstream's rule check panel: list, approve/unapprove
  (undoable mutations), zoom to location (no location marker yet, no
  autofix). Menu actions without dialogs (default settings, output into
  `<project>/output/<version>/`): PDF export (schematics, board assembly),
  Gerber/Excellon, pick&place, IPC-D-356A netlist, BOM, `*.lppz` export,
  "Output Jobs" runs all jobs; worker threads with progress and result
  notifications. Missing: print, image export, opening `*.lppz`.

**M3 — schematic and board editors** (the FSMs are already ported in
`librepcb-editor::fsm::{schematic, board}` and tested headless; M3 wires
them into the app):
- M3a editing in the tabs: `SchematicView`/`BoardView` adapters on the
  scenes, Slint pointer/key events → FSM, FSM outputs → `Data` (tool bar
  `tool-*` fields, cursor, overlays: rubber band, previews, ruler, info
  box, status bar), tool bars from upstream `.slint`
  (`drawwiretoolbar`, `addcomponenttoolbar`, `drawtracetoolbar`,
  `addviatoolbar`, `drawplanetoolbar`, ...), context menus, undo/redo,
  cut/copy/paste through the system clipboard (upstream MIME types, so
  copy/paste works between our app and upstream LibrePCB), save,
  cross-probing between schematic and board (highlighted nets, selection).
- M3b project dialogs as Slint dialogs (**done** except the order PCB and
  place devices panels, which are already `.slint` and not wired yet, and
  the circle properties dialog of the library editors): add component
  dialog (library search, category tree, symbol and footprint preview,
  symbol variant and device), symbol instance / symbol text / device
  instance / net label / bus label properties, via / pad / plane /
  polygon / zone / hole / stroke text / text properties, line width,
  move-align, board setup, project setup, BOM review, pick & place
  generator, graphics export dialog, output jobs dialog (all job types).
  Design: a dialog describes its fields as a `Form` (rows of
  `FormField`: labels, text, length/angle/ratio edits, check boxes,
  combo boxes, radio buttons, lists and tables, attribute lists,
  buttons; pages and a side column) with the labels of the upstream
  `.ui` file (translated with upstream's Qt context, e.g.
  `librepcb::editor::BoardSetupDialog`); the generic
  `ui/dialogs/formdialog.slint` renders it with upstream's widgets and
  writes edited rows back (like the other UI models); the dialog reacts
  to field events and applies its values as one undo group
  (`FormDialog::apply()`), or returns a value for the tab (line width,
  move/align positions) or output jobs to run in a worker thread. The
  dialogs are tested without a window by editing their forms.
- M3c project lifecycle (**done**, `src/app/lifecycle.rs` and the dialogs
  `new_project`, `open_prompts`, `project_library_updater`,
  `workspace_settings`, `initialize_workspace` of `src/dialogs/`): new
  project wizard (metadata, license, path, schematic/board
  initialization with the stroke fonts and upstream's templates, EAGLE
  project import page; also from a folder of the home tab), directory
  lock dialog (upstream wording, "Open anyway" with "I accept the risk.",
  plus "Open Read-Only"), autosave (workspace interval, `.autosave/` of the
  transactional file system) and the restore question after a crash, the
  file format upgrade notification with the migration log, project
  library tab ("Library Manager": versions, downgrades, copy to a local
  library) with the project library updater, workspace settings dialog
  (all tabs: general, appearance with color schemes and the color scheme
  editor overlay from `colorschemedialog.slint`, library, external
  applications, keyboard shortcuts with overrides honored by
  `Backend.is-shortcut`, internet access), initialize workspace wizard
  (first start before the main window content exists, via
  `src/modal_dialog.rs`; "Switch Workspace"). Wizards are form dialogs
  whose extra buttons are "Back"/"Next" and whose OK button is "Finish";
  dialogs can request application actions (`AppRequest`). Tests:
  `tests/app/{lifecycle,workspace_settings,workspace_wizard}.rs`
  (screenshots `lifecycle_*.png`, `settings_*.png`,
  `workspace_wizard_*.png`). Gaps: see COMPAT.md.
- M3d the missing FSM parts: buses, images, standalone board pads, DXF
  import, "find", segment simplification after edits (see COMPAT.md
  Part 1, "Schematic editor FSM" / "Board editor FSM"). **Done**: in
  `librepcb-editor` and in the tabs (bus/bus label and image tools,
  bus member menu, board THT/SMT pad tools with their tool bar, DXF
  import dialog, "find" field with suggestions and zoom, plane visibility
  from the context menu, board cross-probing; see COMPAT.md "M3d tools in
  the tabs" under app in Parts 1 and 2). Test: `tests/app/m3d_tools.rs`.

**M4 — library editors and library management** (the `.slint` tabs exist
upstream: `library/{lib,cat,sym,pkg,cmp,dev,org}`, library tree, create
and download library tabs, libraries panel):
- M4a library management UI (**done**): libraries panel (installed and
  online libraries from `librepcb-network`, install/update/remove with
  progress), download library tab, create library tab, library tab
  (metadata, dependencies, element tree per category, library checks
  with approvals and autofixes, save with undo). Test:
  `tests/app/library_management.rs`.
- M4b element editors: symbol editor (FSM port of upstream
  `library/sym/fsm`: pins, lines, polygons, circles, arcs, texts, names/
  values; pin properties dialog), package editor (`library/pkg/fsm`:
  footprints, pads, holes, zones, stroke texts, pad properties dialog,
  footprint tag/transform panels, 3D model assignment), component editor
  (signals, symbol variants, gates, pin-signal mapping), device editor
  (pinout, parts), category and organization tabs; chooser dialogs
  (category, component, package, symbol); element checks with fixes.
  **Done** (`crates/librepcb-app/src/tabs/{symbol,package,component,
  device,category,organization}.rs` on `element_core.rs` (the
  `LibraryElementEditor` of `librepcb-editor`: undo stack, dirty state,
  interface check, metadata, checks, wizard pages, saving into the
  library) and `base_element_core.rs` (categories and organizations: a
  snapshot undo stack over `BaseMetadata` and the kind specific
  content)): symbol and package canvases with the library FSMs, tool
  bars, context menus and the item properties dialogs (pin, pad,
  polygon, circle, text, stroke text, hole, zone), package pads,
  footprints (tags, 3D transform, model flags) and 3D models (STEP file
  chooser), courtyard and move/align dialogs; component signals,
  variants, gates and pinouts with rendered symbol previews; device
  component/package choosers with previews, pinout (list, reset,
  automatic, CSV file, interactive) and parts; category parent chooser;
  organization logo, URL, priority and PCB design rules list; the
  symbol/component/package chooser is one searchable list dialog
  (`dialogs/chooser.rs`). The element list rows written by the UI go
  through per-tab "row sinks" (deferred to the application state like
  the other UI models). Test: `tests/app/library_elements.rs` (one
  headless test per tab kind, screenshots `element_*.png`). Gaps: see
  COMPAT.md Part 2, app, "Library element editors".
- M4c library import (**done**, `src/dialogs/library_import.rs`,
  `src/app/library_import_host.rs`): EAGLE and KiCad library import
  wizards on `librepcb-import` (start, choose library, select elements
  with dependency states, options, result) from the library tab's import
  page and the library menu; the import runs in a worker thread with
  progress and cancellation, followed by a library rescan. Test:
  `tests/app/library_import.rs` (screenshots `library_import_*.png`).

**M5 — the rest:** 3D board view and STEP export (needs an OpenCascade
replacement or a mesh-based approach), PCB ordering API integration,
printing, keyboard shortcut editor polish (recording key presses),
accessibility pass.

## Risks

- Upstream `.slint` files target a specific Slint version; pin the same
  Slint release (check upstream's `find_package(Slint)` version) and
  upgrade deliberately.
- Some upstream `.slint` code calls C++-only helpers through `Backend`
  pure callbacks (length/angle parsing, shortcuts, ...); each needs a Rust
  implementation — mechanical, but many.
- No display in the cloud sandbox: development relies on the testing
  backend and software-rendered snapshots; interactive checks happen on a
  desktop.

## Debugging the UI with agents

Two ways to look at and drive the running UI without a display.

**Headless screenshots** (no extra features): the app renders its main
window with Slint's software renderer into a PNG and exits.

```sh
cargo run -p librepcb-app -- --workspace /tmp/ws --project X.lpp \
    --tab board --panel layers --screenshot out.png --size 1400x900
```

`--tab home|schematic|board` and
`--panel home|libraries|documents|layers|about|none`
choose what is shown. The test `crates/librepcb-app/tests/app/screenshot.rs`
does the same with an upstream test project (copied to a temporary
directory: opening a project locks its directory) and writes
`app_home.png`, `app_board.png` and `app_schematic.png` to
`$CARGO_TARGET_TMPDIR`.

**Slint's embedded MCP server** (Slint 1.18, feature `mcp`): lets an agent
inspect the element tree, take screenshots and send clicks, drags, wheel
and key events to the live application.

```sh
SLINT_EMIT_DEBUG_INFO=1 SLINT_MCP_PORT=8765 SLINT_BACKEND=headless \
    cargo run -p librepcb-app --features ui-debug-mcp -- \
    --workspace /tmp/ws --project X.lpp &
```

- `ui-debug-mcp` is an opt-in crate feature (`= ["slint/mcp"]`); never
  enable `mcp` by default.
- `SLINT_EMIT_DEBUG_INFO=1` is read by `slint-build` at build time (it
  embeds element type names and ids; without it the element tree is empty).
  Changing it rebuilds the `librepcb-app-ui` crate.
- `SLINT_BACKEND=headless` is a windowless software-rendered backend.
- Ports: 8765 for Slint's UI server, 8766 for LibrePCB's own MCP server
  (`--mcp`). With both, an agent edits the design through 8766 and
  watches (screenshots, clicks) through 8765.

The server speaks JSON-RPC over HTTP at `http://127.0.0.1:8765/mcp`; its
`initialize` result contains usage instructions. A session that zooms,
selects and pans in the schematic tab:

```sh
mcp() { curl -s -X POST http://127.0.0.1:8765/mcp -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}"; }
W='"windowHandle":{"index":"1","generation":"1"}'
mcp list_windows '{}'
mcp find_elements_by_id "{$W,\"elementsId\":\"SchematicTab::ta\"}"   # the scene's TouchArea
mcp dispatch_pointer_scroll "{$W,\"position\":{\"x\":420,\"y\":388},\"deltaX\":0,\"deltaY\":120}"
mcp click_element '{"elementHandle":{"index":"2","generation":"1"}}'
mcp drag_element '{"elementHandle":{"index":"2","generation":"1"},"button":"Middle","target":{"x":500,"y":300}}'
mcp take_screenshot "{$W}"   # result: base64 PNG in content[0].data
```

Useful element ids: `SchematicTab::ta` and `Board2dTab::ta` (scene touch
areas), `AppWindow::menubar`, `AppWindow::sidebar`. Everything else is found
with `get_element_tree` / `query_element_descendants` (accessible labels of
upstream's widgets).

Dialogs (M3b) are overlays of the main window: the editors of a form
dialog have the field id as accessible label (e.g. `author` in the
project setup dialog), the page tabs are `PageButton`s and the buttons
are found by their text ("OK", "Apply", "Cancel"). Example: click
`ProjectSection::setup-dialog-btn`, find the `author` text input, click
it, `dispatch_key_event` some text, click "OK": the undo button's label
becomes "Undo: Modify Project Setup".
