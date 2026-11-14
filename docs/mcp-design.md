# MCP server design

Current contract of `crates/foilfern-mcp`, reviewed 2026-11-11 against `f3c3fda`.
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
foilfern-mcp      MCP protocol (rmcp, stdio + optional streamable HTTP), tool
                  DTOs (serde + schemars), outcome envelope, session state
foilfern-editor   UndoStack + intent-level commands over Mutation (no MCP types)
foilfern-core     model, Mutation/apply, ERC/DRC, exports, jobs, workspace/library DB
foilfern-network  library download (api.librepcb.org)
```

`foilfern-mcp` owns no domain logic: every tool is a thin adapter that
parses the DTO, calls one editor command or core query, and builds the
response. Logic an agent needs that a UI would also need (auto-placement of
wires, device placement, routing helpers) lives in `foilfern-editor`.

## Process and state

- Binary `foilfern-mcp` (stdio by default, `--http <addr>` for streamable
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
  across `.await`. Detached tools lock for capture and publication and run
  their worker on the captured project between those calls.
- `OpenProject { editor: ProjectEditor, file_system (lock), saved_revision,
  upgraded }` lives in the editor crate (`foilfern_editor::OpenProject`,
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

- Library API (`foilfern_mcp::{host, transport}`): the host creates
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

Output jobs install the same `foilfern-graphics::ProjectGraphicsExporter` and
`foilfern-step::board_exporter()` adapters as the desktop application and CLI.
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
  `no_project`, `no_workspace`, `io`, `network`, `not_available`, `refused`,
  `cancelled`, `budget_exhausted`, `internal`) plus the message.
- Checks never read as passing when they did not run: `run_drc` returns
  `ran: true` and the message list, or an error.

## Tool surface

At the reviewed revision, `server.rs` registers 87 tools. The schemas advertised
by `tools/list` are authoritative; the historical target of fewer than 60 tools
is no longer an accurate inventory. New operations should reuse domain services
and batch inputs where appropriate rather than add aliases for existing behavior.

`server_info` reports package and file-format versions and
`api_capability_version` (currently `52`, including supplier acquisition,
official KiCad imports, rendered board-placement reviews, declared connector axes, scoped legend targets and bounded legend placement).
Its `build` object contains
`source_commit` (full Git commit ID or `null`) and `tracked_source_state`
(`clean`, `dirty` or `unknown`). The state covers staged and unstaged changes to
tracked workspace files. Untracked files, external dependencies and build inputs
outside the checkout are outside that check. Builds without usable Git metadata
report unknown provenance. The values are captured at build time.
`cargo_profile` is Cargo's `PROFILE` label (`debug` or `release`), which groups
custom profiles by inheritance. `cargo_opt_level` reports Cargo's optimization
setting for the MCP crate. Record the actual build command for timing comparisons.

Declarative schematic creation and board placement (P1) start with an open
native project. Saved schematic and board requirements (P2) are checked
during design apply. Design previews have worker status and cancellation.
Schematic previews can return matched images.

`routing_compare` ranks builtin candidates by order, via cost and path
preferences. Local repair and region-based placement searches expose progress,
time budgets and cooperative cancellation. The retained-candidate capacity
does not bound the total number of active preparation workers.

Session / project:
`server_info`, `workspace_open`, `workspace_create`, `project_create`,
`project_open`, `project_save`, `project_close`, `project_summary`
(metadata, pages, boards, counts, revision, unsaved changes), `undo`, `redo`,
`history`.

### Declarative schematic updates

`design_preview` takes a version 1 document and returns a semantic `diff`,
the number of checked pin memberships, a preview ID, the selected schematic
UUID and a native page render. Set `create_schematic: true` to use `schematic`
as an exact sheet name and create the page if missing. Apply and undo include
the page itself. Omit `create_schematic` to select an existing page.
Set `compare_images: true` to attach two PNGs with one common extent, scale
and origin. The `images` array names them in MCP content order, `after`
followed by `before`. A new sheet has a blank before image.
Set `image_region` to `{"min":{"x":20,"y":-10},"max":{"x":60,"y":10}}`
to request the same pair cropped to a rectangle in native sheet millimeters
with positive Y up. Both axes must have positive extent. The output fits the
region without stretching and fills any padding with the page background.
The read-only `render` tool accepts the same `image_region` on a schematic
target. It returns one image of the live sheet with the requested frame.

The server captures the source under the session lock, then plans, checks and
renders on the detached model. One design preview may run per server state.
A second request returns `conflict` and can be retried. Other tools can use
the project while the preview runs. A changed revision, save context, library
source or open project causes publication to return `stale_revision`.

`design_preview_status` returns the active job UUID, stage, elapsed time,
remaining budget and the last terminal outcome. The stages are `capture`,
`plan`, `check`, `render` and `publish`. `budget_ms` is optional and begins
at admission. `design_preview_cancel` takes the active job UUID and requests
a cooperative stop. The worker checks between phases, components and wire
groups, and between the two comparison frames. An in-flight library lookup,
wire group, ERC check or individual frame can
finish before the next checkpoint. Cancellation or expiry returns
`cancelled` or `budget_exhausted` and prevents publication. The terminal
report is available after worker capacity is released. This status and the
cancel request use the worker state directly, without locking the project.
The terminal `outcome` can be `complete`, `stale`, `failed`, `cancelled`,
`budget_exhausted` or `request_ended` if the request ended before publication.

The `erc` report compares the source and proposed project and lists
`introduced`, `inherited` and `resolved` findings. Matching uses the rule and
native object identity, so text and designator changes alone do not count as
new findings. Each entry uses the same format as `erc_run`. An inherited
finding includes `previously_approved` when its approval status changes.
The preview leaves approvals and the project revision untouched. ERC findings
are advisory here. Review them and run `erc_run` on the applied result.
The [ESP32-C3 EN/RESET example](examples/esp32-c3-en-reset.json) targets the
saved comparison board. Bind each document key to a component instance UUID
from `component_get` or to an existing `designator`. Use a UUID to keep the
binding after a rename. In `nets`, address pins as `key.pin`. The existing
circuit must match the listed memberships. Unlisted endpoints can share those
nets elsewhere on the project.

Value, designator, position and rotation edits take `expected` and `set` values.
To edit a designator, bind by `uuid` and use `name`, for example
`{"expected":"R1","set":"R2"}`. The target name must be free, and the
component keeps its UUID and connectivity.
Component-level placement fields require a single symbol on the selected
sheet. To edit individual gates, use `components.<key>.symbols` keyed by
placed symbol UUID from `schematic_get` or `component_get`. Each entry accepts
`position`, `rotation` and `mirrored`, with `expected` and `set` values.
Use one placement form per component. Apply absolute text positions for a
transformed symbol in a separate preview.
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

`schematic_review` finds overlaps involving symbol fields, pin names and numbers,
net labels, page notes and schematic wires on the live sheet. Pass `components` with designators
or component UUIDs to focus the review on their symbols and attached labels.
Each finding identifies both annotations and the overlap rectangle as
`[min_x,min_y,max_x,max_y]` in sheet millimeters. In `design_preview`,
`review_components` takes document component keys and checks the proposed sheet.
`annotation_changes` reports `introduced`, `inherited` and `resolved`
overlaps. Matching uses the two annotation identities regardless of their order
or position. `annotation_overlaps` lists all findings in the proposed sheet.
The check uses annotation bounds from the schematic scene. Findings are advisory.

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

Call `design_apply` with the latest preview ID in the same open project to
apply the rendered patch in one undo group. An edit or save invalidates the
preview, and applying an empty diff leaves the revision and history intact.
Use `design_discard` with that ID to release a completed preview while keeping
the revision and undo history. A new completed preview replaces the previous
one. `design_preview_cancel` acts on an active job UUID.
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
Use `board_layout` to place those parts on the PCB in the same patch.

Use `remove_components` to delete component instances by UUID. Repeating an
already applied removal is a no-op. Use `pin_nets` to change a bound pin's
membership, for example `{"r.1":{"expected":"EN","set":"RESET"}}`.
Both fields are required. Use `null` for an unconnected pin. The destination
must exist or be declared in `add_nets`. A third membership is a conflict.
The preview refuses a cascade that would also change an unlisted pin's net.
After reconnecting a pin, `wire_groups` can replace its default labeled stub.
Reconnecting with a stub requires one placed pin for that signal on the
selected sheet. Component removal includes its gates on all sheets. Embedded
library elements and previously unused nets are retained. A net that loses
its last schematic use is removed unless the document ensures it in `add_nets`.

Use `rename_nets` keyed by net UUID from `net_list` to change a name while
preserving net identity and geometry. Each value has `expected` and `set`
names. A name already taken by another net is a conflict. Renames run before
`add_nets`, pin edits and wire groups, so those fields use the resulting names.

`no_connects` sets visual no-connect markers on the selected sheet. Keys use
`key.pin` and can refer to existing or newly added components. Set a key to
`true` to mark an open pin or `false` to clear its marker. A `true` entry
on a connected pin is a conflict. `schematic_get` reports whether each pin
currently displays the marker. A later connection hides it and a later
disconnect reveals it again unless it was cleared. Undo and native project
save include the marks. The marks do not change electrical membership or
ERC. Foilfern uses the same sheet asset as for imported KiCad marks.

Review `board_impact` before applying a topology change. It lists each
affected board and UUIDs of removed or changed traces, vias and planes,
removed devices, and surviving planes on affected nets whose fills can change.
This evidence describes the editor patch and the affected plane nets. Use
board checks separately to evaluate the resulting physical connectivity.

The [ESP32-C3 dev board](examples/esp32-c3-devboard.json) creates a complete
one-page schematic with explicit wiring, notes and initial symbol fields.
Install `LibrePCB Base`, `LibrePCB Connectors`, `Espressif Systems` and `C&K`
in the workspace, create a project, then send the document to `design_preview`
and apply the reviewed patch. It creates `Main` if needed. The document pins exact
library identities. If a library update changes a selected part, resolve it
again through `library_element` and verify its pad mapping and pin positions.

Set `review_components` to document keys to check annotation overlaps in the
preview. The report uses the same bounds check as `schematic_review` and
includes findings involving the selected symbols or their connected segments.
Use `text_positions` under an existing component to move its reference or
value independently of the symbol. Each entry is keyed by the text UUID from
`schematic_get` and uses `expected` and `set` positions in millimeters.
Preview symbol moves and absolute text positions in separate requests.

Use `place_text` to fit one symbol field inside a rectangle on the selected
sheet. Bind the part in `components` or `add_components` and pass its document
key, `text` (`name`, `value`, or a placed text UUID), and `min` and `max` in
sheet millimeters. For example, to place the value of an existing U6:

```json
{
  "version": 1,
  "schematic": "esp-pd",
  "components": {"monitor": {"designator": "U6"}},
  "place_text": {
    "component": "monitor", "text": "value",
    "min": {"x": 208, "y": -161}, "max": {"x": 263, "y": -125},
    "step_mm": 0.8
  }
}
```

The field stays where it is if its bounds already fit with the requested
clearance. Otherwise the planner checks a grid from the lower corner,
including the upper edges, in order of distance from the current anchor.
Ties use X then Y. The default step is 0.635 mm and the default clearance
from visible geometry is 0.5 mm. A grid may contain up to 4096 positions.
The search runs after other edits in the document and checks the resulting
schematic scene. Supply a text UUID for multi-gate components. A field can
use either `place_text` or an explicit `text_positions` entry.

`text_placement` reports the field UUID, position, trials and status (`moved`,
`unchanged`, `no_fit`, `locked`, or `not_visible`). A move appears in `diff`
as `text_position`, and the preview includes annotation changes for the target
component. On sheets with embedded images the planner also checks the export
geometry and font used by the PNG preview.

### Declarative board placement

Supply `board_layout` in a version 1 design document. Omit `schematic` for
a board-only update, or include the two sections to plan and apply them in
one operation. Select the board by name, index or UUID. The first board is the
default. Placement keys resolve to document component keys, designators or
component instance UUIDs. New parts can use the same keys as `add_components`.

```json
{
  "version": 1,
  "board_layout": {
    "regions": {
      "connector": {"min": {"x": 6, "y": 9}, "max": {"x": 8, "y": 11}}
    },
    "placements": {
      "J1": {"position": {"x": 7, "y": 10}, "rotation": 90, "side": "top",
             "locked": true, "region": "connector"},
      "U1": {"position": {"x": 16, "y": 10}},
      "C5": {"relative_to": {"component": "U1", "pad": "5",
                             "offset": {"x": 1.2, "y": -0.8}}}
    }
  }
}
```

For an unplaced part, choose either `position` or `relative_to`. The latter
uses the final position of the anchor's origin or of a pad named by package
pad name or footprint pad UUID. Offsets are in board X/Y millimeters.
Rotation is in counterclockwise degrees. New placements default to zero
rotation, the top side and unlocked. Optionally specify exact library
`device` and `footprint` UUIDs. On an existing placement these UUIDs act as
identity assertions. A named region checks the device origin against the
inclusive bounds in board millimeters.

For an existing placement, omitted pose fields keep their current values.
To change a field, provide its current value in `expected`, for example
`"expected":{"position":{"x":15,"y":10}}`. A field already at the requested
value is a no-op. Moving, rotating or flipping a locked part requires a
separate reviewed unlock.

`board_layout.requirements` takes entries with `id`, `source`, `severity`
and `check` from [saved board requirements](#saved-board-requirements).
The selected board is implicit. Existing IDs must resolve to the same
rule. New IDs are saved with the patch. Use a saved `device_region` check
for a region that should also constrain later edits.

The semantic `diff` reports device poses and new saved rules.
`board_layout.changed_traces` shows the UUIDs and endpoint positions of
traces that follow the moved pads. The board report includes DRC, rebuilt
planes, connectivity, region checks and saved requirement evidence.
`applicable: false` lists why apply is blocked. New devices can introduce
expected air wires. The check still blocks increased gaps among the
previously placed terminals of any existing net, new DRC errors, courtyard
conflicts and failed hard rules. Use `unrouted_after` to see the total
remaining connections, including new devices.

Board placement previews include a candidate PNG, even when checks block apply.
Set `board_layout.image_side` to `top` (default) or `bottom`.
`board_layout.image.index` identifies the image among MCP image content blocks:
index 0 for board-only previews; for combined documents the board image follows
the schematic image(s). It shows the checked candidate, including any reviewed
repair and fresh derived plane/air-wire data. It does not mutate the live board.
`board_layout.devices` contains the selected devices and their absolute pad
positions/rotations using the same schema as `board_get`. Coordinates remain in
board X/Y even for a mirrored bottom-side view. Connector mating direction is not
inferred from pad numbering or the rendered image.

Device read-back (also in placement previews) includes `connector_orientation`.
An instance or library-device string attribute
`FOILFERN_CONNECTOR_MATING_DIRECTION` can declare `+x`, `-x`, `+y` or `-y` in
footprint-local axes, pointing from the connector body toward the external plug.
Instance attributes override library-device attributes. Status `declared` reports
the attribute source, local axis and a unitless `board_vector` transformed through
the native device placement, including bottom-side mirroring. The vector uses
board X/Y coordinates, independently of viewing side, rounded at nanometre
precision for a one-millimetre reference axis. Missing metadata gives `unknown`;
unsupported values give `invalid_metadata` without falling back to a direction
guess. A declaration is metadata, not verified mechanical compatibility.

Use `board_layout.repair` to inspect or replace local copper on a board
with existing routes. Pass a routing rectangle in `region`, an optional
smaller `edit_region`, and the net names or UUIDs in `editable_nets`.
The first preview lists `repair.proposals` based on exact copper clearance
findings and traces that follow moved pads. Each proposal identifies its
blockers, retained anchors and the copper UUIDs in `branch`.
`repair.unresolved_blockers` identifies copper without an eligible
proposal and reports its net and reason.

```json
{
  "region": {"min":{"x":7,"y":9},"max":{"x":21,"y":17}},
  "editable_nets": ["LINK"]
}
```

The caller can revise the pose, discard the preview or resubmit the
document with the preview's `base_revision` as `repair.expected_revision`
and selected proposal lists as `repair.branches`, for example
`"branches":[["<trace UUID>","<via UUID>","<trace UUID>"]]`. A caller may
also specify its own exact lists. Each list must be a simple branch
between two retained anchors. List every incident trace when removing a
via. The request accepts up to eight disjoint branches with 1–16 objects
each. The builtin router reconnects those anchors using the final pad
positions and uses the saved rules. Optional `repair.route` accepts the
builtin settings for explicit copper continuation, with nets inferred
from the branches. Use the design preview budget and cancellation for the
whole operation.

The routed preview reports removed and added copper under `repair`.
Unlisted routing copper on the staged board is preserved, including any
pad endpoint changes already required by the proposed placement.
Full board checks reject failed branches, new errors, new connectivity
gaps, courtyard conflicts and hard rule violations. Repairs run only when
an existing device changes pose. Replaying an applied pose skips the
already completed copper replacement.

Use `design_apply` with the preview ID. The exact patch is tied to the
reviewed revision and goes into one undo entry. After save and reopen, an
identical document at its desired state gives an empty diff.

### Schematic requirements

`requirements_get` returns the current `document` and `revision`.
`requirements_set` takes the full replacement document and the
`expected_revision` from that read. An empty `requirements` array clears it.
Repeating an equivalent document in any list order adds no undo entry. A
change is one undo step and takes effect for previews immediately. Save the
project to persist it. The version 1 document has a `requirements` array with
unique `id` strings, a nonempty `source`, `severity` and `check` for each row.

| `check.kind` | Fields | Measurement |
| --- | --- | --- |
| `symbol_position` | `symbol: {schematic, symbol}`, `position: {x, y}` | Exact symbol origin in mm on the sheet |
| `symbol_orientation` | `symbol: {schematic, symbol}`, `rotation`, `mirrored` | Exact orientation in degrees modulo 360 and mirror state |
| `symbol_region` | `symbol: {schematic, symbol}`, `min` and `max` points | Symbol origin inside the inclusive rectangle in mm |
| `pin_net` | `pin: {component, signal}`, `net` | Net UUID, or explicit `null` for an unconnected signal |

Use `schematic_get` for sheet and symbol instance UUIDs, `component_get` for
component instance and library signal UUIDs, and `net_list` for net UUIDs.
These references survive designator and net renames. `requirements_check`
evaluates the current project. `design_preview` evaluates the proposed project
and includes the same report under `requirements` when rules are present.
Each report identifies its project UUID and artifact, with a project revision
or preview ID and base revision. Findings include the target and threshold,
source, severity, and either the measured position/net or a missing reference.

Each finding has `status` of `pass`, `fail` or `unresolved`. A `hard` failure
or unresolved reference appears in `blocking_requirements` and `design_apply`
returns `refused`. `warning` reports the same evidence without blocking. A
requirements edit makes older previews stale. Direct schematic editing and
import can still change the model, with `requirements_check` available for
review. The requirements cover symbol origins and circuit membership, with
ERC, placement geometry and board validation handled by their own tools.

Foilfern saves the document in `project/foilfern-schematic-requirements.json`
using integer nanometers. The native project files keep their usual format.

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
renamed into place; the result reports `method: zip|git`). Named requests are
settled independently: unknown names do not prevent valid requests from
installing, and failed downloads do not hide successful installations. Results
include ordered `requests`, `failures` and `dependency_failures`; any failure
produces `outcome:partial`, including an all-failed batch. Successful native files
are retained and rescanned. A downloaded library can be listed under `installed`
while its request is partial because a dependency failed; the dependency report
must be checked before using it. A failed list fetch remains a tool error.
`part_import` (exactly one of `lcsc` or `mpn`,
`cache: prefer_cache|offline|refresh`, `require_model`)
acquires an exact supplier part into `SupplierParts.lplib` in the workspace's
local libraries. It returns full device/component details (including signal/pad
mapping), a ready `add_component` device/part entry, source identity and hashes,
and explicit model availability. Source JSON and STEP snapshots are retained for
offline replay. Unsupported geometry or incomplete mapping fails; an unavailable
or invalid STEP gives a partial result unless `require_model` is true. STEP
acceptance does not imply verified footprint/model alignment. Existing native
elements, including manual edits, are retained on repeat; changed source/model
snapshots cause a conflict rather than an overwrite. Downloads and CAD conversion
run outside the session lock; installation rechecks the workspace and writes the
four elements and (when needed) containing library in one filesystem transaction.
MPN resolution uses the tscircuit JLC index, retains its source snapshot, refuses
fuzzy/ambiguous/possibly truncated results, and independently checks the MPN in
the authoritative CAD source. Candidate LCSC IDs are reported on ambiguity.
`dry_run:true` converts and returns symbol/footprint PNGs and mappings without
installing native elements; verified cache snapshots are retained. Its `install`
object is a ready `part_import` request using offline snapshots and
`expected_source_sha256` plus, when available, `expected_model_sha256`. Changed
hashes fail before installation; a model hash implies mandatory validated STEP.
The images are 2D geometry reviews; `model.alignment_verified` remains false.

`library_rescan`, `library_import` (EAGLE `*.lbr` or KiCad libraries
into a local workspace library or a library directory, like upstream's
import wizards: `dry_run` lists the elements with selection and
"already imported" state, `elements` selects some with their
dependencies, options name prefix, "EAGLE/KiCad Import" categories and
additional categories; rescans the index; implementation in
`foilfern-import`). `project_create` imports an EAGLE project with
`eagle_schematic`/`eagle_board` (upstream new project wizard with EAGLE
import; a delegating host gets both paths in `ProjectCreateRequest`).

For automatic official KiCad acquisition, call `library_import` with
`format:"kicad", source:"official:Interface_USB:CH340G"`. `official_release`
defaults to the tested 10.0.8 release, resolved independently to exact commits
for symbols, footprints and models. The acquisition supports split symbol files
and older aggregate files, fetches the selected symbol's parent and exact default
footprint, and validates available STEP geometry. `official_footprint` selects an
explicit `Library:Footprint`; `source_cache` accepts prefer_cache/offline/refresh.
Source bytes, hashes and original license documents are retained. A native
"KiCad Imports" collection is created by default only for actual imports; dry
runs create no native library. Results include `official_source` provenance and
installed device details, with `add_component` when exactly one device resolves.
Missing/invalid optional models are reported explicitly as partial. The existing
native importer preserves already imported elements: refreshing source caches is
not a native update operation. Cyclic/deeper-than-one-level inheritance is refused;
the existing importer remains the format compatibility boundary. No 3D alignment
or datasheet correctness is inferred from successful acquisition.

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
`design_rules_get`, `design_rules_preview`, `design_rules_set`, `planes_rebuild`, `autoroute` (built-in router for
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

`export_bom` defaults to the first board, including its device package and part
information. Projects without boards fall back to a generic schematic BOM.
Use `schematic_only: true` explicitly for a generic BOM in a project with boards;
it cannot be combined with `board`. The response identifies the selected board
UUID (or null for the generic BOM).

`drc_run` includes an `objects` array on each finding. Copper-pair clearance
findings identify both objects. Graphic width and silkscreen clearance
findings identify the affected polygon, circle or stroke text. Footprint
graphics include `device_uuid` to identify the placed instance.

Escape hatch: `mutation_apply` (core `Mutation` JSON, full validation, one
undo group), `mutation_schema`.

### Board manufacturing profile

`design_rules_get` reads the selected board and returns its UUID, revision,
and the editable `profile`. The profile covers native board design defaults,
DRC settings and selected board thickness, copper layers, solder resist
and silkscreen. Manufacturer source records and opaque native DRC options
are reported alongside the profile and carried through edits. Board size,
permitted thicknesses, layer count and colors describe manufacturing
capabilities stored in the native profile. The DRC preview reports checks
implemented by native board DRC.

`design_rules_preview` and `design_rules_set` accept the same flat patch.
For example, for a process that supports the following limits, pass
`{"min_silkscreen_width":0.12,"min_pth_slot_width":0.6}`.
The preview compares full-board DRC before and after on a snapshot and
returns `profile_changes`, the resulting `profile`, and introduced and
resolved counts by finding kind. Check `drc.complete` before relying on
the delta. Preview leaves the project revision, undo history and approved
findings untouched.

Call `design_rules_set` with the same patch, the preview's `board` UUID
and `expected_revision` set to its `base_revision`. Changes form a single
undo step. Omitted fields are preserved. For profile reuse, pass the
`profile` returned by `design_rules_get` as the patch for another board.
Use `project_save` to persist the values in native board files.

Lengths are in mm. For bounded mask, paste and annular-ring ratios, use
`{ratio_ppm,min_mm,max_mm}` (250000 is 25%). For board dimensions use
`{width,height}`. Layer arrays take native layer IDs such as `top_legend`.
`solder_resist:"none"` selects exposed copper. In allowed color lists
`"none"` permits an absent coating or legend. Empty capability lists
leave the corresponding choice unrestricted.

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
segment-scoped via anchors. `layer_changes` counts transitions between
consecutive trace layers. A via touched only on one layer is a via anchor
without a layer change. Lengths follow trace centerlines between anchors,
including pad centers; via barrel length is excluded. Footprint pads are keyed
by component instance and pad, local anchors by segment and layer. Only valid
via/pad copper joins layers; a junction is not an implicit via.

With `suggest_via_reduction: true`, measured paths also return
`via_reduction` proposals. Each is a pair of transitions with uniform-width
leads on the same outer layer and no explicit branches at the two vias or
interior junctions. It includes the fixed outer anchors and a revision-bound
`region_request` for `routing_region_prepare`, with exact rip-up UUIDs.
The request is a trial; inspect its connectivity, DRC and path measurements
before selecting a candidate.

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

1. Built-in grid router: the pure algorithm in `foilfern-autoroute` (A* on
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
   (`foilfern_editor::FreeroutingRouter`: export with a manifest, run the
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

Read `librepcb://guides/board-design` for the workflow and
`librepcb://examples/esp32-c3-devboard` for a complete JSON input to
`design_preview`. The acceptance test reads and applies the same resource.

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
default font for rendering come from `foilfern_scene::resources`: a
resources directory on disk (`LIBREPCB_SHARE`, else `../share/librepcb`
next to the executable) wins; otherwise the fonts embedded into the
binary at build time from `$LIBREPCB_UPSTREAM_DIR/share/librepcb/fontobene`
(nothing is copied into this repository) are used, so an installed
`foilfern-mcp` works on machines without the upstream checkout.

