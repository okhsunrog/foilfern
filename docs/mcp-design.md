# MCP server design

Current contract of `crates/librepcb-mcp`, reviewed 2026-11-11 against `f3c3fda`.
The [Konnect notes](mcp-research-konnect.md) record its historical rationale;
the [roadmap](roadmap.md) defines planned work. Goal: an agent designs a board end to
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
  include the undo stack's clean state, manual modifications and a pending
  file format upgrade.
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
  `Project::changes_since(cursor)` and repaints. An atomic write that rolls
  back successfully preserves the revision, history and host notification
  sequence. An incomplete rollback returns an internal error and forces a
  resync.
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

## Tool surface

At the reviewed revision, `server.rs` registers 81 tools. The schemas advertised
by `tools/list` are authoritative; the historical target of fewer than 60 tools
is no longer an accurate inventory. New operations should reuse domain services
and batch inputs where appropriate rather than add aliases for existing behavior.

`server_info` reports package and file-format versions and
`api_capability_version` (currently `14`). Its `build` object contains
`source_commit` (full Git commit ID or `null`) and `tracked_source_state`
(`clean`, `dirty` or `unknown`). The state covers staged and unstaged changes to
tracked workspace files. Untracked files, external dependencies and build inputs
outside the checkout are outside that check. Builds without usable Git metadata
report unknown provenance. The values are captured at build time.
`cargo_profile` is Cargo's `PROFILE` label (`debug` or `release`), which groups
custom profiles by inheritance. `cargo_opt_level` reports Cargo's optimization
setting for the MCP crate. Record the actual build command for timing comparisons.

Declarative schematic creation (P1) starts with an open native project and
sheet. Persisted enforced requirements (P2), worker jobs/cancellation (P3) and
visual candidate review (P5) are not current MCP capabilities.
`routing_compare` already ranks builtin-order candidates; runtime
candidate capacity does not limit the number of active preparation workers.

Session / project:
`server_info`, `workspace_open`, `workspace_create`, `project_create`,
`project_open`, `project_save`, `project_close`, `project_summary`
(metadata, pages, boards, counts, revision, unsaved changes), `undo`, `redo`,
`history`.

### Declarative edits to an existing schematic

`design_preview` takes a version 1 document and returns a semantic `diff`,
the number of checked pin memberships, a preview ID and a native page render.
The [ESP32-C3 EN/RESET example](examples/esp32-c3-en-reset.json) targets the
saved comparison board. Bind each document key to a component instance UUID
from `component_get` or to an existing `designator`. Use a UUID to keep the
binding after a rename. In `nets`, address pins as `key.pin`. The existing
circuit must match the listed memberships. Unlisted endpoints can share those
nets elsewhere on the project.

Value, position and rotation edits take `expected` and `set` values.
Placement fields require a single symbol on the selected sheet.
Omitting a field preserves its current value. When the current value equals
`set`, it contributes an empty diff. A third value produces a conflict and
identifies the managed field. Positions are in millimeters and rotations are in
degrees.

Use `block` to position selected components inside a rectangle on the sheet.
Specify `members` in visual order, `min` and `max` corners
as `{x,y}` in millimeters, `direction` (`row` or `column`), and a minimum `gap`
in millimeters. A row runs left to right, a column top to bottom. An optional
`baseline` gives the common Y coordinate for a row or X for a column. Each
symbol retains its grid phase, so a symbol off the grid can end up slightly
offset from the baseline. With no baseline, members are packed along the top
or left edge of the area.

List only the members whose positions the document may change. Surrounding
components, wires and annotations stay in place. Existing symbol fields and
independent wire stubs move with their symbol. To move members joined by a
shared wire, include an explicit wire group covering every pin of that
segment, including any fixed pins. Preview places the members and redraws
those groups together. A bus tap or an unlisted pin on a replaced segment
causes the preview to fail. Each member requires a single symbol on the
selected sheet. `block_position` entries report the changed positions and
the preview reviews annotation overlaps for the block.

Use `wire_groups` for pins that should be joined by visible wires on that
sheet. Each entry gives an existing `net` and at least two `pins` in the same
`key.pin` form. Group pins are checked against the circuit just like the
memberships in `nets`, so they only need to be listed once. The editor tries
local orthogonal routes at the proposed positions and labels each new group.
The diff lists the UUIDs of the segments it would replace. If a replacement
would touch a segment with a pin outside the group or a bus connection, the
preview fails. Pins already physically connected satisfy the group, preserving
its existing wire geometry.

