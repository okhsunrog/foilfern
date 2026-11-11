# Designing a board through MCP

This guide is available from the server as `librepcb://guides/board-design`.
Use `tools/list` for the exact schemas of the running version. The server
speaks millimeters and degrees, with the y axis pointing upwards. Each
write tool is one undo group shared with the desktop application.

## Start or resume

Call `server_info` first. If a project is already open, inspect
`project_summary`, `history`, `schematic_list`, `component_list` and
`board_get` before changing it. An embedded server edits the user's active
project; opening another project is a lifecycle operation, not a prerequisite
for every task. Use the existing session instead of launching a standalone
server against a project already open in the application.

For a new design, open or create a workspace with `workspace_open` or
`workspace_create`, then call `project_create`. It creates a first page
and board by default. A workspace is needed to discover and install parts;
projects keep their own copies of library elements.

Keep the requirements explicit: circuit function, supply voltages,
interfaces, component constraints, dimensions, layer count and fabrication
rules. Ask for missing electrical requirements before making consequential
choices. Derive pin functions and ratings from the part's datasheet;
matching a library name is not evidence that the part fits the circuit.

## Discover parts and build the schematic

For a flat KiCad schematic, call `project_create` with
`kicad_schematic` pointing to the `.kicad_sch` file. The new project uses
the schematic's embedded symbols, placements, wires, labels, texts,
rectangles and images. Power symbols connect matching named nets. Local
unnamed nets receive LibrePCB names. Set `create_board: false` to import
just the schematic. Hierarchical sheets, buses and alternate symbol body
styles are rejected. Check the imported netlist and run ERC.

Add `kicad_board` with the matching `.kicad_pcb` to start a new routing run
from its original placement. This imports embedded footprints on both
sides, the outline, copper layer count and total board thickness, plane
outlines and keepouts. A sibling `.kicad_pro` supplies supported board
rules and exact net-class assignments. Existing traces and vias stay in
the KiCad source. The import checks the numbered pad net assignments
against the schematic and reports conversions that need review.

1. Use `library_list`, then `library_install` for missing official libraries
   (for example `{"libraries":["LibrePCB Base"]}`), or `library_rescan`
   after installing local libraries.
2. `library_search` accepts several words, such as `resistor 0805`.
   Inspect a candidate with `library_element`: signals, device pad mapping,
   footprints, and symbol variants with their gate and pin mappings.
3. Pass the chosen device or component UUID to `component_add`. A name
   search can be ambiguous. Set `device` explicitly when selecting among
   compatible devices; inspect the returned selection and warnings.
   Set `symbol_variant` to a UUID returned by `library_element` when a
   particular symbol representation is required. The default follows the
   project's norm order. All gates of that variant are placed.
4. Add pages with `schematic_add`; select them through `component_add.schematic`.
   Multiple gates of one component must stay on the same page in the current
   model. Use `symbol_move` with `gate` (suffix, index or UUID) to position
   an individual gate. `component_get` lists the actual placed gates and pins.
5. Connect by electrical intent: `{"net":"VCC","pins":["J1.1","R1.1"]}`.
   A pin address uses a pad number or component signal name, not an assumed
   schematic pin number. Inspect the library mapping to resolve ambiguity.
   Supply symbols force their net name. Cross-page connections use net labels;
   they do not need geometry spanning sheets.
6. Use `netlist` and `component_get` to verify actual connectivity, then
   `schematic_tidy` per page and `render` for each schematic page. Inspect
   the images for readability, label collisions and circuit grouping.
   Use `symbol_text_move` with `text: "name"`, `text: "value"`, or a text
   UUID from `schematic_get` to adjust an annotation without moving the
   symbol or its pins.
7. Run `erc_run` and resolve the reported messages. ERC checks structural
   electrical rules; it does not establish that a circuit meets its ratings,
   timing or analog requirements.

## Lay out the board

Set `board_set_outline` and `design_rules_set` before routing. `inner_layers`
counts inner copper layers: `0` means two total layers, `2` means four.
Set clearances, widths and drill sizes from the fabrication requirements.
`net_class_set` assigns routing dimensions to selected nets.
Omit `trace_width` from builtin routing requests to honor the per-net
widths. An explicit value applies to all selected nets. A net class sets
the default for future routing, so use `trace_update` or reroute selected
connections to change existing copper. Check DRC after resizing.

Use `board_polygon_add` for graphics or cutouts: pass a LibrePCB board layer
ID such as `top_legend` or `brd_cutouts` and at least three vertices in
millimeters. The path closes automatically. `board_get` reports the geometry
and IDs of non-edge polygons under `polygons` and board edges under
`outlines`. To connect a copper polygon to a net, use `plane_add` with a
custom `outline`, then rebuild planes.