## Tests

- Unit tests per tool adapter on an in-memory session.
- End-to-end test over stdio (rmcp client): create workspace, install/scan a
  test library, create a project, add components, connect nets, place
  devices, set outline, autoroute, add a ground plane, run ERC/DRC, export,
  save, and open the result with the official `librepcb-cli` (ERC/DRC
  clean, Gerber export works).
- The `stdio_esp32_c3_schematic_acceptance` replay uses the published
  [dev-board document](examples/esp32-c3-devboard.json) from an empty project.
  Set `LIBREPCB_TEST_LIBRARIES_DIR` to the directory containing the four
  listed `.lplib` libraries and `LIBREPCB_CLI` to the official CLI for the
  upstream ERC check, then run
  `cargo test -p foilfern-mcp --test mcp stdio_esp32_c3_schematic_acceptance`.

### Reviewed builtin routing

#### Saved board requirements

`board_requirements_get` returns the version 1 `document` and current
`revision`. Pass the full replacement `document` and that
`expected_revision` to `board_requirements_set`. Each rule has a unique
`id`, a nonempty engineering `source`, `severity` (`hard` or
`warning`), a `board` and a `check`. Rewriting an equivalent document
is a no-op. An empty list clears the asset. Save the project to persist it.
The edit is undoable and invalidates earlier candidates.

