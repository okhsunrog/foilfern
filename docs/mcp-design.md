# MCP server design

Design of `crates/librepcb-mcp` (milestone M1.5), building on the decisions
in `mcp-research-konnect.md`. Goal: an AI agent designs a simple board end to
end through MCP alone — find parts, draw the schematic, place and route the
board, run ERC/DRC, export manufacturing data — and the result opens in
upstream LibrePCB.

The server advertises the bundled Markdown resource
`librepcb://guides/board-design` through `resources/list` and `resources/read`
over both transports, including the embedded host. It contains the
[board design workflow](mcp-board-workflow.md), with multi-gate and
multi-page usage, layer-count conventions and error recovery. Initialization
instructions point clients to it. Reading the guide needs no workspace or
project, does not touch session state, and works from an installed binary
without the repository or client-specific configuration files.

## Layers

```
librepcb-mcp      MCP protocol (rmcp, stdio + optional streamable HTTP), tool
                  DTOs (serde + schemars), outcome envelope, session state
librepcb-editor   UndoStack + intent-level commands over Mutation (no MCP types)
librepcb-core     model, Mutation/apply, ERC/DRC, exports, jobs, workspace/library DB
librepcb-network  library download (api.librepcb.org)
```

`librepcb-mcp` owns no domain logic: every tool is a thin adapter that
parses the DTO, calls one editor command or core query, and builds the
response. Logic an agent needs that a UI would also need (auto-placement of
wires, device placement, routing helpers) lives in `librepcb-editor`.

## Process and state

- Binary `librepcb-mcp` (stdio by default, `--http <addr>` for streamable
  HTTP), or embedded in the desktop app (see below). One server holds at
  most one workspace and one project, as shared handles
  (`SessionSlots { workspace: Option<SharedWorkspace>, project:
  Option<SharedProject> }` in `McpState`, shared by all MCP client
  sessions), where `SharedWorkspace = Arc<parking_lot::Mutex<Workspace>>`
  and `SharedProject = Arc<parking_lot::Mutex<OpenProject>>`. Tools run
  their synchronous core work in `tokio::task::spawn_blocking` through
  `McpState::call()`: tool calls are serialized, the workspace and then
  the project are locked into a `Session` (whose fields are the lock
  guards, so tool code reads `session.project()` as before), the tool
  runs, replaced handles (open/create/close) are written back and the
  locks are released before the host is notified. No lock is ever held
  across `.await`.
- `OpenProject { editor: ProjectEditor, file_system (lock), saved_revision,
  upgraded }` lives in the editor crate (`librepcb_editor::OpenProject`,
  `OpenProject::open()` locks and loads), so the app opens projects the
  same way. The `ProjectEditor` (editor crate) owns the project, its
  undo stack and the library element source (the workspace library
  database, shared via `Workspace::shared_library_db()`). Unsaved changes
  are the undo stack's clean state (plus a pending file format upgrade).
  Derived data (plane fragments, air wires) is updated outside the undo
  history through `ProjectEditor::update_derived_data()` (DRC, exports,
  output jobs, `planes_rebuild`). The upstream-compatible directory lock is
  taken on open, so upstream LibrePCB refuses to open the project
  concurrently (and vice versa).
- The workspace is optional: without one, library tools operate on the
  project library only and on library directories passed explicitly.
  `--workspace <dir>` or `workspace_open` selects one; `workspace_create`
  makes a fresh one (used by tests and first-time setup).

## Embedded in the application

The desktop app starts the server in-process ("watch the agent",
`ui-design.md` decision 4); the standalone binary is just another host.

