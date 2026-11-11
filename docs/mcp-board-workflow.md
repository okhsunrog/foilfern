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
7. Run `erc_run` and resolve the reported messages. ERC checks structural
   electrical rules; it does not establish that a circuit meets its ratings,
   timing or analog requirements.

## Lay out the board

Set `board_set_outline` and `design_rules_set` before routing. `inner_layers`
counts inner copper layers: `0` means two total layers, `2` means four.
Set clearances, widths and drill sizes from the fabrication requirements.
`net_class_set` assigns routing dimensions to selected nets.

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
Check `unrouted` after routing; a completed tool call does not establish
that every connection was routed.

Routing and SES import return `validation` from full before/after DRC on
the applied project. Inspect actual `unrouted_after`, `new_error_count`,
`new_errors`, `netlist_preserved` and `drc_clean`. New errors or incomplete
checks make the result `partial`, even when no air wires remain. Existing
errors stay visible; `complete` can still have `drc_clean: false`. Inspect
the candidate or undo the whole operation before trying another approach.

Add copper planes with `plane_add`, rebuild with `planes_rebuild`, and run
`drc_run`. Inspect both board sides with `render`. Resolve errors and review
warnings; a zero error count alone does not validate the physical design.

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