| `check.kind` | Fields |
| --- | --- |
| `device_region` | `component`, inclusive board-mm `min` and `max` for the footprint origin. Equal corners fix the position. |
| `pad_distance` | `from`, `to`, `max_distance_mm` for straight-line distance between physical pad centers. |
| `trace_path` | `from`, `to` and optional `net`, `max_length_mm`, `max_layer_changes`, `layers`. Measures the shortest anchored trace path. |
| `net_routing` | `net`, optional `layers`, `allow_vias`, `priority`. Hard layer sets intersect existing request limits and other saved sets. A hard `allow_vias: false` forbids vias on this net. Priority uses the builtin critical-net tier. |
| `protected_copper` | `object`, the UUID of an existing trace or via. A hard rule excludes it from suggested and authorized removals. Candidate checks compare its native properties and resolved geometry to the source. |

Board, device and net names may be used to author a document. Pads accept
a label such as `C5.1` or an object with `component` and `pad`. Reads
return normalized UUIDs and native copper layer IDs. With an omitted
trace-path net, the pads must resolve to the same assigned net.

`board_requirements_check` reports measurements at the live revision.
Routing validation reports `board_requirements_before` and
`board_requirements_after`. Findings carry the source, normalized check
in native nanometers, and measurements in mm where applicable. A hard failure or missing reference makes a
candidate inapplicable. A trace path that is still unwired is `pending`
so incremental routing can proceed. Removing an existing anchored path
fails its requirement. A pending path still needs closure for board
completion. Warning findings provide evidence and priority remains a
search preference. Trace-path rules measure explicit traces without plane
copper, via barrel length, or unanchored copper contacts.