- Library API (`librepcb_mcp::{host, transport}`): the host creates
  `McpState::new(Arc<dyn McpHost>)`, hands over its own handles with
  `set_workspace()` / `set_project()` (e.g. the project of the active
  tab; takes effect with the next tool call) and starts
  `transport::start_http(state, addr, &runtime_handle)` (feature `http`)
  from any thread: it binds synchronously (a busy port fails at once),
  spawns the streamable HTTP service (`http://<addr>/mcp`, loopback
  `Host` headers only) on the host's tokio runtime and returns an
  `HttpServerHandle` (`url()`, `stop()`, `shutdown().await`; dropping it
  stops the server). Port convention: 8766 (`transport::EMBEDDED_PORT`)
  for our server; Slint's UI debugging MCP server uses 8765.
  `transport::serve_stdio(state)` / `serve_http(state, listener, token)`
  are what the binary uses.
- `McpHost` (all methods default to the standalone behavior): before
  `workspace_open`/`workspace_create`, `project_open`, `project_create`
  and `project_close`, the server asks the host on a blocking thread with
  no lock held; `HostDecision::Allow` (the server does it; the host sees
  the new project in the notice and can open a tab for it),
  `Refuse(message)` (tool error kind `refused` with the host's message) or
  `Delegated(value)` (the host did it itself, e.g. opened the project in
  a tab and hands over its `SharedProject`; for close: the session just
  detaches). Closing only drops the session's handle: a project the host
  still holds stays open and locked. Preconditions (no project open,
  unsaved changes) are checked before the host is asked.
- Change notification: after every tool call which changed the
  session's project (revision, undo stack state, clean state; i.e. all
  write tools, undo/redo, save, derived data) or replaced the workspace
  or project, the server calls `McpHost::changed(&ChangeNotice)` (no
  locks held; keep it short, e.g. post to the UI thread) and bumps the
  `tokio::sync::watch<u64>` channel of `McpState::subscribe()`.
  `ChangeNotice { sequence, tool, project, project_replaced,
  workspace_replaced, revision_before, revision, focus }`; `focus` lists
  the schematics and boards touched by the call (from the change
  journal), so the UI can show them. The UI pulls
  `Project::changes_since(cursor)` and repaints; read tools and failed
  writes notify nobody.
- Concurrency: UI and MCP edit the same `ProjectEditor` under the same
  mutex, so they share one undo stack: agent writes are groups
  `"AI: <tool>"`, UI commands have their own texts, and undo/redo from
  either side undoes the latest group whoever made it. Agents pass
  `expected_revision` to detect user edits in between (`stale_revision`).
  An open UI group (interactive command in progress) makes agent writes
  fail with `conflict` instead of mixing into it. Lock order for hosts:
  workspace before project, and never call `McpState` setters while
  holding a lock of the current workspace. Long operations keep the
  snapshot/work/apply split (Freerouting, library download): the locks
  are released while the external work runs, and the apply step checks
  the revision.

## Tool conventions

Output jobs install the same `librepcb-graphics::ProjectGraphicsExporter` and
`librepcb-step::board_exporter()` adapters as the desktop application and CLI.
`jobs_run` executes mechanical STEP and realistic 2D graphics alongside the
manufacturing, interactive BOM and archive jobs. Core owns output filenames,
archives and cleanup; it has no CAD or renderer dependency. The standalone
server's stdio integration test reads its STEP output back through the CAD
backend and checks the realistic board PDF.

- Tools address entities by what the agent sees: designators (`"R1"`),
  pins as `"R1.1"` / `"U1.VCC"` (component designator + signal name, or pad
  name on the board) or a designator alone for single-pin components
  (`"GND1"`), net names, schematic/board names; UUIDs are accepted
  everywhere as an alternative.
- Arguments are strict: every DTO denies unknown fields, and the server
  checks the argument names of each call against the tool's input schema
  before dispatch, so a misspelled argument (e.g. `path` instead of
  `directory`) fails with `invalid_argument` naming the key and listing
  the valid ones instead of being ignored. Deserialization errors (wrong
  types, unknown nested keys) are `invalid_argument` tool errors too.
