# Roadmap

Goal: a Rust + Slint LibrePCB that is file-compatible with upstream and lets
AI agents design simple boards through MCP. Milestones are ordered to reach
something working, and usable by AI agents, as early as possible.

## M1 — headless `librepcb-cli`

Open real upstream projects and libraries, run ERC/DRC, export
manufacturing data. Validated with upstream `tests/cli` (pytest) and by
diffing outputs against the official `librepcb-cli`.

- Wave 3b: schematic items, board items, file format migrations.
- Wave 3c: board export data and plane fragments, DRC, ERC + BOM + project
  attribute lookup.
- Wave 4: output jobs and the CLI.

## M1.5 — AI track (MCP)

- MCP server, read-only: open a project, query circuit/netlist, components,
  schematic and board contents, run ERC/DRC, export outputs.
- Command bus with undo (designed together with the user), then MCP writes
  through `Mutation`. This also covers undoable project-library operations
  (adding a component from a library).
- Workspace, library index (SQLite) and library manager (download the
  official libraries through `librepcb-network`), so agents can find and
  place real parts.
- Routing: Specctra DSN export and SES import, for autorouting with
  FreeRouting.

## M2 — viewer

Slint app on upstream's `.slint` UI (see `ui-design.md`): home tab,
schematic and board tabs on `librepcb-scene`, layers panel, navigation,
live updates from the change journal with the embedded MCP server (watch
an AI agent work), ERC/DRC panels, graphics export and output jobs.

## M3 — schematic and board editors

Editing in the tabs through the ported FSMs (select, move, rotate, flip,
delete, wires, components, labels, traces, vias, planes, polygons, zones,
holes, texts), clipboard compatible with upstream, cross-probing, all
project dialogs as Slint dialogs (properties, board/project setup, BOM
review, output jobs, graphics export, new project wizard), workspace
settings.

## M4 — library editors

Library management (download, create, libraries panel), symbol, package,
component, device, category and organization editors with their FSMs and
dialogs, library checks with fixes, Eagle/KiCad library import.

## M5 — the rest

The board's interactive 3D view uses Bevy on the shared physical geometry and
pure-Rust STEP component meshes. Camera navigation, material visibility and
updates from project edits are wired to the Slint board tab. GPU frames are
currently read back into Slint images; shared GPU textures remain an optimization.

Remaining: package-editor 3D previews, broader STEP representation coverage,
Eagle/KiCad project import, PCB ordering API and printing. Mechanical STEP jobs
and standalone STEP/minification tools are already available through the
pure-Rust backend.
