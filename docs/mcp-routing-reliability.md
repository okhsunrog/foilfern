# Routing reliability for MCP design workflows

The STM32G474 experiment on `c1101ac` established a useful end-to-end
workflow and exposed two routing-integration failures. Correctness and
ordinary board editing take priority over design-intent storage, templates,
additional exports and 3D work.

## Reproducible regressions

The original experiment has a frozen native project with its embedded
library elements, the MCP transcript and a manifest of file hashes and
element versions. The case is `stm32g474-c1101ac`, archive SHA-256
`a589d2364be89bb8cefd42ef6f9717379c38e5d5b6569285fcd636eb5f607197`.
It captures the final saved project, including later manual edits, rather
than claiming to be the initial unrouted board. The original experiment
and the snapshot remain outside the repository.

Small regressions use the local upstream populated-library fixture instead
of network searches or copied library data. With `LIBREPCB_UPSTREAM_DIR`
configured, run:

```sh
cargo test -p librepcb-editor autoroute_test
cargo test -p librepcb-mcp --features http routing::
```

The first suite reproduces the automatic/manual via representation problem
for drill 0.3 mm and size 0.6 mm, checks other dimension combinations,
net-class inheritance and DRC limits, and applies a real layer-changing
route. Undo/redo, the actual via geometry, full DRC and optional upstream
CLI acceptance are checked.

The second suite supplies actual SES candidates for a two-net board to
both direct import and the Freerouting completion path. At 0.15 mm width
against a 0.2 mm minimum, the applied candidate has zero remaining air
wires but two new width violations. It must return `partial`, with both
violations identified. A 0.25 mm candidate passes. This test does not need
Java or execute Freerouting; it isolates the import and validation boundary.
The existing external-router suite exercises the subprocess when installed.

`trace_update` changes widths of selected UUIDs or nets as one undo group,
preserving trace identities and anchors. Its regression scenario repairs
the thin SES candidate, then deliberately widens a trace into neighboring
copper. The second change must report new clearance errors and `partial`.
Invalid selections and stale revisions leave geometry and history unchanged;
repeating the same width does not add an undo entry.

## Native STM32 replay

At `9aa62e3`, the frozen final project was copied into an isolated stdio
session without a workspace or network library lookup. After removing its
old routing, a built-in run with 0.1 mm grid, 0.2 mm traces and 0.3/0.6 mm
via dimensions reduced actual air wires from 57 to 1. Full model checks
found zero copper errors before and after, no new non-connectivity errors,
and an unchanged netlist. The result correctly remained `partial`.

Upstream LibrePCB independently opened the saved candidate and agreed:
one missing `V3V3` connection to `U1.21`, two silkscreen warnings, and no
other DRC errors. ERC reported an unused-net-class hint. A subsequent
V3V3-only pass on a 0.05 mm grid left the same one connection outstanding.
This is evidence for the applied-result checks and the explicit via fix;
the final placements differ from earlier experimental attempts, so these
counts are not an isolated router-performance comparison.

Two bounded manual probes on separate copies were reverted. A normal
escape from `U1.21` crossed GND copper and failed clearance. Bridging the
adjacent same-net `U1.20` did not reduce the air-wire count, since it was
already in the same connected segment. The remaining routing-access case
is preserved; a grid or endpoint-conversion root cause has not been proven.

## Applied-result contract

`autoroute` (both backends) and `specctra_import` return `result.validation`:

- `before` and `after`: actual unrouted count, unapproved DRC errors,
  copper errors, warnings, whether checks ran/completed, and check failures;
- `netlist_preserved`: component signal assignments and net identities
  stayed intact (physical shorts are checked by DRC);
- `drc_clean`: the final full DRC completed without unapproved errors;
- `acceptable`: both checks completed, the netlist stayed intact, and no
  new non-connectivity DRC errors appeared;
- new/resolved error counts, including a separate new-copper-error count,
  and at most 50 detailed new errors with kind, text and approval identity.

Both snapshots run full DRC on the project, rebuilding planes and air wires.
They do not add undo steps or discard the routing result. A result with
new errors, an altered netlist, or an incomplete check is `partial` and
stays available for inspection or one-step undo. Existing errors remain
visible and produce a warning; `complete` does not imply `drc_clean`.