- Units: millimeters as JSON numbers in tool arguments and responses
  (converted exactly to nanometers with `Length::from_mm`), angles in
  degrees. The `apply_mutation` escape hatch uses the core serde
  representation (integer nanometers).
- Write tools take an optional `expected_revision`; a mismatch fails with
  kind `stale_revision` without changing anything. Every write is one undo
  group labeled `"AI: <tool>"` and returns `revision`, the created/affected
  entities re-read from the model, and the `Change` events.
- Response envelope (`structuredContent` + a short text summary):
  `{ "outcome": "complete"|"partial"|"failed", "revision": n, "result": {...},
  "warnings": [...] }`. Errors map `project::Error` & co. to stable kinds
  (`not_found`, `invalid_argument`, `conflict`, `stale_revision`,
  `no_project`, `no_workspace`, `io`, `network`, `not_available`, `refused`
  (host application), `internal`) plus the upstream message.
- Checks never read as passing when they did not run: `run_drc` returns
  `ran: true` and the message list, or an error.

## Tools (first version, < 60)

Session / project:
`server_info`, `workspace_open`, `workspace_create`, `project_create`,
`project_open`, `project_save`, `project_close`, `project_summary`
(metadata, pages, boards, counts, revision, unsaved changes), `undo`, `redo`,
`history`.

Library:
`library_search` (query, kind: component|device|package|symbol, limit;
upstream whole-string matching first, then — if fewer than `limit` —
elements containing all words of the query in name, keywords,
description or, for devices, component/package name and part numbers:
core facade `LibraryDb::search_all_tokens()`; results carry `match:
whole|all_words`), `library_element` (details: component signals + gates +
symbol variants, device pad→signal map with the pad geometry of the
default footprint + package footprints + pads with position, rotation,
shape, size, corner radius, drill and holes, package footprints),
`library_list` (installed libraries), `library_install` (official remote
libraries by name/uuid, downloads + rescans; if a ZIP download fails, e.g.
blocked by a proxy, and the library list entry has a repository URL and
`git` is available, the repository is cloned shallowly at the commit of
the ZIP URL into a temporary directory, `.git` is removed and the result
renamed into place; the result reports `method: zip|git`),
`library_rescan`, `library_import` (EAGLE `*.lbr` or KiCad libraries
into a local workspace library or a library directory, like upstream's
import wizards: `dry_run` lists the elements with selection and
"already imported" state, `elements` selects some with their
dependencies, options name prefix, "EAGLE/KiCad Import" categories and
additional categories; rescans the index; implementation in
`librepcb-import`). `project_create` imports an EAGLE project with
`eagle_schematic`/`eagle_board` (upstream new project wizard with EAGLE
import; a delegating host gets both paths in `ProjectCreateRequest`).

Circuit & schematic:
`component_add` (component or device uuid, optional designator/value,
symbol variant UUID from `library_element`, schematic page, position,
rotation, mirror; places every gate, stacked
automatically if no position), `component_remove`, `component_update`
(designator, value, attributes, assembly options), `component_list`,
`component_get` (signals with nets, symbols and pin positions, placed
devices and the `selected_device` UUID used for default placement),
`symbol_move`, `connect` (net name + list of pins; creates/merges nets,
draws wires or net labels on the schematic), `disconnect` (pins),
`net_rename`, `net_list` (nets with pins and pads), `netlist`,
`schematic_add`, `schematic_list`, `schematic_get` (all items with
coordinates), `net_class_set`, `schematic_tidy` (re-arrange a page by
connectivity and redraw its nets).

`connect` and `disconnect` accept component signal names or UUIDs and
physical package pad names such as `U1.21`. Signal references take
precedence. Before board placement, physical lookup uses the first
assembly option, matching default `device_place`. The selected device
and package are read from the project library or the open workspace.
Use `library_element` with the `selected_device` UUID to see its pad map.

