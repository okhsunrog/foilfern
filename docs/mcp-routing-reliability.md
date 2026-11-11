# Routing reliability for MCP design workflows

Chronological implementation and replay evidence. The STM32G474 experiment on
`c1101ac` established the initial workflow and exposed routing-integration
failures; later sections record subsequent fixes and their limits. Counts,
timings and statements about the next experiment belong to the cited revision.
Use [status](status.md) for the current summary, [MCP design](mcp-design.md) for
available APIs and [roadmap](roadmap.md) for the next delivery gates. In particular,
persistent requirements are now planned in P2, after the ordinary editing and
candidate-validation foundations recorded here.

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

### STM32G474 with buck converter and bottom GND pour

On a copy of the routed board, removing the existing stitch at C2.2 left
one connected fragment and nine discarded islands. Both versions of the
board had zero air wires. Probing (34.2, 25.0) mm identified the largest
discarded island, 70.30 mm², with bounds from (28.44, 21.35) to
(47.44, 27.95) mm.

`plane_stitch_prepare` checked a 0.30 mm drill, 0.60 mm via and 0.35 mm
top lead from C2.2. Preview showed the same 70.30 mm² fragment as
connected at the proposed via. After apply, the connected fragment count
rose from one to two, connected area from 1805.27 to 1875.57 mm², and
discarded islands fell from nine to eight. The MCP replay checked the
live board before apply, undo/redo, and save/reopen. Upstream LibrePCB
opened the saved copy with zero ERC or DRC errors and the same 17
silkscreen warnings.

### Reference coverage on the STM32G474 buck board

The bottom `GND` pour was measured under the top segments of `SWCLK` and
`SWDIO`. Removing and restoring the via at `C2.2` changed the connected
pour by 70.30 mm². Uncovered centerline length stayed at 1.244 mm for
`SWCLK` and 3.544 mm for `SWDIO`.

A bottom-layer plane keepout from (35.2, 29.7) to (35.6, 30.1) mm
introduced a 0.543 mm gap under `SWDIO` with zero air wires. A reviewed
replacement detoured the top trace around the gap. The preview reported
0.543 mm resolved and zero new gaps on the two selected nets. Apply,
undo/redo and save/reopen reproduced the measured states. DRC found no
new errors. This replay measures geometric centerline coverage.

## ESP32-C3 USB-PD sink benchmark

Use the KiCad source at commit `5b96caf` on `epr-48v` for the next real
board replay. The four-layer board has 86 footprints, 1,442 trace
segments and 285 vias. The original routing is a comparison baseline.
KiCad 10.0.7 DRC on the original reports four copper-to-hole errors at
J2, three open connections between J1 shield pads, and 211 warnings.

The flat schematic import produced 148 placed symbols including power
symbols, and matched all 60 groups of connected physical pins in the
KiCad CLI netlist when compared by reference and pin number. LibrePCB-RS
ERC reported zero errors and warnings. KiCad's generated names for
unnamed nets can differ in the schematic-only import.

Importing the PCB setup restored all 86 placements (75 top, 11 bottom),
339 numbered pad centers, eight plane outlines, the antenna keepout, and
the four-layer board outline. The 12 unnumbered KiCad entries were
paste and mask apertures and non-plated holes. The initial LibrePCB-RS board had
169 open connections after fill. Its remaining DRC errors included two
copper-to-hole findings and a keepout finding on the antenna pad. The
KiCad keepout allows pads, while the native copper keepout covers pads
as well as tracks and vias. THT-only zone thermal relief maps to solid
connections. The routing setup retains the layer count and total
thickness. Dielectric stack details and 3D assets remain in the source.

The J1 and J2 THT pads use paste on one side. Import represents those
apertures as explicit polygons on the correct paste layer.

Compare new routing from the imported placement and rules using native
DRC and connectivity, power-path widths, ground coverage, total length,
via count and manual interventions. Report the original layout metrics
alongside the new ones.