Missing-connection errors are counted in the DRC totals and represented
by actual air wires, but excluded from the new-error identity comparison:
the air-wire tree can change as copper is added. Built-in `failed` still
counts unsuccessful router requests; `unrouted_after` counts the actual
board connections. For a filtered routing call, the operation's completion
still refers to the requested nets, while validation describes the whole board.

Error identity uses upstream approval expressions. Reimported objects with
new UUIDs can cause an existing physical violation to be newly reported;
the detailed identities and before/after counts make that conservative
comparison explicit. Do not infer a physical regression from counts alone.

## Reviewed trace replacement

`routing_edit_prepare` accepts `replacements` alongside via dimensions
and anchor moves. Each item names `traces` in chain order and `points`
in board millimeters. The new path uses the existing endpoints, net,
layer and width. With one trace the points start at its `from` end as
reported by `board_get`. An empty point list joins the endpoints directly.
Interior junctions must be unbranched. Vias, pads and branch junctions
can serve as endpoints.

Preview exposes the exact new trace and junction UUIDs and the elements
to remove. Apply requires completed DRC, no new errors and no increased
air-wire count on any net. Undo restores the original path in one step.

Add `"route":{"nets":["V3V3"]}` to try the builtin router after the
edit. The route object accepts the same layer, grid, width and via
overrides as `routing_prepare`. The trial uses the edited snapshot with
rebuilt planes. `route.routing` reports the attempted nets and failures.
The combined candidate's validation compares the original board with
the final copper. Prepare and preview leave the live board available for
another trial. Apply replays both patches as one undo step.

On the saved STM32 retry board, the builtin baseline still returned
`no_path` for `V3V3` at 0.1 mm grid pitch, 0.2 mm trace width and
0.3/0.6 mm via drill/diameter. Four reviewed trace replacement trials
on `GPIO_J4_1` and `GND` left one connection open. One of the four also
introduced a clearance violation. All four were discarded.

Moving `GPIO_J4_3` via `1b087d22-cc3e-4b15-a23a-e54fd3ca9c0d` from
(26.5, 19.4) to (27.0, 19.6) mm and routing only `V3V3` at the same
settings closed the connection in a 15-mutation candidate. The router
added 13 trace segments. The applied result preserved the netlist,
had zero new copper errors and reduced the board's missing connections
from one to zero. Stdio MCP save and reopen retained zero air wires.
Upstream LibrePCB 2.1.1 reported only the two existing silkscreen
clearance warnings.

## Current routing stage and next work

The [unified routing service design](routing-service-design.md) is the detailed
roadmap and states proposed versus implemented capabilities. R1 now provides
built-in candidates prepared and validated on isolated model state,
previewed without editing the live project, then applied against the original
project/revision in one undo operation. Re-running routing during apply is not
an exact-candidate implementation. Preparation must preserve redo, dirty state,
journal and revision as well as geometry.

The implemented R1 contract is `routing_prepare` (builtin only),
`routing_preview` (exact mutations paginated at up to 200 per page),
`routing_apply` and `routing_discard`. Each `OpenProject` owns the service;
candidates are bound to its service identity, project UUID and source revision.
At most eight candidates are retained, each at most 100,000 mutations; capacity
is refused, not silently evicted. Preparation runs native snapshot/search/checks
away from the live state and the search/check worker releases session locks.
Apply reuses the exact prepared mutations, requires `acceptable` validation,
checks source freshness, commits once and rechecks the applied model. Remaining
connections can stay partial; a candidate is consumed only after successful
mutation commit. See the design for post-commit evidence and lifecycle limits.
This is a data preview; GUI overlays, job IDs, cancellation and automatic ranking
are later work. Existing immediate `autoroute` retains its compatibility behavior.
Candidate lifecycle tests and a real stdio MCP prepare/preview/apply/undo/redo/
save/reopen test cover this stage. On a copy of the STM32 experiment with copper
removed through mutations, builtin at 0.1 mm prepared 559 mutations: 57 to four
remaining connections, zero new DRC/copper errors, unchanged netlist. Preparation
left the live board/revision unchanged; apply reproduced the candidate and native
save/reopen retained the four missing-connection findings. This is service
validation, not a claim that the remaining routing-access problem is solved.