Board:
`board_add`, `board_get` (outline, devices with pads and positions,
traces, vias, planes, unrouted air wires), `board_set_outline` (rectangle
with optional corner radius, or polygon), `device_place` (designator,
position, rotation, side, optional device/footprint choice),
`device_auto_place` (all unplaced devices inside the outline, placed by
connectivity),
`trace_add` (net or pads, layer, width, list of points; endpoints snap to
pads/vias/junctions at the given coordinates), `trace_update` (batch width
changes by trace UUIDs or net names, preserving UUIDs and connected geometry,
with before/after DRC evidence), `via_add`, `trace_remove`,
`plane_add` (net, layer, outline or whole board, clearance, connect style),
`design_rules_set`, `planes_rebuild`, `autoroute` (built-in router for
simple boards, see below), `unrouted` (air wires).

Checks and outputs:
`erc_run`, `drc_run`, `export_fabrication` (Gerber + Excellon),
`export_bom`, `export_pick_place`, `export_netlist`, `jobs_list`,
`jobs_run`, `render` (PNG of a schematic page or a board side, as MCP image
content; the target defaults to the board when `board` or `side` is
given).

Escape hatch: `mutation_apply` (core `Mutation` JSON, full validation, one
undo group), `mutation_schema`.

## Routing

An agent cannot route by coordinates reliably. Two paths, both behind
`autoroute`:

1. Built-in grid router: the pure algorithm in `librepcb-autoroute` (A* on
   a per-layer grid with via cost, exact clearance checks, rip-up and
   reroute, air wires routed shortest first) and the editor command
   `commands::Autoroute`, which extracts the problem from the board (DRC
   data: pads, copper, holes, keepouts, DRC settings and design rules),
   routes only unrouted air wires (optionally of selected nets and layers)
   and applies ordinary traces/vias through `AddTrace`/`AddVia` in one undo
   group. Enough for 2-layer boards with a few dozen nets.
   `autoroute` backend `builtin`, used by `auto` when FreeRouting is not
   installed or a nets/layers filter is given.
2. FreeRouting through Specctra DSN export / SES import
   (`librepcb_editor::FreeroutingRouter`: export with a manifest, run the
   jar headless as a subprocess with a timeout, strict session import as
   one undo group). For MCP, export under the read lock, run FreeRouting
   without lock and import under the write lock; the manifest rejects the
   session if the project changed meanwhile. `autoroute` backend
   `freerouting`; `auto` (default) uses it when `FreeroutingConfig::detect()`
   finds it and falls back to `builtin` if it fails. `specctra_export` /
   `specctra_import` expose the files for other routers.

## Agent-oriented editor commands

Logic an agent needs beyond upstream's interactive editing lives in the
editor crate (`commands::wiring`, `commands::placement`), usable by a UI
too:

- `ConnectNet` (tool `connect`): connects component signals to one net
  (forced net names of supply symbols win, other nets are merged; supply
  symbols whose signal is unconnected and whose forced net name equals the
  net name are attached too and reported) and draws a readable schematic:
  unwired supply symbols are moved in front of the nearest pin, pins on a
  page are joined by a minimum spanning tree of wires (up to 25.4 mm, one
  corner, only routes which do not cross symbol bodies or texts, pins,
  junctions, or run along or across other wires), all other pins get a
  stub wire carrying a net label (upstream draws the label text along its
  wire, so the stub is as long as the estimated label text: 0.9 mm per
  character); the stub goes straight out or jogs sideways, whichever first
  keeps the label rectangle and the wire clear of symbols with their texts,
  other labels, wires and pins; a net with several segments gets a label
  on each segment (supply symbols count as label).
- `DisconnectSignals` (tool `disconnect`): removes the wires at the pins;
  the other ends of direct wires keep their net through a stub with label.
- `AutoPlaceSymbols` (`component_add` without position, `symbol_move`
  with `auto`): first free place in rows of about 200 mm (2.54 mm grid)
  without overlapping symbols (with their texts), wires or labels; every
  symbol keeps 12.7 mm of room in the direction of each pin for a stub
  with a net label, so labels of neighbors do not collide.