The first two builtin trials reduced open connections from 169 to 162
and then to 40, with zero new non-connectivity DRC errors. The power
trial regressed `VMOS` and `GND`. The signal trial regressed `VBUS` and
`GND`. In both cases the previous candidate gate allowed the change
because it relied on the non-connectivity DRC comparison. The checked
builtin path now rejects increased air-wire counts on any net. A replay
of the power trial showed the same regressions and refused apply.

Immediately after opening the imported board, `unrouted` previously
reported 266 open connections until a separate plane rebuild brought it
to 169. `unrouted` and `board_get` now calculate a detached fill when
needed. Both report 169 on the reopened project without requiring
`planes_rebuild` or changing the project revision.

The power-first Foilfern replay closed the power rails, GND, USB data,
the crystal and antenna paths before the broader signal pass. The
reopened 20-gap checkpoint has 763 trace segments, 1,033.762 mm of trace
centerlines and 101 vias. Its ERC is empty. Both Foilfern and upstream
LibrePCB report 20 missing connections, the same three non-connectivity
errors as the import, and nine warnings. The OLED pin-30 legend was moved
away from mask openings without moving its footprint.

The broad trial exposed specific plane contacts near the ADC divider,
the OLED and the power connector. Reporting air-wire endpoint positions
made it possible to seed local plane contacts and to preserve a useful
candidate while repairing the copper that divided GND. The two zero-length
gaps at the data USB connector came from coincident pad identities on
the same net; small solid fills limited to the existing pad footprints
connected the native model for this replay. A durable import solution
should preserve the pin mapping and represent shared physical copper
without adding such fills by hand.

The continuation started from the 19-gap checkpoint. Part of the top-layer
VCC distribution enclosed the ESP32-C3 pads on two sides. Moving the
distribution off the top opened up sites for staggered through vias.
A fixed-placement candidate reached six gaps. Moving C11 by 1.4 mm
away from the south row opened an escape for the remaining pins. The
completed routing also moved the local VCC branch by U2 to escape INT
and added ground connections after checking the rebuilt plane fragments.
The circuit and schematic files were byte-identical to the 19-gap checkpoint.

The reopened four-layer result reports zero missing connections in
Foilfern and upstream LibrePCB, zero ERC findings, the three inherited
non-connectivity errors and nine inherited warnings. It has 1,009 trace
segments, 1,529.788 mm of trace centerlines and 145 vias. No trace runs on
`In1`, which serves as the ground reference.

The USB pair kept its path lengths and its projected ground coverage on
`In1` (91.1% for D+ and 91.6% for D−). On the bottom layer the 19-gap
checkpoint measured 86.2% and 86.9%. A parallel SCL run reduced those
figures to 61.1% and 67.3%. A local SCL reroute recovered coverage to
72.9% and 73.9% with the same via count. These figures measure projected
overlap with ground copper. Differential impedance requires the
dielectric stack and a separate check. The inherited hole-clearance and
antenna keepout findings still require a fabrication review.

## Two-layer ESP32-C3 dev board completion

The comparison against tscircuit used a 45 × 60 mm board with 36 positions
and 29 nets. The saved Foilfern board opened with six gaps on `GPIO1`,
`GPIO3`, `GPIO4`, `GPIO6`, `GPIO7` and `V3V3`. Routing `GPIO3` alone with
pad escape on 0.10 and 0.075 mm grids produced empty candidates. A larger
regional trial left eight gaps and was discarded.

Five checked MCP patches closed the six gaps on the original two layers:

1. Replaced local EN and ground paths and resized three existing vias
   from 0.70 to 0.60 mm, keeping their 0.30 mm drills. The new `GPIO3` route
   stayed on the top layer.
2. Reworked the `GPIO0` and `GPIO2` launches to open the `GPIO4` path.
3. Moved C5 by (−0.30, −0.80) mm and restored its supply and ground leads
   in the same candidate as `GPIO6` and `V3V3`.