The builtin search, staged routing, joint repair and placement comparison
inherit saved rules. Placement and path outcomes are measured on the
candidate instead of rerunning with relaxed constraints. Review and apply
still require explicit copper removal groups. Auto backend selection uses
builtin when saved net-routing rules are present. Explicit Freerouting
search requires a board without those rules, and Specctra import is checked
against the saved rules before commit. The project asset is
`project/foilfern-board-requirements.json` and uses integer nanometers.

#### Routing candidates

`routing_prepare` accepts builtin board/nets/layers and optional dimensions in
mm, plus `expected_revision`. Requested and default trace widths are raised to
the board and applicable net-class minimum widths. It captures an isolated native snapshot, releases
the host locks, runs builtin search and before/after DRC, and retains the exact
mutation candidate only if its source project session and revision still match.
It does not edit the live board or its revision, dirty state, undo/redo or files.
The builtin router connects coincident same-net round-rect SMT pads of a
single footprint before grid routing. It uses the pad's copper layer and a
native trace between the two pad UUIDs when the trace width fits within both
pads. The saved trace preserves connectivity in LibrePCB. `trace_add` accepts
the same connection when given both pad anchors.
The optional nonnegative `via_cost` expresses the cost of a layer change
in equivalent millimeters of trace, with a default of 2 mm. The same
preference is available in comparisons, regional routing and post-edit
continuation, and is shown in the rule provenance from `routing_diagnose`.
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
An optional `route` with explicit target nets uses the same builtin options
as `routing_edit_prepare`. It routes remaining connections after the
explicit changes and a plane rebuild. The continuation preserves copper from
the staged board, including new escapes. Preview returns both the explicit
edit counts and the builtin result. Apply and undo cover the combined patch.
Use only `base_candidate` and `route` to continue routing after a checked
neighbor repair. The intermediate candidate stays in preview and the combined
candidate is checked against the original live board.
Candidate validation reports new DRC warnings by identity as well as errors.
Review warnings such as undersized drills before applying; warnings alone
do not block a candidate.