For explicit wiring, add `paths` to a wire group. Each path lists pin names
and absolute points `{x,y}` in mm. Omit `pins` to infer them from the pin
vertices, or supply it explicitly to check that every listed pin appears.
`verified_pins` counts each resolved circuit signal once across the document.
Edges must be orthogonal. The paths form one connected segment, and a branch ending on
another path splits it at a shared junction. A single-pin group can draw a
stub. Separate segments on the same net go in separate groups. To show their
relationship, place a `label` on each wire or use supply symbols. `label` is
an optional point on the wire; its orientation follows the edge there.
For either axis of a waypoint or label, replace the numeric coordinate with
`{"pin":"cap.1","offset":2.54}` to use that pin's final coordinate plus the
signed offset in mm. The offset defaults to zero. For example,
`{"x":{"pin":"cap.1"},"y":{"pin":"switch.1","offset":2.54}}`
aligns the point with the capacitor pin and puts it 2.54 mm above the switch
pin. References must belong to that wire group. A pin itself is written as a
string vertex, such as `"cap.1"`.

The preview checks explicit wires against symbol bounds, unrelated pins,
buses and other wires, including other explicit groups in the document. It
rejects touching, overlapping or crossing wires between distinct groups.
Segments being replaced are excluded from collision checks. The diff reports
the requested paths and the segment UUIDs they replace. Identical geometry
and label placement are a no-op. On a new sheet, `connect` with
`assign_only: true` assigns the net without drawing stubs or moving supply
symbols. After assigning nets and placing symbols, use `schematic_get` to
read absolute pin positions before describing paths.

Use `notes` to add free text to the schematic with `text`, `position` and
`height` in mm. An identical note at the requested position is a no-op.

Use `symbol_styles` to replace rectangular placeholders with native artwork.
Keys are project library symbol UUIDs from `component_get.symbols[].library_symbol`.
The `capacitor` and `normally_open_push_button` styles accept two aligned,
opposing pins. The [four-terminal crystal example](examples/esp32-c3-crystal.json)
uses an object with `style: "crystal"`, `resonator_pins`, and `case_pins`.
Each pair lists two distinct library pin UUIDs from `component_get.symbols[].pins[].uuid`.
The template accepts a row of opposing resonator pins and a matching case row
below it in the library symbol's coordinates. The case leads are drawn
independently. Any shared ground connection comes from the schematic.
A style expects a single plain rectangle. A symbol already using the requested
outline is a no-op. Other artwork is refused.
The edit affects the shared symbol in the current project's library. The diff
lists every affected designator, including uses on other sheets. Pins, pin
names, component text positions, wires and device mappings are preserved.
The templates move pin names that sit on the lead axis away from the lead.

Call `design_apply` with the latest preview ID in the same open project.
It applies the rendered patch in one undo group. An edit or save invalidates
the preview, and applying an empty diff leaves the revision and history intact.
Use `add_nets` to create any missing named nets before adding parts. Existing
nets retain their UUIDs and classes. Use
[`add_components`](examples/esp32-c3-add-decoupler.json) to add parts to the
selected sheet. Each entry provides a stable key, the initial designator,
the exact component and device UUIDs, a value and a map of package pad names
to net names. Specify `symbol_variant` to select a particular library variant.
For a schematic-only part, omit `device` and use exact component signal names
in `connections`. The preview reports the pad, signal and library UUIDs.
Omit the initial position for automatic placement. Set `mirrored: true` for
an initial mirrored symbol, for example a header with pins facing left.
For a component with one placed symbol, `fields` sets initial reference and
value placement. Use `name`, `value` or a text UUID as the key. For example,
`{"name":{"offset":{"x":2.54,"y":1.27},"rotation":0,"height":1.5}}`
sets a horizontal designator relative to the final symbol origin in sheet
axes. Omit a field property to use the library default. Replaying the same
entry keeps subsequent designator, placement and field edits. A changed part, value or pin
membership reports a conflict. The key is stored in the component's native
`FOILFERN_DESIGN_KEY` attribute. Apply and undo include the embedded library
files. `nets` assertions and `wire_groups` can refer to new component keys in
the same document. Set `assign_only: true` on a new part to supply its wires
with explicit paths. By default each assigned pin gets a labeled stub.
PCB placement remains a separate operation.

The [ESP32-C3 dev board](examples/esp32-c3-devboard.json) creates a complete
one-page schematic with explicit wiring, notes and initial symbol fields.
Install `LibrePCB Base`, `LibrePCB Connectors`, `Espressif Systems` and `C&K`
in the workspace, create a project with a `Main` sheet, then send the document
to `design_preview` and apply the reviewed patch. The document pins exact
library identities. If a library update changes a selected part, resolve it
again through `library_element` and verify its pad mapping and pin positions.