4. Removed a GND via that had become redundant after the first patch.
5. Closed `GPIO1` and `GPIO7`, replaced the conflicting `V3V3` branch,
   and added a top-layer ground link to reconnect the two ground areas
   that the `GPIO7` trial separated. The supply branch crosses the west
   side of the board, away from the oscillator and RF feed.

All geometry was applied through MCP. An external scratch search helped
propose exact coordinates for the local paths. The checked candidates used
the native model, full DRC and the rebuilt copper planes. `board_get` now
reports effective dimensions for automatic vias, `routing_copper_prepare`
can replace selected vias and move unlocked devices with their copper, and
validation reports per-net connectivity gains separately from acceptance.

The final board has 691 trace segments, 1,096.493 mm of centerlines and 79
vias. The baseline had 608 segments, 852.307 mm and 71 vias. Foilfern and
LibrePCB 2.1.1 agree on zero ERC errors, zero missing connections, zero DRC
errors and 13 inherited silkscreen-to-stop-mask warnings. The circuit and
schematic directories are byte-identical to the baseline. The 47 traces of
`RF_CHIP`, `RF_ANT`, `XTAL_P` and `XTAL_N` retain their UUIDs and geometry,
with zero vias on those nets. Their bottom-layer ground coverage is
unchanged, including 90.7% for `RF_CHIP` and 100% for each crystal net.
Impedance, antenna tuning and hardware operation are unverified.

The five accepted previews contain 105 forward mutations. Replaying them
through MCP against a fresh baseline copy reproduced the same placement,
traces, vias, plane fill, outline and connectivity. An independent
LibrePCB export matched all 11 Foilfern Gerber and Excellon files after
removing generator version, timestamp and Gerber checksum metadata.

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

### STM32 comparison of initial orders

One stdio `routing_compare` call started from the same unrouted copy (57
air wires). All three trials used the same settings, including pad escape:

```json
{
  "options": {
    "grid_pitch": 0.1,
    "trace_width": 0.2,
    "via_drill": 0.3,
    "via_size": 0.6,
    "pad_escape": true
  }
}
```

| Rank | Order | New DRC errors | Open air wires | Vias | Trace length, mm |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | `constrained_first` | 0 | 0 | 47 | 765.493 |
| 2 | `longest_first` | 0 | 0 | 56 | 741.532 |
| 3 | `shortest_first` | 0 | 1 (`V3V3`) | 40 | 719.540 |

All previews reported the same baseline, and the live board and revision
stayed unchanged until apply. The recommendation favors nine fewer vias
over about 24 mm less trace length among the two complete candidates.
Applying it reproduced the preview validation, board metrics and netlist.
The other IDs became stale. Saving and reopening preserved the geometry,
and the upstream CLI reported only the two pre-existing silkscreen-to-mask
warnings. This run took about 63 s in the local debug build for the full
three-order comparison.

### Congestion history and STM32 with a buck converter

Set `congestion_history: true` in `routing_prepare`, in the common options
of `routing_compare`, or in a nested builtin `route`. Once a soft search
passes the exact checks and commits a path after displacing other generated
copper, the router records contested locations along the path. A tile is
four by four grid cells, with separate maps per trace layer and for via
sites. Each accepted rip-up contributes at most one hit per tile, capped
at four hits. A hit adds two straight steps of cost when a subsequent soft
search enters the tile. Strict search, physical permissions, DRC and
search limits stay the same. History is reset for each candidate.
Candidate previews expose `routing.effort` with `searches`, `rip_ups` and
`congestion_tiles`.

A small regression has two contested crossings. Repeatedly using one
crossing raises its cost enough for the next soft search to choose the
other. Strict search still rejects both while they contain other nets'
copper. A board-level test verifies that an actual rip-up records history
and remains geometrically valid.

On the STM32G474 copy with an AP63203 5 V to 3.3 V buck, the placed and
routed power stage (including a local bottom GND polygon) was protected
while 57 open connections went to the builtin router. Rules: 0.1 mm grid,
0.2 mm trace width, 0.3/0.6 mm vias and pad escape enabled:

| Order | History | Open air wires | New DRC errors | Vias | Trace length, mm | Rip-ups | History tiles |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `shortest_first` | off | 1 | 0 | 42 | 748.592 | 11 | 0 |
| `shortest_first` | on | 1 | 0 | 42 | 748.592 | 11 | 14 |
| `constrained_first` | off | 1 | 0 | 48 | 751.974 | 12 | 0 |
| `constrained_first` | on | 1 | 0 | 48 | 751.974 | 12 | 16 |

History did not change the outcome on this board. The last air wire
was `V3V3` at `U1.21`, which remained open even in regional trials.
A via explicitly placed at (26.0, 18.95) mm and connected on both
layers closed it without changing the power stage. This is a useful
case for exact local via-site exploration: learning where previous
routes conflicted did not recover a missed legal transition.

The portable `routing-baseline.zip` in the experiment deliverables has
SHA-256 `47876d3e09a84f58d39eea5ff67aa29693467a00c52034a7e5d93e16dcc9619d`.
It retains the benchmark placement. The separate finished project also
shifts C10 and its local ground via for assembly clearance.

### Short pad escapes on a wide net

Pass `neck_down: {"width":0.2,"max_length":1.0}` to a reviewed builtin
trial to allow a short reduced-width escape from a pad to a grid point
that accepts the full trace width. Both values are in mm. An escape is
straight or has one 45° bend. Its length is also bounded by 24 grid pitches.
The exact checks apply the width of each segment. A cost penalty favors
ordinary escapes, and the main A* search uses the full width.

The STM32 buck benchmark has 15 open `V3V3` connections before routing.
With the other nets still open, the same placement and 0.1 mm grid gave:

| Trace widths | Remaining V3V3 | New DRC errors | New trace length |
| --- | ---: | ---: | --- |
| 0.20 mm throughout | 0 | 0 | 81.826 mm at 0.20 mm |
| 0.35 mm throughout | 10 | 0 | 44.135 mm at 0.35 mm |
| 0.35 mm with short pad escapes | 0 | 0 | 76.956 mm at 0.35 mm, 5.105 mm at 0.20 mm |

In a second trial with existing signal copper fixed, rerouting the 15
V3V3 connections left two open at 0.20 mm or with the mixed-width option,
and ten at 0.35 mm. The mixed-width candidate added 93.804 mm at
0.35 mm and 3.264 mm at 0.20 mm. In both trials, applying the mixed-width
candidate reproduced the preview metrics and the widths survived native
save and reopen.

After routing the remaining nets in the power-first trial, upstream
LibrePCB found one open GND connection at U1.23. The fixed-copper trial
still had two open V3V3 connections, including U1.21. Both upstream checks
reported zero ERC findings and no DRC errors other than the open connections.

### Local via search

On a copy of the completed STM32 buck board, removing the hand-placed
via and its four lead segments restores the single V3V3 break at U1.21.
Ordinary builtin routing at 0.1 mm with pad escape leaves it open.
`routing_via_prepare` connects U1.21 on top to the existing V3V3
junction at (27.8, 18.9) mm on bottom. The request uses 0.20 mm
traces and a 0.30/0.60 mm through via.

The exact-position request at (26.0, 18.95) mm and the automatic
1 mm radius search both produce a checked candidate at that position.
An exact request on the neighboring foreign-net pad is refused.
Automatic search samples every 0.05 mm, up to 1.5 mm from the first
anchor. Each lead is at most 4 mm and uses at most one bend. Exact
checks keep the drill outside pad copper and allow the via annulus
to touch copper of the same net. The full board DRC runs on the
resulting patch, and apply requires improved connectivity on the
target net and preservation of the existing traces and vias.

Applying the automatic candidate closes the one open connection with
zero new DRC errors. All existing trace and via UUIDs and geometry
match the baseline. Undo and redo restore the two board states, and
save/reopen with rebuilt planes preserves the repair. Upstream LibrePCB
reports zero ERC findings and only the 17 existing silkscreen warnings.
