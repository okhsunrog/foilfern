# Status and handoff

State of the port as of 2026-11-10, for continuing the work in a new
session (for example a cloud session). Read `AGENTS.md` first; this file
covers where things stand and how the work has been organized.

## Where we are

The project editing, MCP, workspace/library, import and application work is
merged with the physical board renderer and pure-Rust STEP backend. CLI,
standalone MCP and desktop jobs share the same graphics and CAD exporters.
The board's interactive Bevy 3D view is connected to Slint. Package-editor 3D
previews and the coverage gaps in `COMPAT.md` Part 2 remain.

The application is checked headless (Slint software renderer, screenshots,
embedded HTTP MCP); desktop platform integration (Wayland/X11, clipboard,
native file dialogs, DPI and Slint GPU rendering) remains a separate manual pass.
Bevy's offscreen GPU rendering is checked separately from Slint presentation.

Done, in `crates/librepcb-core` unless noted:

- **Base:** types, the S-expression format, the serialization traits, and utils.
  Upstream Qt semantics are dropped, with the divergences listed in `COMPAT.md`
  (Part 1).
- **Geometry, stroke fonts, polygon clipping:** `crates/clipper` is a
  faithful port of Clipper 1, verified differentially against the C++
  library.
- **Attributes, air wires** (literal port incl. upstream's Delaunay
  library, same tie-breaking as upstream), the math parser (evalexpr),
  and SQLite.
- **File I/O** (transactional file system, directory locks, ZIP, CSV) and
  system info; `crates/librepcb-network` (tokio + reqwest) incl. the
  library download/installer (api.librepcb.org).
- **Workspace** (`workspace/`): workspace open/create, settings, the
  library index (SQLite, schema identical to upstream `cache_v8.sqlite`),
  scanner and search facade.
- **Libraries:** all element types and their checks, identical to upstream
  `librepcb-cli --check`.
- **Project model** (`project/`): circuit, schematics and boards with
  mutations, inverses, the reverse index, the change journal and serde.
  All test projects round-trip byte for byte.
- **Checks:** ERC and DRC; messages and approvals match `librepcb-cli`
  on all upstream test projects (DRC: 37/37 boards).
- **Exports:** plane fragments, Gerber/Excellon, pick & place, Gerber X3,
  IPC-D-356A, BOM, project JSON, interactive HTML BOM (`interactive-html-bom`
  crate like upstream); about 1000 files compared byte for byte with
  `librepcb-cli`. Output jobs (`job`) and the output job runner.
- **File format migrations** v0.1 → v1 → v2, byte-identical incl. `jobs.lp`.
- **`crates/librepcb-cli`:** native CLI including standalone STEP loading,
  CPU tessellation, document minification and library `--minify-step`.
  Mechanical STEP, realistic graphics, interactive HTML BOM and manufacturing
  output jobs share core output ownership and archives.
- **`crates/librepcb-editor`:** undo stack and intent-level commands
  (components, wiring with forced net names, devices, traces, vias,
  planes, outline, autorouting) used by MCP and later the UI. The
  schematic and board editor FSMs (`fsm::schematic`, `fsm::board`) cover
  all upstream tools and actions (M3d done in the editor crate: buses,
  images, polygon vertex editing, standalone board pads, DXF import,
  "find", change device, segment simplification after edits, the tool
  bar's value attribute, pasting graphics from the library editors,
  cross-probing outputs; headless scenarios in
  `tests/editor/{fsm_schematic,board_fsm}_test.rs`); the application tabs
  expose them (M3d wiring done). M4b
  groundwork: `library_editor` (element editor with snapshot undo stack,
  dirty state, interface check, save; commands for symbols, packages,
  components and devices; check fixes) and `fsm::library` (symbol and
  package editor FSMs with all upstream tools),
  headless scenarios in `tests/editor/library_editor_test.rs`; the Slint
  element tabs are wired (see `librepcb-app` below).
- **`crates/librepcb-import`:** EAGLE import (parseagle model, type and
  library converters, library import, project import) and KiCad library
  import (`*.kicad_sym`, `*.kicad_mod`/`*.pretty`, 3D models), UI
  independent: element selection with dependency states, wizard options,
  `Progress` with cancellation and a `MessageLogger` (core
  `utils::message_logger`). The EAGLE test project is imported byte for
  byte like upstream's golden sample; imported libraries pass
  `librepcb-cli open-library --all --strict`. MCP: `library_import`,
  `project_create` with `eagle_schematic`. The app's import wizards (M4c)
  and the new project wizard's EAGLE page use it.