Set `review_components` to document keys to check annotation overlaps in the
preview. The report gives the object IDs, designators or net names and the
intersection in sheet coordinates. It checks rendered bounding boxes of
symbol fields, page text and net labels touching the selected symbols or
their connected wire segments. It is advisory and covers annotation spacing.
Use `text_positions` under an existing component to move its reference or
value independently of the symbol. Each entry is keyed by the text UUID from
`schematic_get` and uses `expected` and `set` positions in millimeters.
Preview symbol moves and absolute text positions in separate requests.

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
position, rotation, side, optional device/footprint choice and `include_pads`),
`device_auto_place` (all unplaced devices inside the outline, placed by
connectivity),
`trace_add` (net or pads, layer, width, list of points; endpoints snap to
pads/vias/junctions at the given coordinates), `trace_update` (batch width
changes by trace UUIDs or net names, preserving UUIDs and connected geometry,
with before/after DRC evidence), `via_add`, `trace_remove`,
`plane_add` (net, layer, outline or whole board, clearance, connect style),
`design_rules_set`, `planes_rebuild`, `autoroute` (built-in router for
simple boards, see below), `unrouted` (air wires).

`device_place` returns full pad geometry by default. With `include_pads: false`
it returns the placed device's identity and placement, with the usual revision,
warnings and journal summary. The compact response omits the `pads` field and
skips resolving pad geometry. Use `component_get` for a read-only view of a
component's placements and pads, or `board_get` for board-wide detail.

Checks and outputs:
`erc_run`, `drc_run`, `export_fabrication` (Gerber + Excellon),
`export_bom`, `export_pick_place`, `export_netlist`, `jobs_list`,
`jobs_run`, `render` (PNG of a schematic page or a board side, as MCP image
content; the target defaults to the board when `board` or `side` is
given).

Escape hatch: `mutation_apply` (core `Mutation` JSON, full validation, one
undo group), `mutation_schema`.

### Read-only trace path measurements

`trace_path_measure` accepts `from`, `to`, optional `board` and
`expected_revision`. Endpoints are `"OLED1.28"` style pad addresses or
`{"component":"<instance UUID>","pad":"<footprint pad UUID>"}`. Pad UUID,
package pad name and component signal name are tried in that order; an
ambiguous signal/name refuses instead of selecting an arbitrary physical pad.
Both resolved pads must have the same assigned net.

The response pins `project`, `board` and `base_revision`, resolves both endpoint
identities, and reports `direct_mm`. A measured shortest explicit path includes
`length_mm`, layers, ordered trace IDs/segment IDs/widths/lengths and unique
segment-scoped via anchors. Lengths follow trace centerlines between anchors,
including pad centers; via barrel length is excluded. Footprint pads are keyed
by component instance and pad, local anchors by segment and layer. Only valid
via/pad copper joins layers; a junction is not an implicit via.

A missing explicit path returns partial `status: "not_measured"`, null
`length_mm` and a reason. Plane copper and unanchored copper overlaps are not
measured or inferred. `same_net_plane_present` is a hint about board contents,
not evidence that a particular plane connects these endpoints. A measured
trace path may coexist with a shorter unmeasured plane path. These measurements
do not replace rebuilt-plane connectivity or DRC. No model, derived plane,
revision, history or file is changed. The algorithm lives in core and reuses
its existing petgraph dependency; no new dependency or native format is added.

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

Preparation retains one complete source DRC result per open project session.
Later candidates may reuse it only for the same project, board, revision and
save-time/path context. `baseline_reused` reports whether this happened.
The detached copy still rebuilds planes and air wires before routing. Every
candidate receives a fresh full after-check, and a non-empty applied patch
receives a fresh full live-model check. Incomplete checks and stale worker
completions cannot populate the cache. Discarding a candidate preserves the
still-current baseline; closing the project drops it. Comparison trials share
the same mechanism and report `baseline_reused` at the comparison level.

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
The rectangle limits new copper. An optional `rip_up_region` inside it
limits removal to a smaller area while letting the route reach distant pads.
Copper removal still requires each trace or via UUID in `rip_up`. Air wires
skipped because an endpoint lies outside the routing rectangle appear in
`routing.skipped_outside_region` with reason `outside_region`.
The candidate preserves the geometry and UUIDs of copper outside the list
and uses the shared preview and apply tools.

See [routing service design](routing-service-design.md) for subsequent stages.
The existing `autoroute` compatibility path continues to apply directly.