- `TidySchematic` (tool `schematic_tidy`): removes the wiring of a page
  (nets, net classes and boards stay), places the symbols again by
  connectivity (greedy: the most connected symbol first, then the symbol
  most strongly connected to the placed ones at the free grid position
  minimizing the weighted pin distance to its nets, keeping the pin room;
  net weights 1/(n-1), supply nets less) and draws all nets again with
  `ConnectNet` (shortest nets first, direct wires up to 38.1 mm); supply
  symbols end up next to a pin of their net. Deterministic.
- `AutoPlaceDevices` (tool `device_auto_place`): greedy placement by
  connectivity inside the board outline: the most connected device first
  (near the center), then the device most strongly connected to the
  placed ones at the free position (1 mm steps) minimizing the weighted
  Manhattan length of its air wires to placed pads plus a small pull to
  the center; connectors (designator prefix J, X, P, CN) only at the
  outline edge. The gap between footprints (courtyard/pads/texts) grows
  with the free board area (up to 5 mm, at least `spacing`) so the
  devices spread over the board and leave room for traces; if not all
  devices fit, smaller gaps and finally row packing are tried. Board edge
  clearance of the DRC settings; devices which do not fit are put right
  of the board and reported (`outcome: partial`). Deterministic.

## Resources

The stdio suite also selects single- and multi-gate symbol variants from
the upstream populated-library fixture, moves an individual gate with undo,
connects components across two pages, routes a four-layer board and exports
all four copper layers. A save/open round trip preserves the selected variant
and board settings. When `LIBREPCB_CLI` is available, upstream opens, checks
and exports the generated project as well. This scenario does not need
separately installed official libraries; its internal copper layers remain
unused by the simple routed circuit.

The editor routing regression suite checks the geometry actually represented
by each generated via, including automatic/explicit dimensions, net-class
inheritance and DRC clamping. In particular, a 0.3 mm drill with a 0.6 mm
outer diameter must not turn into the invalid combination of automatic
drill and explicit size. A forced layer transition exercises application,
undo/redo, full DRC and upstream CLI acceptance.

Routing tools and SES import share an editor-level before/after assessment
of the applied model: full DRC, actual air wires and circuit signal
assignments. The response includes bounded new-error evidence and never
calls a regressing or unchecked result complete just because air wires
reached zero. See [routing reliability](mcp-routing-reliability.md) for
the contract, reproducible cases and current priorities.