`routing_placement_compare` accepts one or two explicitly authorized unlocked
`targets`. Give each a list of absolute board `positions` and optional
`rotations` in degrees. Omitted rotations preserve the device's current angle.
The Cartesian product is limited to 16 poses and is tried in request order.
Every trial uses the same `copper` template and explicit continuation nets.
Omit `copper.placements` and `copper.base_candidate`. Each `via_rules` entry
can attach a labeled through via to a moving device, interpreting that via's
`position` as a footprint-local offset with the existing side and the trial's
rotation. An optional same-net `plane` UUID requires the via center to lie
inside connected copper of the rebuilt pour. The response reports the point
and result for each required contact.

Each `trace_requirements` entry contains a `requirement` with a pad pair
(`path.from`, `path.to`) and optional `max_length_mm` and
`max_layer_changes`. A measurable explicit path is required. Set `minimize`
on the paths whose lengths should contribute to the ranking. Native quick DRC
can reject staged copper clearance failures before routing. Every retained
candidate passes full DRC relative to the source, preserves per-net
connectivity, introduces no courtyard conflicts and meets all supplied path
and plane requirements. Ranking prefers fewer air wires, then shorter summed
objective paths, then fewer new warnings. The report includes baseline trace
measurements, each tested pose, rejection evidence, and the number of full
checks. It retains the best one to three patches (default two). The default
30-second budget is checked between trials and can be increased to 120 seconds.
Use `routing_preview` and `routing_apply` to review and apply a patch or
`routing_discard` to free its slot. Unlisted copper and placements are protected.

`routing_placement_start` accepts the same copper template and requirements.
For each target, give explicit `positions` or a `region` with a positive
`pitch_mm`. The region bounds footprint **centers**. Grid coordinates start
at `region.min` and advance by the pitch up to `region.max`, inclusive
when it falls on the grid. Up to 64 center/rotation choices per device
form at most 4,096 joint poses. The worker tries at most `max_trials`
(default 16, maximum 64). When a region is used it orders poses by the
summed straight-line distance of `minimize` pad pairs, then distance from
region centers, and keeps grid order for ties. Actual routing, copper
clearance and engineering checks determine which of the trials are viable.

`avoid_blockers: true` opts into extra in-region trials when quick DRC
finds a copper-pair clearance between a moving footprint pad or a
device-relative via and fixed copper. For a rejected grid trial, the
worker proposes up to two offsets away from the fixed trace, via or pad,
and interleaves them with ordinary grid trials within the same budget.
The offsets can fall between grid points. A pose keeps its allowed rotation,
and new trials preserve all original copper permissions. `total_poses`
counts the grid combinations. With obstacle guidance, `planned_trials`
is the upper limit including suggestions.

Each comparison trial has a `status` of `rejected`, `valid_unretained` or
`retained`. Quick DRC conflicts include object UUIDs, their owner when
applicable, location bounds in millimeters and known copper-pair layers.
A via created on the detached board includes its index into `copper.vias`
for identification across trials. A suggested pose's `suggested_by` pair
indexes its parent trial and that trial's `clearance_conflicts` in the
report, counting from zero. All patches still require full validation.