- **`crates/librepcb-autoroute`:** built-in grid A* router with rip-up and
  exact clearance verification.
- **`crates/librepcb-scene`:** scene builders for schematics, boards,
  symbols and footprints on `crates/librepcb-canvas`, headless PNG
  rendering, and the graphics export (PDF via `pdf-writer`, SVG,
  PNG/JPEG/BMP; upstream's page layout and realistic board rendering).
  Schematic and board scenes are updated incrementally from the change
  journal (`apply_changes()`, `SceneSync`), checked against fresh builds
  after editor command sequences and seeded random edits.
- **`crates/librepcb-mcp`:** MCP server (rmcp, stdio/HTTP), see
  `mcp-design.md`.
- **`crates/librepcb-i18n`, `tools/ts2po`, `lang/`:** translations from the
  upstream catalogs.
- **`crates/librepcb-app`** (binary `librepcb`, M2a + most of M2b):
  upstream's Slint UI (`ui/`, compiled by the `librepcb-app-ui` crate with
  Slint master `aebe3dd` / edition 1.20, see `ui/PROVENANCE.md`) with a Rust backend: main window,
  sections and tabs, home tab (folder tree, recent/favorite projects),
  themes, about panel, notifications, translations, documents panel,
  schematic and board 2D tabs rendered through `librepcb-scene` and
  `librepcb-canvas` (pan/zoom like upstream, grid, layers panel, read-only
  select/hover), headless screenshots (`--screenshot`) and Slint's MCP
  server for agents (feature `ui-debug-mcp`, see `docs/ui-design.md`).
  M2c: embedded LibrePCB MCP server (`--mcp`, status bar toggle) editing
  the open project live with a shared undo stack, the UI following the
  agent; M2d: ERC/DRC rule check panel (automatic ERC, background DRC,
  approvals, zoom to location) and exports/output jobs from the menus.
  M3a (editing in the tabs) is in place: the schematic and board tabs
  drive the `librepcb-editor` FSMs (tools, tool bars, overlays, context
  menus, clipboard with upstream MIME types, cross-probing, undo/redo;
  `crates/librepcb-app/tests/app/editing.rs`). M3b (project dialogs) is
  done: generic Slint form dialogs (`src/dialogs/`,
  `ui/dialogs/formdialog.slint`) for the properties of all schematic and
  board items the FSMs request (symbol, symbol text, net/bus label
  rename, polygon, text, device, via, pad, plane, board polygon, stroke
  text, hole, zone), line width, move/align, board setup, project setup,
  graphics export, output jobs (all job types), BOM review and the
  pick&place generator, and the add component dialog
  (`ui/dialogs/addcomponentdialog.slint`: categories, search, symbol and
  footprint previews, symbol variant, device); each applies one undo
  group. Tests: `tests/app/{dialogs,setup_dialogs,output_dialogs}.rs`
  (headless) and `tests/app/dialog_screenshots.rs` (the dialogs in the
  headless application, screenshots in `$CARGO_TARGET_TMPDIR`).
  M3d in the tabs: bus and image tools, board pad tools, DXF import,
  "find", plane visibility (`tests/app/m3d_tools.rs`). M4a (library
  management) and M4b (element editors) are done: libraries panel with
  online libraries and installer, create/download library tabs, library
  tab, symbol, package, component, device, category and organization
  tabs with the library FSMs, list panels, properties and chooser
  dialogs, checks with approvals and autofixes, undo/redo, wizard mode
  for new elements and saving into the library
  (`tests/app/{library_management,library_elements}.rs`); see
  `docs/ui-design.md` (M4) and COMPAT.md Part 2, app, "Library management" /
  "Library element editors" for the gaps.
  M3c (project lifecycle) is done: new project wizard (incl. EAGLE project
  import), directory lock and autosave restore prompts, autosave, file
  format upgrade notification, project library tab and updater, workspace
  settings dialog (all tabs, keyboard shortcut overrides, color schemes
  applied to the scenes), initialize workspace wizard
  (`tests/app/{lifecycle,workspace_settings,workspace_wizard}.rs`).
  M4c (EAGLE/KiCad library import wizards on `librepcb-import`) is done
  (`tests/app/library_import.rs`).
  Also done: rule check runs remove approvals of disappeared messages
  (`tests/app/rule_checks.rs`); images, DXF import and "Paste Geometry" in
  the symbol and package tabs (`tests/app/element_graphics.rs`); automatic
  and manual ("Rebuild All Planes") plane rebuilds in worker threads
  (`tests/app/planes.rs`); opening `*.lppz` archives read-only; all
  `tr!` contexts of the app are checked by `librepcb-i18n`'s
  `context_check` (app-only strings in its `NEW_STRINGS`).

