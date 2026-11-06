# Roadmap

Goal: a Rust + Slint LibrePCB that is file-compatible with upstream and lets
AI agents design simple boards through MCP. Milestones are ordered to reach
something working, and usable by AI agents, as early as possible.

## M1 — headless `librepcb-cli`

Open real upstream projects and libraries, run ERC/DRC, export
manufacturing data. Validated with upstream `tests/cli` (pytest) and by
diffing outputs against the official `librepcb-cli`.

- Wave 3b (complete): schematic items, board items, file format migrations.
- Wave 3c (complete): board export data and plane fragments, DRC, ERC + BOM + project
  attribute lookup.
- Wave 4 (in progress): output job model and manufacturing runner complete;
  native CLI core available (115 upstream cases pass). Full acceptance still
  needs rendering/STEP command coverage and parser/help/version expectations.

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

Read-only Slint app: schematic and board tabs on `librepcb-canvas`, layers
panel, live updates from the change journal (watch an AI agent work).
Schematic PDF export.

## M3 — editing

Selection, move/rotate/delete, wires and traces, placing components,
property dialogs (batched per area), save.

## M4 — library editor

Symbol, package, component and device editors.

## M5 — the rest

3D view and STEP, interactive HTML BOM, PCB ordering API, Eagle and KiCad
importers.