With `copper.route.search_evidence: true`, a placement trial that ran
builtin includes up to eight `route_failures` with original anchors,
reasons and at most eight exact-checked grid samples per failure. Rejected
poses carry this evidence in the comparison and need no candidate UUID.

Pass the returned UUID as `search` to `routing_placement_status`.
It reports the stage, current poses, planned and completed trial counts,
elapsed budget and final stop reason. Set `cancel: true` on status to
request a cooperative stop. The budget and cancellation are checked during
builtin search, at DRC check boundaries, and between detached work stages.
Snapshots, started DRC checks, plane builds and plane diagnostics can finish
before the worker stops. Completed validated patches can be published after
cancellation or budget expiry. An interrupted trial yields no patch.
`stop_reason` reports `completed`, `trial_limit`, `time_limit`, `cancelled`
or `failed`. `comparison` is available at finish if the worker completed
its baseline and the source revision, session and save context still match.
The search reserves one to three candidate slots before starting. One
placement and one repair search may run per project, within the shared
candidate capacity. Starting the next placement search replaces the status
of the previous one. Published candidates retain their normal lifecycle.

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
`recommended` ID. With `trials`, choose two or three distinct combinations
of `order` and optional `via_cost` instead of the `orders` list. Each result
reports the effective `via_cost_mm`. Ranking compares acceptance, new and remaining
non-connectivity errors, nets whose air-wire count regressed, remaining
critical and total air wires, unmet trace preferences, via count, then total trace centerline length
in mm. Metrics describe the resulting whole board; existing copper is common
to all trials. Caller order breaks ties. A candidate with unacceptable
validation or a connectivity regression is never recommended. Inspect any
candidate with `routing_preview`; there is no automatic apply.
Optional `trace_preferences` accepts up to 16 physical pad pairs. Each entry
has a `path: {from, to}` and at least one of `max_length_mm` and
`max_layer_changes`. The shared result identifies resolved pads and each
candidate reports the shortest anchored path's measurements and whether
it meets both supplied limits. Missing measurements count as unmet for
ranking. These preferences do not alter the route search or the apply gate.

`routing_staged_prepare` groups the explicit, nonempty `options.nets`
allowlist into one candidate. Pass the other `routing_prepare` settings,
including `expected_revision`, under `options`. Critical nets are queued
first. Within each tier, nets with shorter longest initial air wires go first.
`max_gap_mm` optionally defers non-critical nets above that length.
After each trial, the worker rebuilds planes and air wires and runs full DRC.
It retains progress on the selected nets when there are no new
non-connectivity DRC errors or air-wire count increases on any net. A
rejected group is rolled back. Rejected or unproductive groups with multiple
nets are retried as halves. Existing trace and via geometry and UUIDs are
preserved.

Top-level limits are `batch_size` (1–32, default 8), `max_trials` (1–64,
default 24) and `max_time_ms` (1–300000, default 120000). The time budget
is checked between trials. The `staged` result gives each trial's net IDs,
outcome and validation. Each trial reports the count of failed router
requests and up to eight samples with reasons and anchors. Use
`options.search_evidence` to include the sampled blockers for those searches.
`stop_reason` describes why the queue stopped. The `deferred_nets`,
`pending_nets` and selected `remaining_nets` lists report IDs, names,
longest initial air-wire lengths in mm and remaining air-wire counts. A completed queue
can still leave open connections. Review and apply the retained batches as
one candidate with the usual preview and apply tools.

`routing_preview` reads candidate metadata and exact mutations with `offset` and
`limit` (1–200, default 100), `next_offset` and a `stale` flag. This is a data
preview; GUI overlays and candidate PNG rendering are future work. Retained storage bounds do not limit
the number of concurrently running preparation workers.
Supply `trace_paths` with up to 16 pad pairs to measure the shortest anchored
trace paths before and after the frozen patch, including length, layers,
via anchors and layer changes. Pad identities are resolved on the source
board. A missing trace-only path reports `not_measured`. Combine this with
`reference` to compare ground coverage in the same preview.
With `plane_probes: {plane, points}`, compare the native fill status of up to
16 fixed board points on one plane before and after the candidate. Each point
reports the same classification as `plane_diagnose` or null outside every
fragment.
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
and uses the shared preview and apply tools. With `improve_net`, apply also
requires a decrease in air wires on that target net.

`routing_diagnose` accepts an optional `repair` object for a supported air
wire of the selected net. Use `air_wire` to select a zero-based connection
(default 0), `region` for new copper, `rip_up_region` for the smaller edit
area and `editable_nets` for the neighboring nets that may be considered.
`max_objects` caps the number of traces and vias per proposal (1–16,
default 8). The two endpoints must fit inside `region`.

The response identifies blockers sampled by exact endpoint clearance probes.
An optional `evidence_candidate` adds original copper IDs from failed
builtin or staged searches for the same connection, board and revision.
It also accepts an additive `routing_copper_prepare` candidate prepared
directly from the source revision with a builtin continuation. Set
`route.search_evidence: true` in that request. Diagnosis follows the
candidate's copper to the failed continuation's anchors, such as a new
escape via, and reports matching attempts in `continuation_failures`.
Ambiguous matches are omitted.

Alternatively, set `repair.evidence_search` to the UUID of a finished
`routing_repair_start` search on the same board revision and target net.
For additive searches with `target_launch_search` and `route.search_evidence`,
diagnosis follows failed continuations from the generated vias back to the
requested air wire. Its `escape_sites` array contains matching failures,
sampled blocker UUIDs and a `target_launch` in board millimeters ready to
reuse in a new request. A proposal's `escape_sites` indexes this array.

For repairs rejected by whole-board validation, `validation_failures`
reports the failed trial's strategy, removals, launch if present, exact
copper-pair clearance findings and any sampled new gaps on neighboring
nets. `source_copper` lists the traces and vias in those conflicts that
already exist in the live board and were outside the trial's removal list.
A proposal's `addresses_clearance_in_trials` lists the zero-based trial
indices for which it covers an existing blocker. It is a next scope to
review. Use the conflict details to choose another exit if the conflicting
geometry was generated during the failed trial, or authorize an additional
branch if a fixed trace or via is eligible for replacement.

Finished repair evidence also includes `screening_failures` when the local
escape screening found no sites, including a neighbor screened against an
already staged target launch. These entries identify the pad, removal scope,
and bounded probe blockers that still belong to the source board. Local
probes suggest scope to inspect; they do not prove that removing it will
make a via or a continuation legal. In search status the same failures are
available even if no routing trial ran. `routing_failures` retains the
failed endpoints and bounded frontier evidence from target and neighbor
continuations, rather than only their net names and reason strings.