## Interactive 3D board viewer

- `librepcb-render3d` builds physical meshes from `BoardPhysicalGeometry`,
  including through/blind/buried drills, copper, mask, silkscreen and paste.
  `librepcb-step` supplies directly tessellated component bodies with their
  existing placement, normals and surface colors. CAD/GPU work runs on owned
  snapshots in the viewer worker, outside the project lock.
- The Slint board's `3D` button opens the existing 3D tab. Orbit, pan, zoom,
  top/bottom, fit (including component bounds), material opacity and return to
  `2D` are connected. Project edits and shared undo/redo refresh the snapshot.
- Bevy renders offscreen with PBR materials, lights and tone mapping. Readback
  into Slint images works with the software UI renderer; shared Slint/Bevy GPU
  textures are still an optimization to implement.
- GPU acceptance ran without a window server on Intel Arc (MTL), Mesa Vulkan.
  The GPU test checks rendering, camera-only updates, both sides and unaligned
  viewport resizing. The application GPU scenario checks navigation, material
  sliders, a command-bus color edit, undo and closing/returning to 2D.
- The actual application was driven through Slint's `ui-debug-mcp` server:
  CAN–USB orbit/zoom, bottom view and return to 2D, with all 40 recorded pointer
  and wheel events accepted. CAN–USB has 44,031 render triangles in 115
  material groups. Dark/light application screenshots were inspected.
- Run `librepcb --project /path/to/project.lppz --tab board3d`; add
  `--screenshot /path/to/view.png` for an offscreen GPU/UI capture. Diagnostic
  artifacts and logs are in
  `/home/okhsunrog/tmp_zfs/codex_tmp/librepcb-bevy-3d/`.

Validation: all workspace test targets pass across the application run and the
separate remaining-workspace run: 1,503 passed, 10 ignored by default. Both
GPU tests also pass when explicitly enabled on the GPU host. The initial overall
shell was interrupted by SIGTERM; the separate Cargo run completed with status
0. Workspace Clippy with warnings denied, formatting, the application build
with `ui-debug-mcp`, and the headless CAN–USB screenshot command pass.

## Merge integration

- The CLI, app and MCP install `librepcb-step::board_exporter()`; `jobs_run`
  executes STEP and graphics instead of skipping them as unsupported.
- `librepcb-graphics::ProjectGraphicsExporter` prepares encoded outputs; core
  checks all page filenames before writing, retaining indexed ownership,
  collision protection, archive dependencies and obsolete-file cleanup.
- The existing MCP stdio/HTTP, shared editing/undo and application suites
  cover the combined implementation. A stdio regression exports and reads
  back STEP and checks the realistic-board PDF.
- The CAN–USB assembly was regenerated through the merged CLI: independent
  OCCT/XCAF checks retain 87 valid solids and all 2,048 face appearances.
  Realistic top/bottom PNGs were regenerated and inspected as well.
- Slint dependencies, build compiler and canvas adapter use the same master
  revision `aebe3dd580ce0594de272bc5797b7444a2aa18fd` (1.20.0 development snapshot).

Validation before the 3D viewer changes: 1,499 Rust tests pass across the complete
workspace run and the focused graphics/i18n reruns after the two merge fixes;
eight tests are ignored. All 53 headless application scenarios and the new
stdio STEP/PDF regression pass. The unmodified upstream CLI suite reports
177 passed / 1 failed; the remaining snapshot expects Qt/OpenCascade version
lines, intentionally different in this Rust implementation.
Formatting, workspace Clippy with warnings denied, and production builds of
the CLI, MCP server and application pass.

Artifacts and logs: `/home/okhsunrog/tmp_zfs/codex_tmp/librepcb-mcp-integration/`.

## Next steps

