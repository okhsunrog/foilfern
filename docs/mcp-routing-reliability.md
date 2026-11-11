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
2. Fix selected-device pin resolution before PCB placement and expose effective
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

## Builtin search evidence

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