Use `device_place` to anchor connectors and mechanically constrained parts;
then `device_auto_place` for the remaining components. Inspect `board_get`
and `render` before routing. Automatic placement is a starting point;
decoupling, return paths, thermal layout and sensitive signals need circuit
specific placement decisions.

`autoroute` supports `backend: "auto"`, `"builtin"` or `"freerouting"`.
Auto uses an installed Freerouting executable when supported and otherwise
uses the built-in router. Net and layer filters apply only to the built-in
router. `specctra_export` and `specctra_import` support external routing.
Use `trace_add`, `via_add` and `trace_remove` for targeted routing changes.
Use `trace_update` to change existing widths in one undo step, for example
`{"nets":["VCC"],"width":0.2}` or `{"uuids":["<trace UUID>"],"width":0.25}`.
It preserves trace identities, anchors and layers. Inspect its before/after
validation: increasing width can remove width errors while adding clearance
errors. Undo a regressing change rather than assuming wider copper is valid.
Check `unrouted` after routing. It measures current air wires with rebuilt
planes even immediately after opening a project.

Routing and SES import return `validation` from full before/after DRC on
the applied project. Inspect actual `unrouted_after`, `new_error_count`,
`new_errors`, `netlist_preserved` and `drc_clean`. New errors or incomplete
checks make the result `partial`, even when no air wires remain. Existing
errors stay visible; `complete` can still have `drc_clean: false`. Inspect
the candidate or undo the whole operation before trying another approach.

Add copper planes with `plane_add`, rebuild with `planes_rebuild`, and run
`drc_run`. Use `plane_set_outline` with the plane UUID and replacement
vertices to change the extent of an existing pour. Inspect both board
sides with `render`. Resolve errors and review
warnings; a zero error count alone does not validate the physical design.

`plane_diagnose` runs the normal fill on a snapshot without editing the board.
Pass a plane UUID from `board_get`. The report shows the count and area of
connected, discarded, kept-island and netless fragments at the final island
filter. Each fragment has board-space bounds in mm, with discarded islands
listed first. Pass `probe: {"x": 34.2, "y": 25}` to test whether a point is
inside any fragment, including discarded ones. Use `offset` and `limit`
to page through the report and `expected_revision` to pin it to your read.

For a discarded island, choose a via center using `probe` and an existing
same-net anchor on another copper layer. Pass `plane`, `position`,
`from` and `from_layer` to `plane_stitch_prepare`. Supply `points` for
intermediate lead vertices and `trace_width`, `via_drill`, or `via_size` to
override the editor defaults. The candidate requires the via center to
belong to a connected fragment after fill. Its report gives the fragment
areas at the site before and after, plus full DRC and connectivity checks.
Use `routing_preview` to inspect the exact copper edits and `routing_apply`
to commit an acceptable candidate as a single undo step.

Leave unused islands discarded. Use a stitch when the recovered copper
serves a specific connection or return path. Inspect both layers around a
critical signal before and after the edit. The stitch check confirms the
fill at the via center.

Use `reference_coverage` to measure selected signals over a reference pour.
For the STM32 board, the request is

```json
{
  "reference": {
    "signal_nets": ["SWCLK", "SWDIO"],
    "reference_net": "GND",
    "reference_layer": "bottom"
  }
}
```

The report gives the checked length, uncovered intervals in board mm and
the trace count on unchecked layers. A null `covered_fraction` means there
are no checked traces on adjacent copper layers for that net. The check
uses the rebuilt pour and the trace centerlines. It includes gaps from holes
and clearances, including around signal vias.