Historical MCP branch acceptance (before this merge): a stdio session designed a
two-transistor LED multivibrator from the official libraries — library
install, part search, schematic with `connect`/`schematic_tidy`, board
outline, connectivity-based placement, Freerouting, GND plane, ERC/DRC,
Gerber export — and the official `librepcb-cli` reports ERC 0, DRC 0.

1. M3c/M4c polish (see COMPAT.md "app"): scene highlight colors and the
   library editors from the color schemes, recording key presses in the
   shortcut editor, remembering wizard paths, the example projects
   download.
2. M2 viewer: finish M2b (library info on the home panel, grid/unit and
   layer visibility persisted in the user settings, keyboard handling) and
   the rest of M2d (rule check location markers and autofixes, print and
   image export); M2c is done.
3. MCP: stdio MCP mode for Freerouting, subcircuit templates, design
   intent file (see `mcp-research-konnect.md`), and larger real-library
   designs with multi-gate ICs and routed inner copper layers. The stdio
   suite now covers explicit symbol-variant selection, individual gate
   movement/undo, cross-page connections and four-layer fabrication output
   using upstream fixtures; upstream CLI validation runs when available.
4. Extend the 3D viewer to package editors, replace GPU readback with shared
   Slint/Bevy textures, and broaden STEP representation support. Mechanical
   STEP is already implemented by `librepcb-step` (Monstertruck, no C++ FFI).

## How the work is organized

- Work goes in **waves** of parallel agents, each in its own git worktree
  (`../librepcb-rs.w<N>-<name>`, branch `wave<N>/<name>`) with a clearly
  separated file ownership. The coordinating session reviews each branch
  (fmt, clippy, tests, no hardcoded paths), merges it into `master` and
  pushes.
- Related small tasks are batched per agent (about 5–10 agents per wave),
  rather than one agent per tiny task.
- Architecture and design work (design documents) runs on the strongest
  available model, and the design is reviewed with the user before
  implementation starts.
- Acceptance is always mechanical where possible: upstream unit test
  vectors, byte-identical round trips over upstream `tests/data`, and diffs
  against the official `librepcb-cli`.
- Each intentional divergence from upstream goes into `COMPAT.md` Part 1;
  missing features still to be matched go into Part 2.

## Environment

Run `scripts/cloud-setup.sh` on a fresh machine. It sets up the pinned
upstream checkout next to the repository and the official `librepcb-cli`
behind Xvfb, and Freerouting 2.4.1 with Java 25; after that,
`cargo test --workspace` runs everything, including the upstream
comparisons. Set `USER` if the environment lacks it (a ported system info
test needs it), and `LIBREPCB_TEST_LIBRARIES_DIR` to a directory with
official `*.lplib` clones to run the real-library design tests. With
several parallel agents, use `CARGO_INCREMENTAL=0` and shared target
directories: the cloud disk allowance fills up quickly.

To keep the target directory small, the dev profile builds workspace crates
with line tables only (backtraces keep file and line) and dependencies
without debug info, and each crate has one integration test binary (see
AGENTS.md). For full debug info in a local debugging session, build with
`CARGO_PROFILE_DEV_DEBUG=full` (workspace crates), plus
`--config 'profile.dev.package."*".debug="full"'` for the dependencies.
Measured with `cargo clippy --workspace --all-targets` followed by
`cargo test --workspace` in a fresh target directory: 25 GB before (49
executables, 20 GB), 16 GB with one test binary per crate, 5.7 GB with the
debug info settings too (26 executables, 2.7 GB). `split-debuginfo =
"unpacked"` saved only another 0.4 GB and is not supported on all
platforms, so it is not set.

## Open items to remember

- Adding and removing project library elements are `Project` methods, not
  `Mutation`s; `librepcb-editor`'s undo stack makes them undoable.
- `@tr()` strings of our own `.slint` additions are not checked
  automatically (only `tr!`/`trn!` in Rust are, by `context_check`); new
  `.slint` strings should use `@tr("upstream::Context" => "text")` where an
  upstream string exists.
- Direct ZIP downloads from codeload.github.com are blocked in the cloud
  sandbox; official libraries can be cloned with git instead.
- Upstream bugs found along the way, which could be reported upstream:
  Qt/Transifex plural form order mismatches in several languages, `"1e-3"`
  rejected as a length, and a v0.1 circular board outline that cannot be
  migrated.