Search evidence includes bounded samples at multiple locations and layers
along a failed frontier. Proposals supported by continuation samples appear
before those based only on the original pad probes. The planner follows
eligible traces through two-trace junctions, stops at branches and fixed
anchors, and proposes removing a via only when all its attached traces fit
the permission and bounds. It returns up to eight proposals with exact
UUIDs and retained anchors, ordered by affected copper count within each
group. When a proposal forms one branch between two retained anchors, it
includes `neighbor_anchors` for a single-group `routing_repair_start`
request. When all removals fit the same object limit, the response also
gives their union as `combined_region_request`.

Review and optionally edit a proposal's `region_request`, then send it to
`routing_region_prepare` to authorize those exact removals and prepare a
checked candidate. Diagnosis only proposes scope. To choose a candidate,
check the requested connection, full DRC and the rebuilt connectivity.
For a two-stage repair, route the neighbors first by omitting `improve_net`
and using only their nets in `route.nets`. Pass that candidate and the
original target net to `routing_copper_prepare` for continuation. Apply only
the reviewed combined patch.

`routing_repair_start` runs local alternatives without editing the live
board. Supply a `target` net and one to eight `groups` of trace and via
UUIDs explicitly authorized for removal. The lists from
`routing_diagnose.repair.proposals[].region_request.rip_up` are suitable
inputs after review. Include all attached traces in any group containing a
via. The service deduplicates groups and pairwise unions. It tries singles
before pairs and smaller removals first within each tier.

Use `rip_up_region` to bound original copper removals, `neighbor_region`
to bound replacement copper (defaults to `rip_up_region`), and `region`
to bound new target copper. Both routing rectangles must contain
`rip_up_region`. Pass independent builtin settings under `neighbor_route`
and `route`. The selected group determines neighbor nets, and `target`
determines the target net. Include `expected_revision` to bind the search
to the reviewed revision.

`neighbor_anchors` optionally supplies an exact retained pair for a removal
group, for example
`[{"group":0,"from":{"junction":"<uuid>"},"to":{"pad":"R2.2"}}]`.
The zero-based group must contain one continuous branch with no third retained
attachment point, including any via removed from its interior. Endpoints can
also be `{"via":"<uuid>"}` or `{"board_pad":"<uuid>"}`. A junction endpoint
must still have copper attached after rip-up. The router reconnects these
two anchors. Other open connections of that neighbor net remain outside
the trial. Both endpoints and all replacement copper must fit the routing
bounds. A pairwise trial skips combining a constrained group with another
removal on the same net. Trial status and preview report `neighbor_branches`.

`joint_route` optionally adds a joint pass with its own builtin settings
for the same target and neighbor nets. The sequential pass routes
neighbors first and rebuilds connectivity before routing the target.
The joint pass searches within `region` and also checks that all new
neighbor copper fits `neighbor_region`. Both use the exact same removal
set and `rip_up_region`. Each attempt starts from the same detached
snapshot. When enabled, the joint attempt follows its sequential attempt
for each set, and the status and candidate identify their `strategy`.

`max_objects` caps each removal set (1–16, default 12), `max_trials`
counts attempts across all sites and both strategies (1–72, default 24), and
`max_candidates` reserves up to three candidate slots (default 2).
Only complete checks, no new errors, no net connectivity
regression, and fewer opens on the target produce a candidate. Candidates
are ranked by target opens, new warnings, affected objects, board via count
and board trace length. Use measured preview for pad-pair path costs.

`target_launch` optionally adds a straight lead from a named footprint pad on
the target net to one explicit through-via. Supply `from`, `layer`, the via
`position` in board mm, the lead `width`, and optional `drill` and `size`.
The lead and via are inside the target region. In the sequential pass the
launch is staged after neighbor routing, before the target continuation.
In the joint pass it is staged after rip-up and before both nets are routed.
It stays fixed during routing and is previewed and applied as part of the
same checked patch.

`target_launch_search` is an alternative to giving an exact `target_launch`.
Supply a target-net pad and its copper layer, for example
`{"from":"U1.12","layer":"top","radius":1,"max_sites":6}`. Within the
radius (at most 1.5 mm) the worker screens straight leads and through vias
on a 0.025 mm site grid, using the target `route` dimensions after applying
the builtin minimum rules. For each trial group or pair of groups, it
screens sites on a detached copy with those exact UUIDs removed, using
native clearance thresholds. It keeps up to eight sites per group,
shortest lead first, and cycles through the groups before trying their
next sites. The worker tries the routing strategies and ranks candidates
using the complete board checks and path requirements.
With this option, omit `groups` and `rip_up_region` to try an additive
continuation. The `launch_sites` status lists the union of sites that passed
screening even if no continuation succeeds. Each trial, choice,
and preview identifies its exact `target_launch`.
For blocked continuations, pass the finished `search` UUID as
`routing_diagnose.repair.evidence_search` to inspect the corresponding
neighboring copper before authorizing removals.

For two pads competing for the same escape corridor, add
`neighbor_launch_search` with the same fields and supply `joint_route`.
The neighbor pad must belong to a different net with an authorized removal
in the trial. Its group must allow ordinary continuation on that net, so
omit exact `neighbor_anchors` for it. This mode runs joint trials only.
Both site searches use the dimensions from `joint_route`. The neighbor
lead must fit `neighbor_region` and clears the staged target lead and via.
Sites on each pad are spaced apart so a small budget can explore different
escapes. Each pair counts as one trial. Status and preview report the
chosen coordinates as `target_launch` and `neighbor_launch`. The candidate
still needs explicit application.

In paired searches, a checked candidate can also discard vias created by
the trial that DRC marks as useless. The worker checks the resulting board
and pad-pair requirements again and reports any discarded IDs in
`pruned_vias`. Existing vias require explicit authorization. A
`UselessVia` finding includes its via UUID in `objects` for review.

`trace_requirements` adds up to 16 hard requirements measured on each
fully checked repair. Each entry names a pad pair under `path` and may
specify `max_length_mm` and `max_layer_changes`. The anchored path must
exist even when limits are omitted. A trial that closes the target but
misses a requirement is reported with its before and after measurements
and does not publish a candidate. Preview includes those measurements
for retained candidates. This is a per-search contract.

Poll `routing_repair_status` with the returned `search` UUID. It reports
the phase, budget, checked trials, failure samples and the retained candidate
IDs after the worker stops. Set `cancel: true` on that call to request
cancellation. `max_time_ms` (1–300000, default 120000) and cancellation
are cooperative. A* polls at each connection and every 1,024 expansions.
DRC skips checks that have not started and joins checks already running.
In-flight model preparation and plane fill finish before the worker stops.
Trials whose validation has completed remain available on time limit or
cancellation if the source is still current. Edits, save-context changes or
project replacement prevent stale publication. A subsequent search replaces
the previous status while published candidates keep their normal lifecycle.

See [routing service design](routing-service-design.md) for subsequent stages.
The existing `autoroute` compatibility path continues to apply directly.