Pass the same `reference` object to `routing_preview` for a fresh candidate.
The `reference_coverage` section shows before and after measurements and
the resolved and new intervals per signal. A shorter gap can have a
resolved interval alongside its remaining uncovered portion. Gap lists
have separate `gap_offset` and `gap_limit` fields inside `reference`.
Length totals cover the full lists. Keep using the candidate's DRC and
connectivity checks when deciding whether to apply. See the
[ground coverage and stitch workflow](routing-service-design.md#ground-reference-coverage-and-targeted-stitching).

Use `trace_path_measure` to compare the explicit routed path between two
same-net pads without collecting trace segments manually:

```json
{"board":"default","from":"OLED1.28","to":"C21.1"}
```

The result identifies its project, board and revision, resolved pad identities,
centerline length in mm, layers, traces and unique via anchors. Supply
`expected_revision` when reviewing a particular snapshot. UUID endpoint objects
also work, for example `{"component":"<instance UUID>","pad":"<pad UUID>"}`.
An ambiguous pin refuses; specify its physical footprint pad UUID.

Inspect `status` before using the length. `not_measured` with null length means
no explicit anchored trace path was found, including cases joined through a
plane. It does not mean disconnected or zero length. Plane copper, unanchored
copper overlaps and via barrels are excluded; continue using `unrouted`,
reference coverage and DRC for connectivity and reference checks.

## Save and deliver

After the final edits, rerun ERC, DRC and `unrouted`. Export fabrication
files with `export_fabrication`, plus `export_bom` and `export_pick_place`
when needed. `jobs_list` and `jobs_run` run configured output jobs, including
graphics and STEP. Check the returned files and warnings; an absent job is
not a successful export.

Call `project_save` explicitly. Check `history` for unsaved changes. The
project uses upstream LibrePCB's native format; close it with `project_close`
before opening it in a separate LibrePCB process, which takes the same
directory lock. Report artifact paths, remaining messages, unrouted count,
and which visual or upstream checks actually ran.

## Recover from errors and concurrent edits

Tools return `{outcome, revision, result, warnings}`; tool failures set
`isError` and carry a stable `error.kind`. Treat `partial` and `failed`
as incomplete work and inspect their evidence. A check which did not run
cannot be treated as passing.

Pass the most recently observed project `revision` as `expected_revision`
on writes when working alongside the user. On `stale_revision`, re-read
the affected entities and revise the operation before retrying. Do not
blindly repeat a write after a lost connection: inspect current state first.
Use `history`, `undo` and `redo` to recover whole tool steps. Prefer the
intent tools over `mutation_apply`; use `mutation_schema` before the raw
mutation escape hatch (its length fields use integer nanometers).

## Review builtin routing before applying it

Use `routing_prepare` with optional `board`, `nets`, `layers`, `trace_width`,
`via_drill`, `via_size`, `grid_pitch` and `expected_revision`. It returns an
opaque `candidate`, full validation and remaining connections without changing
the live board. Inspect its exact patch through `routing_preview` (paginate with
`offset`/`limit` and `next_offset`). Check `validation.acceptable`, `drc_clean`
and remaining connections separately. Apply with `routing_apply` and the
candidate ID, or release it with `routing_discard`. Any intervening edit, even
an edit followed by undo, can make a candidate stale. Saving can also invalidate
its text attribute context without advancing the revision; prepare again instead of
changing its revision. Apply preserves the reviewed geometry and is one undo.
Candidates are session-local and disappear on closing the project. This first
stage supports builtin only and returns data previews, without a GUI overlay.

Use `routing_compare` to try several initial orders with the same settings:

```json
{
  "options": {"pad_escape": true, "grid_pitch": 0.1},
  "orders": ["shortest_first", "longest_first", "constrained_first"]
}
```

`options` accepts the same fields as `routing_prepare`, including
`critical_nets` and `expected_revision`. Omit `orders` to try all three.
Compare the ranked candidates' DRC, open connections, via count and total
trace length. Preview the recommended ID or choose another. Applying a
candidate that changes the board makes the other IDs stale; discard them
to free their slots.

Use `neck_down` for short pad escapes on a wider net. For example,
`{"nets":["V3V3"],"neck_down":{"width":0.2,"max_length":1.0}}`
uses the net-class width for the main route and allows up to 1 mm
of reduced width at each pad. The board and net-class DRC minima
raise the requested narrow width when necessary. Inspect
`routing.trace_width_profile` in the candidate for the total length
at each width and the exact reduced-width segments. Profile dimensions
and segment coordinates use integer nanometers, like the mutation preview.

Use `routing_diagnose` with `net` to inspect remaining endpoint access and
local clearance blockers. It accepts the builtin `grid_pitch`,
`trace_width`, and `layers` options. `probe_length` sets the length in mm
of each straight probe. Access points come from the builtin grid. A straight
probe checks local clearance along one direction.

Use `routing_via_prepare` to connect two same-net anchors on different
layers through one via. Pass each endpoint and its layer. For example,
`from: "U1.21"` on `top` can connect to a junction UUID from
`board_get` on `bottom`. Omit `position` for a search around the first
anchor, or provide an exact via center in board millimeters. Inspect
`via_connection` and the patch with `routing_preview`, then apply the
candidate with `routing_apply`. The preview uses integer nanometers for
the via position, bends and dimensions.

Use `routing_edit_prepare` for grouped changes to existing copper. In
`vias`, provide `uuid`, `drill`, and `size` in mm. In `moves`, provide
`kind` (`via` or `junction`), `uuid`, and an absolute `position` in mm.
The same via can be resized and moved in one request. Attached traces
follow the anchor and retain their UUIDs. Review the candidate with
`routing_preview` and commit it with `routing_apply` after checking
`validation.acceptable`. An edit that increases the rebuilt air-wire count
on any net is refused.

In `board_get`, each via includes `effective_drill` and `effective_size`
in mm, using the same net-class and board rules as DRC. The `drill` and
`size` fields show explicit overrides and are null in automatic mode.