The completed via, applied-result validation and `trace_update` work above is
the R0 foundation and must remain covered during the service refactor. For
existing immediate tools, a result with new violations remains `partial` and
available for undo; the new candidate workflow separately specifies what may
be accepted before commit.

After R1:

1. Extend grouped trace-width editing with via-size and connected geometry
   edits using the shared evidence, and add focused diagnostics/images.
2. Done. Resolve selected-device pins before PCB placement and report effective
   routing rules and their sources.
3. Improve LQFP pad access, local grid refinement, route ordering and regional
   rip-up/reroute against fixed examples without losing DRC correctness.
4. Evaluate a local `pnsrouter` adapter through the common candidate path, then
   add bounded agent-directed placement/strategy experiments. Full-board engine
   comparisons remain benchmarks until their benefit is demonstrated.

PCBCraft's [track/via editing contract](https://github.com/tangency-labs/pcbcraft/blob/9a74b2fbd69c667c4de5bd546cfb8a58edec846e/docs/mcp.md)
and [session-owned update plans](https://github.com/tangency-labs/pcbcraft/blob/9a74b2fbd69c667c4de5bd546cfb8a58edec846e/crates/engine/src/board/update/commands.rs)
are references for grouped edits and reviewed candidates. Retain LibrePCB's
shared model and existing intent-level tools rather than duplicating their
project or schematic-to-board synchronization model.

## Local access diagnostics

`routing_diagnose` takes a net name or UUID, optional board, builtin grid/width
and layer options, and a straight probe length (default 0.6 mm). It reports
grid access points at up to 16 remaining endpoints, effective width and
clearance, and the first blocker found by each straight probe. A blocker
with a source object includes `object.kind` and `object.uuid`, plus the
placed `device_uuid` for footprint geometry and the owning `pad_uuid` for
pad holes. The board-edge finding describes the combined outline. Terminal
blockers keep their board anchors and trace blockers retain `trace_uuid`.
Probe results describe local geometry and depend on which shape the check
encounters first.

To request one focused image, pass `"image":{"endpoint":0,"layer":"top"}`,
where `endpoint` is the zero-based index in the returned endpoints array.
Omit the layer to use the first probed layer, or the first allowed copper
layer if there are no probes. The 640 × 640 PNG is an MCP image block.
`result.image` gives the selected layer, crop bounds in board millimeters
and a color legend. The crop stays in top-view coordinates on every layer.
Amber marks the reported blocking shape, pink and green mark blocked and
clear probes, and cyan marks the endpoint and reported access nodes.

On the preserved STM32 retry board, `V3V3` at 0.1 mm grid and 0.2 mm width
had 17 access nodes at `U1.21` and four at the other endpoint. With 0.21 mm
builtin clearance, six of eight 0.6 mm straight probes from the pad were
blocked by `GND` and `GPIO_J4_3` traces or a `GPIO_J4_3` via. The result
establishes local obstruction and rules out an empty access set at either
endpoint for these settings. It does not determine why the full search fails.

The top-layer crop around `U1.21` covers 2 × 2 mm. A stdio MCP call
attached the PNG and returned five trace UUIDs and one via UUID for the six
blocked probes. All six resolved in `board_get` at the same project revision.

A follow-up `routing_edit_prepare` on a separate STM32 copy moved the
`GPIO_J4_3` via 0.05 mm in +X. It prepared and applied one reviewed
mutation with zero new copper errors. The attached traces followed the via.
The `V3V3` connection still remained open.

## Effective builtin rules

`routing_diagnose` returns `result.rules` in millimeters. For trace width,
via drill and via diameter it reports `effective_mm`, the original `base`
value with its source, the board DRC `minimum`, and whether the minimum
raised the value. `via_drill` and `via_size` accept the same overrides as
`routing_prepare`. The diameter floor is the effective drill plus twice
the DRC annular-ring minimum. `via_storage` records whether new vias
would store explicit dimensions and shows their inherited dimensions.

`copper_clearance` uses the largest board or project net-class minimum
plus the 0.01 mm builtin margin. A board tie uses the board setting.
The report includes the board and largest class inputs, the other
clearance types, and the effective trace layers. `grid_pitch` reports the
actual pitch when endpoint access was computed, otherwise
`effective_mm` is null. Probe blockers indicate which clearance applied,
including when the pad's own clearance exceeded the routing baseline.

Builtin uses the selected net class's default trace width when there is
no request override. Its width and drill floors come from the board DRC
settings. `net_class_drc` shows the selected class's width and drill
minima, which full DRC can enforce on a candidate. A net-class default
via drill affects the inherited via dimensions used when deciding
whether to store explicit dimensions.

On the saved STM32 retry board, stdio diagnostics for `V3V3` reported
0.2 mm from the net class, 0.21 mm copper clearance (0.2 mm board minimum
plus margin), and 0.3/0.7 mm via dimensions with automatic storage.
An explicit 0.3/0.6 mm request met the board minimum and switched the
reported storage to explicit. Both calls left the project revision intact.

## Pre-placement pin resolution

`component_add` and `component_get` expose the default assembly device
as `selected_device`. Physical pin names in `connect` and `disconnect`
use placed devices when present, then the default assembly device for
an unplaced component. A component with neither still supports the
previous lookup through its single device in the project library.
Conflicting placed-device mappings require an explicit signal name.

The resolver reads the device and package from the project library first
and then from the open workspace. If an unplaced device exists only in
the workspace, a session without that workspace can still use logical
signal references. No PCB placement is needed for schematic wiring.

A stdio regression places one MOSFET variant, saves and reopens a project
with a second variant still unplaced, and checks that connecting and
disconnecting its package pad 1 uses signal `G`. A pad name found only on
the cached alternative is rejected. Default placement then confirms the
connected physical pad. A separate stdio run with the experiment's
STM32G474CBTx library mapped `U1.21` to `U1.VDDA` and assigned `V3V3`
while the board still had no placed devices.

## Builtin search evidence

Set `search_evidence: true` in `routing_prepare` or an edit's nested
`route` to collect rejected grid steps for failed strict searches.
Each original failed request can include up to 32 samples in
`search_evidence`, selected near its target. A sample identifies the
layer, move coordinates in nanometers, and whether generated copper
blocked the grid step. The exact geometry check can report a board
object UUID, the board edge, or a generated route's request index and
net UUID. Regional routing can also identify the region boundary.
A null blocker means the sampled step had no exact hit.
The samples describe the attempted search and can guide a local trial.

`routing_prepare` and `routing_preview` expose `routing.failed_requests` and
`routing.remaining_air_wires`. Immediate builtin `autoroute` returns
`failed_requests` alongside the existing `remaining` list. Failed requests
keep their original net and anchors. Remaining air wires describe the rebuilt
connectivity tree and inherit a failure reason only when the net and both
anchors still match. Rebuilt pairs without a matching failure say `not_routed`.

`reason` reports the last strict failure or the outcome of final verification.
`soft_reason`, when present, reports the last failed attempt that allowed rip-up.
Attempts can see different states of the generated copper. In both modes
`no_access` means an empty anchor set, `no_path` a drained heuristic frontier,
`search_budget` the expansion cap reached without a path, and
`geometry_rejected` a failed exact check. Soft mode can also report
`rip_up_limit` when an intersecting generated route exhausted its allowance,
or `no_rip_up_conflict` when its proposed path hit no removable route.
Final verification can report `attachment_removed` for a route that depended
on rejected copper. `cancelled` and `not_routed` cover open connections without
a recorded failure. Original requests with unsupported anchors keep
`unsupported_anchor`.

Existing board copper remains fixed during builtin routing. A drained soft
search describes reachability in the attempted grid and its pruning rules
with generated routes traversable at a penalty. Use local access diagnostics
to inspect individual obstructions.

On the preserved STM32 retry board, a stdio `routing_prepare` for `V3V3`
at 0.1 mm grid pitch, 0.2 mm trace width and 0.3/0.6 mm via drill/diameter
reported `no_path` for both attempts. It retained the original junction and
`U1.21` anchors and proposed zero mutations. The one missing connection
remained open. This run exhausted the heuristic frontier on existing copper
with the specified rules and grid.

## Regional builtin routing

`routing_region_prepare` takes `region.min` and `region.max` points in board
millimeters and a `route` object with named `nets`. The same object accepts
the builtin layer, grid, width, via, and `search_evidence` options. An optional
`rip_up` list authorizes specific existing traces and vias. Each selected
object must belong to a target net and fit entirely within the rectangle.
Selecting a via requires listing every trace attached to it.

On the detached board, the service removes authorized copper, rebuilds
planes, and routes eligible air wires whose two anchors lie in the region.
Existing copper still affects clearance. New trace caps and via copper
must fit inside the rectangle. The service checks the final anchors,
positions, dimensions and UUIDs of protected traces and vias. Net segment
IDs can change when splitting and reconnecting copper. Apply uses the exact
reviewed patch and requires completed DRC and no increased air-wire count
on any net.

On the saved STM32 retry board, the failed strict search for `V3V3`
reported 16 rejected moves. Exact checks identified two bottom-layer
`GPIO_J4_1` traces, `ce92b034-7398-43b9-abd7-e01f1c57a228` and
`e54fbac9-c00e-40c2-b1a8-ab0eeb3e6423`, in 11 samples. A regional trial
authorized both traces in (24.55, 16.7)–(28.2, 20.2) mm and targeted
`GPIO_J4_1` and `V3V3`. It reconnected `GPIO_J4_1` with zero new DRC
errors, and `V3V3` stayed open. The candidate was discarded.

On a copy of the STM32 board completed by the earlier via-move trial,
a top-layer candidate replaced GND trace
`e70299f9-1f47-4590-896f-67a5755f171a` in (25.35, 17.28)–(27.15, 17.72)
mm. The stdio preview left the live revision unchanged. Apply and native
save/reopen preserved all other trace and via UUIDs and geometry, with
zero remaining air wires and zero new DRC errors. Upstream LibrePCB 2.1.1
reported only the two existing silkscreen warnings.

## Pad escape trials

Set `pad_escape: true` in `routing_prepare`, in a nested builtin `route`,
or in `routing_diagnose`. The router adds nearby entries outside the pad,
retains up to four per direction and layer, and checks the entire path
from the pad center. A path can include one off-grid 45° elbow. The grid
pitch and the per-search expansion limit stay the same. The diagnostic
reports the number of added entries as `escape_count`.

A fixed test uses a 0.65 mm gap, 0.2 mm trace width, 0.2 mm clearance
and a 0.25 mm grid. The baseline reports `no_path`. With pad escape
enabled the same grid routes the connection and passes the geometric checks.

On the saved STM32 board with one open connection, enabling pad escape
increased the grid entries for `U1.21` from 17 to 25 at 0.1 mm pitch.
The other endpoint retained four entries. Both attempts still reported
`no_path` with one open air wire and no new DRC errors. This explains
why more pad entries alone do not replace the targeted local edit above.

### Routing order and critical nets

With pad escape enabled the builtin router scores the exact-checked
exterior entries on the initial board. For each air wire it considers the
more constrained pad, comparing exclusive entries first and total entries
second. A layer/node pair reachable by different nets counts as shared.
Wire length breaks ties.

Pass `critical_nets: ["V3V3"]` to `routing_prepare` or in a nested builtin
`route` to give those nets the first routing tier. A lower priority net
can still take a clear path, but its soft attempt reports `protected_route`
if that path would rip up a critical route generated in the same trial.
This option does not guarantee the critical net is connected: check the
candidate's remaining air wires and DRC before applying. With both options
omitted, the order stays shortest first.

Single-layer regression cases verify that a constrained pad can take
precedence over a shorter crossing, and that a declared critical net keeps
its route when an ordinary net tries to cross it.

The full STM32 replay starts from the same placements with existing routing
removed, 57 air wires, and the same grid (0.1 mm), trace width (0.2 mm)
and via dimensions (0.3/0.6 mm) for all three candidates:

| Builtin options | Remaining air wires | New DRC errors | Traces | Vias |
| --- | --- | ---: | ---: | ---: |
| Default order | 1 (`V3V3`) | 0 | 470 | 42 |
| `pad_escape` | 0 | 0 | 466 | 47 |
| `pad_escape` + `critical_nets: ["V3V3"]` | 2 (`NRST`, `GPIO_J4_3`) | 0 | 404 | 32 |

The complete candidate went through MCP preview, apply, save and reopen
with zero remaining air wires. Upstream LibrePCB reported only the two
existing silkscreen-to-mask warnings, no missing connections or copper
violations.

The complete candidate combines the added pad entries with constrained
ordering; this comparison does not isolate their individual effects.
Giving a net explicit priority can trade overall completion for that
net's connectivity, so compare a trial with and without priority before
applying it.