Declarative `add_components` can select an instance-specific orderable `part`
with non-empty `mpn`, optional `manufacturer` and string `attributes` (e.g.
`LCSC`). Component string `attributes` are separate. Selection requires a device;
the reserved `FOILFERN_DESIGN_KEY` cannot be supplied. Replaying a supplied part
or attribute that differs from the current instance reports a conflict. Omitted
fields preserve current selections. `component_get.assembly_options` exposes
native selected parts. `export_bom.additional_attributes` appends columns to the
project defaults; `LCSC[]` exports the per-part LCSC attribute.

## Local calls without an MCP client

The standalone binary dispatches through the same tool registry and validators:

```sh
foilfern-mcp call server_info '{}'
foilfern-mcp --project /path/to/project.lpp call export_bom '{"additional_attributes":["LCSC[]"]}'
foilfern-mcp --project /path/to/project.lpp call design_preview @design.json
foilfern-mcp --project /path/to/project.lpp batch calls.json --save
```

`call TOOL JSON` accepts inline JSON, `@file`, or `-` for stdin. The default
arguments are `{}`. `batch FILE` accepts a JSON array of `{tool, arguments}`;
`arguments` defaults to `{}`, and `FILE` may be `-` for stdin. The complete file
is parsed before the first call, then calls run sequentially in a shared session.
Each operation retains its usual transaction/undo boundary. A batch is not one
atomic transaction: explicit saves and exports performed before an error persist.

Stdout is JSON Lines, one `{index, tool, response}` per executed operation.
`response` is the ordinary MCP CallToolResult, including structuredContent,
text, warnings and base64 image blocks. Logs and diagnostics go to stderr.
Protocol dispatch errors have `response.isError: true` and an `error` object.
Malformed input/setup errors print a diagnostic and return a nonzero exit code.

By default execution stops at the first failed call. `--continue-on-error` runs
remaining calls but still exits unsuccessfully. Saving is explicit: use the
`project_save` tool or `--save`, which appends a save after successful calls if a
project is open (and emits its result as another JSON line). A failure prevents
this final automatic save. Remaining unsaved changes cause a stderr diagnostic
and nonzero exit status. No implicit save happens on failure or process exit.

A singleton object `{"$ref":"INDEX#JSON_POINTER"}` in batch arguments selects a
value from an earlier raw `response`. References are zero-based and can address
only completed earlier calls. For example, after a design preview at index 0:

```json
[
  {"tool":"design_preview","arguments":{"version":1,"schematic":"Main"}},
  {"tool":"design_apply","arguments":{"preview":{"$ref":"0#/structuredContent/result/preview"}}},
  {"tool":"project_save"}
]
```

`$ref` singleton objects are reserved in local arguments. Other argument objects
are passed through unchanged. A missing or forward reference fails that call.
`--workspace` and `--project` work before or after the local subcommand. HTTP
serving and local execution cannot be combined. No client, subprocess bridge,
network transport or protocol initialization is required; the SDK peer context
is local plumbing for the same dispatcher. Interactive client requests are not
supported in this mode.

For declarative `add_components`, `library_component` is optional when `device`
is supplied. The device's native component reference is resolved while preparing
the transaction. An explicitly supplied component must still agree with the
device. Omit `device` only for schematic-only components, which must name their
`library_component`.

`library_search` with `exact: true` resolves a unique exact name, device MPN or
UUID (case-insensitive names/MPNs). It returns full element detail in the single
`elements` entry, including matching library parts under `matched_parts` for an
MPN query. An empty query fails; zero matches are `not_found`, multiple matches
are `conflict` with candidate UUIDs. `limit` does not truncate exact resolution
or hide ambiguity. Specify `kind: "device"` when resolving physical parts. Normal
keyword search remains the default. Project library objects take precedence over
workspace objects with the same UUID.

Device detail includes package `models` with UUID, name, STEP filename and an
`available` flag indicating file existence, not model validity. Footprints expose
model IDs plus their native model_position in mm and model_rotation in degrees.
Pad/signal mappings and footprint pad geometry are returned in the same response.

`board_legend_list` returns each text's native `uuid` and a complete `target`
(`board`, optional component-instance UUID, text `uuid`). Footprint text UUIDs
identify templates and can repeat across instances; pass the whole `target` to
`board_text_move`. Legacy `board`/`component`/`uuid` calls remain accepted, but
cannot be mixed with `target`. `text` preserves the native attribute template;
`rendered_text` resolves it with the same part/device context as rendering.
`bbox` is the transformed stroke envelope in board millimeters, including half
the stroke width on each side; it is null for empty text. Rotated and mirrored
instance texts are already in board coordinates and are not transformed twice.

A board layout can contain `legend_placements` without device `placements`.
Each entry contains a complete `target` from `board_legend_list` and `candidates`
(an ordered list of `{position:{x,y}, rotation?}` in board mm). Up to 16 texts and
32 poses per text are accepted. The first clear pose wins. The preview reports
each attempted pose's mask, board-edge/cutout, courtyard and text-overlap checks,
the chosen text and the existing full before/after DRC validation. Checks use
native font strokes and exact DRC regions. Mask and board-edge clearance use the
configured silkscreen clearance. Courtyards fall back to package outlines with
explicit component IDs; devices with neither geometry block the search. This
checks the supplied library geometry, not whether it accurately models a body.
The final board is rechecked after authorized routing repair. No valid pose
blocks apply; the candidate image and evidence remain reviewable. Successful
moves use the normal revision-bound patch, single undo group and replay behavior.

`plane_add` without `outline` copies the board's one closed outer contour,
including native arcs and footprint-provided board contours. It refuses missing
or multiple outer contours and asks for explicit vertices. Native cutouts stay
separate and are subtracted by the ordinary plane fill. The editor's interactive
automatic drawing gesture still uses the upstream enclosing rectangle.

`jobs_configure` creates or updates native output jobs using `step`,
`schematic_pdf`, `board_assembly_pdf` or `board_rendering_png` presets. New jobs
require a preset; provide a stable `name` for replay. Omit `job` to upsert by
that name, or use an existing exact name/UUID to update or rename it. Existing
UUIDs, forward-compatible options, job order and unspecified settings stay
intact. A supplied preset asserts the existing content type instead of resetting
custom settings. `output_path` supports native placeholders and validates the
exporter's extension; `board` selects one board for STEP or board graphics.
Identical configuration produces no mutation. Changes use ordinary revision
checks and one undo group. `jobs_list` stays compact by default; `job` filters by
exact name/UUID and `include_settings:true` returns complete native settings.
Ambiguous names are rejected in listing, configuration and execution. Exporting
files remains a separate `jobs_run` operation.

The SDK is rmcp 3.10.0. Transport acceptance explicitly covers legacy
`initialize` with MCP 2025-11-25 and modern `server/discover` with MCP 2026-07-28
over stdio and HTTP. Both expose the same complete deterministic tool catalog.
Application project handles remain shared across HTTP requests and clients.
Local CLI calls still dispatch through the registry without a protocol handshake.
Deferred insertion of tool definitions into a model's context is a client/host
concern, as described in the official client best practices; no server tool
profiles or session-based tool activation are provided. Schema or response
reductions require a separate agreed change.