The stroke fonts new projects get (`resources/fontobene/*.bene`, like
upstream's `Project::create()` copies them from `share/librepcb`) and the
default font for rendering come from `librepcb_scene::resources`: a
resources directory on disk (`LIBREPCB_SHARE`, else `../share/librepcb`
next to the executable) wins; otherwise the fonts embedded into the
binary at build time from `$LIBREPCB_UPSTREAM_DIR/share/librepcb/fontobene`
(nothing is copied into this repository) are used, so an installed
`librepcb-mcp` works on machines without the upstream checkout.

## Tests

- Unit tests per tool adapter on an in-memory session.
- End-to-end test over stdio (rmcp client): create workspace, install/scan a
  test library, create a project, add components, connect nets, place
  devices, set outline, autoroute, add a ground plane, run ERC/DRC, export,
  save, and open the result with the official `librepcb-cli` (ERC/DRC
  clean, Gerber export works).

### Reviewed builtin routing

`routing_prepare` accepts builtin board/nets/layers and optional dimensions in
mm, plus `expected_revision`. It captures an isolated native snapshot, releases
the host locks, runs builtin search and before/after DRC, and retains the exact
mutation candidate only if its source project session and revision still match.
It does not edit the live board or its revision, dirty state, undo/redo or files.
If rebuilt planes leave more connections open on a net, validation lists its
ID in `unrouted_regressions` and includes names, before/after counts and
sampled air-wire endpoints in mm in `unrouted_regression_details`. These
endpoints describe the rebuilt connection tree and help locate lost plane
contacts. The sample covers up to 20 nets and eight air wires per net.
`validation.connectivity_progress` reports `improved`, `unchanged`, or
`regressed` independently of `acceptable`. A regression on any net takes
precedence. `unrouted_improvements` lists up to 20 nets with their names and
counts before and after, so a caller can tell whether the target net improved.

`routing_copper_prepare` previews an explicit batch of vias and trace paths.
Give each new via a label and use `{"via":"label"}` as a trace endpoint;
pad names, existing anchor UUIDs and coordinates also work. The whole batch
is checked against the board as one candidate and applies in one undo step.
Use `remove_traces` to replace selected existing traces by UUID in the
same candidate. Use `remove_vias` to replace selected vias by UUID and list
all their attached traces in `remove_traces`. The other copper keeps its
geometry and UUIDs. `placements` adds explicit moves and rotations of unlocked
devices to the same candidate. Each entry names a `component` and supplies an
absolute `position` in mm and/or `rotation` in degrees. Include the traces that
must change in `remove_traces` and provide their replacement paths. Unlisted
traces and vias remain protected. The combined request allows up to 512
placements, vias, paths, intermediate points and removals.
Use `base_candidate` to add the explicit geometry to an earlier candidate,
including a rejected route with a plane split. The combined patch is checked
against the unchanged live board and remains independently previewable.
Candidate validation reports new DRC warnings by identity as well as errors.
Review warnings such as undersized drills before applying; warnings alone
do not block a candidate.

`routing_compare` uses one detached baseline and a fresh copy of it for
each of two or three distinct builtin initial orders. Pass common
`routing_prepare` settings under `options` and optional `orders` at the
top level. The default orders are `shortest_first`, `longest_first`, and
`constrained_first`. Each still honors the same critical tier and geometry
options. The constrained order estimates pressure using exterior pad escapes
when enabled and ordinary grid entries otherwise.

The batch is published together after the same source checks as single
preparation, provided there is room for all candidates. The response contains
the shared baseline DRC, per-candidate final DRC and costs, and a nullable
`recommended` ID. Ranking compares acceptance, new and remaining
non-connectivity errors, nets whose air-wire count regressed, remaining
critical and total air wires, via count, then total trace centerline length
in mm. Metrics describe the resulting whole board; existing copper is common
to all trials. Caller order breaks ties. A candidate with unacceptable
validation or a connectivity regression is never recommended. Inspect any
candidate with `routing_preview`; there is no automatic apply.

`routing_preview` reads candidate metadata and exact mutations with `offset` and
`limit` (1–200, default 100), `next_offset` and a `stale` flag. This is a data
preview; GUI overlays and candidate PNG rendering are future work. Retained storage bounds do not limit
the number of concurrently running preparation workers.
`routing_apply` checks the candidate revision independently of the optional
caller revision (and rejects a changed save time or path, since text
attributes can affect geometry), refuses incomplete validation or new non-connectivity DRC
errors, then applies the frozen mutations as one undo group without rerouting.
A successful commit consumes the candidate and reports `applied: true`,
`consumed: true` and fresh applied-model validation. A failed post-commit check
is reported as partial validation, never as an absent write. Partial connectivity
is allowed and remains explicit. `routing_discard` frees a candidate without an
edit. Closing the project clears candidates. Storage refuses more than eight
candidates or a patch over 100,000 mutations; existing candidates are not evicted.

`routing_region_prepare` takes a rectangle in board millimeters, explicit
target nets under `route` and an optional `rip_up` list of trace and via UUIDs.
The rectangle limits new copper. The candidate preserves the geometry and
UUIDs of copper outside the list and uses the shared preview and apply tools.

See [routing service design](routing-service-design.md) for subsequent stages.
The existing `autoroute` compatibility path continues to apply directly.
