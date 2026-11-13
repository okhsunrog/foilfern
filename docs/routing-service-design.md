# Unified routing service

Reviewed 2026-11-11 against `f3c3fda`. This document owns the routing service
contract. R0–R6 are historical design areas, not the delivery order: implemented
slices and remaining proposals coexist within them. The [roadmap](roadmap.md)
sets current priorities and acceptance gates; [reliability reports](mcp-routing-reliability.md)
retain the evidence for individual changes.

## Current capability map

| Area | Available | Remaining work |
| --- | --- | --- |
| Candidates | Detached prepare/preview/apply/discard, exact patches, stale guards, one undo | Desktop candidate overlay (P5) |
| Evaluation | Full DRC and rebuilt connectivity, per-net deltas, connectivity improvement status, 2–3 builtin order comparisons | Requirement-aware objectives and disturbance costs (P2/P4) |
| Local repair | Trace replacement, junction/via moves, regional rip-up, local via search, explicit unlocked-device moves/rotations with copper | Diagnosed bounded placement search; connected side changes are unsupported |
| Router | Pad escape, critical-net ordering, neckdown, congestion history | Joint fanout planning and benchmark-driven refinements (P4) |
| Planes and measurements | Split diagnostics, stitching, geometric reference coverage, trace-only path length | Electrical return-path analysis and broader measurement semantics |
| Runtime | Detached workers, guarded baseline reuse, bounded retained candidate store | Worker admission, job progress and cancellation (P3) |

## Purpose and scope

Build one routing workflow for the editor and MCP. The user describes the
connection or region to improve, permitted changes and objective. The service
prepares a candidate, checks the result in the LibrePCB model, exposes a preview
and applies the reviewed candidate as one undoable operation. Specialized
algorithms are internal implementation choices, not competing project models or
an expanding set of unrelated user workflows.

The first stage uses the existing built-in router. It is not a router rewrite,
a new board format, an integration of every available engine or a claim that
all boards can be routed. Existing immediate `autoroute` and external
Freerouting/SES workflows retain their documented behavior during migration.

## Existing foundation (R0, implemented)

- `librepcb-autoroute`: independent geometry problem and routing result,
  deterministic grid A*, pad access, bounded rip-up/reroute, simplification and
  geometric verification, configurable ordering, pad escape and congestion history.
- `librepcb-editor`: extraction from real board/DRC data and application through
  ordinary commands, preserving the shared model and undo history.
- Full applied-model before/after DRC and connectivity validation for built-in,
  Freerouting and SES import; explicit/automatic via dimensions are consistent.
- `trace_update`: grouped width changes preserve trace identities and anchors,
  with the same validation evidence.
- Frozen STM32G474 reproduction and small local-fixture regressions. Ordinary
  builtin routing left `V3V3` access to `U1.21` open; the subsequent
  [local-via search](mcp-routing-reliability.md#local-via-search) closes that
  case. This does not establish general autorouter completion.

## Ownership and architecture

```text
GUI / MCP
    |
    v
routing service in librepcb-editor
    |-- snapshot / effective constraints / request
    |-- planner and algorithm adapters
    |-- candidate geometry and topology changes
    |-- apply to isolated model / validate / evaluate
    `-- revision-bound apply through the command bus
                 |
                 v
      authoritative LibrePCB project + shared undo
```

`librepcb-core::Project` remains the only authoritative board and circuit model.
A routing snapshot is derived from it and never becomes a second independently
editable project. Integer nanometres and explicit board, net and object IDs are
used at boundaries; external coordinate ranges require checked conversions.
Native upstream file compatibility remains a hard requirement. Runtime
candidates, previews and worker state do not change native project files.

The editor owns domain behavior. MCP owns transport, input/output DTOs and
runtime call orchestration. The GUI will consume the same service; it does not
need a second implementation. Keep core synchronous. Long work belongs on
caller-owned blocking workers, without holding project locks across awaits.

### Request

The target contract describes the board and base revision, scope (whole board,
selected nets, connection or region), permitted changes, objective and budget.
Current entry points support builtin options, selected regions and explicit
rip-up permissions, local repairs and selected device moves/rotations combined
with copper changes. They do not automatically search allowed placement regions
or enforce a persisted engineering-intent document. Unsupported capabilities
must be rejected explicitly; see the [current MCP contracts](mcp-design.md).

Hard constraints include connectivity, clearance/width/drill limits, forbidden
areas/layers and protected geometry. Preferences include route length, via cost,
preferred corridors and preserving already good work. A preference cannot pay
for violating a hard constraint. Changes to hard constraints are separate,
explicit project edits with their own provenance, not an internal search trick.

### Candidate and preview

A candidate is a frozen proposal tied to the source project state and board. It
contains enough information to apply the reviewed result without rerunning the
search, plus requested options, geometry/change preview, remaining connections,
metrics and validation evidence. An opaque ID addresses runtime-owned candidate
state; callers cannot submit a modified patch as that reviewed candidate.

Preview must leave the live project's geometry, revision, journal, dirty state,
undo/redo and files unchanged. It is not "apply and immediately undo" on the live
editor: that changes revision/history, may publish changes and can destroy redo.
Prepare and validation run on an isolated model. Preparation failures leave the
live state unchanged as well.

Native runtime object IDs generated while preparing a candidate must either be
preserved when applying it, or mapped explicitly so the preview and committed
geometry/anchors can be compared. The exact candidate is applied; a second
routing run is not an implementation of apply.

### Validation and apply

The geometric algorithm is a fast search aid. Acceptance evidence comes from
the candidate applied to the real LibrePCB model and rebuilt derived data:

- full checks must complete; a failed or skipped check is not a pass;
- circuit net identities and component signal assignments are preserved;
- connectivity is rebuilt from applied copper;
- new non-connectivity DRC errors are identified against the baseline;
- pre-existing errors and warnings remain visible;
- remaining mandatory connections remain explicit.

Preserve the existing distinction between `acceptable`, `drc_clean` and routing
completion. A valid partial candidate can be useful and must remain inspectable.
A candidate which introduces new mandatory-rule violations must not silently
become an accepted reviewed result. The R1 implementation contract below states
the actual rejection behavior; future explicit overrides must be separately
designed, rather than inferred from a numeric score.

Apply checks source project identity and revision before changing anything.
Another edit, including an edit followed by undo, can make a candidate stale;
do not infer freshness from visually identical geometry. Unknown, stale,
discarded, consumed or wrong-project IDs reject without changing geometry or
history. Candidate consumption happens only after a successful atomic commit.
A successful nonempty apply is one shared undo group and produces committed
read-back; redo restores the same accepted result. A no-op must not add history.

### Topology construction

Algorithms may output segments and vias, but their adapters must connect those
to the existing typed pad/via/junction identities. Use one shared topology
construction path rather than a separate importer for each algorithm. Coordinate
matching is checked supporting logic, not a replacement for known identities.
Delete/reshape patches and any future shove operation must state what items change
and preserve unrelated topology, copper and locks.

## Design areas and implementation evidence

The original R labels are retained for links and implementation history. Use
roadmap P0–P6 for new delivery decisions. Do not start a new engine integration
simply because it has an attractive feature list.

### R1 — built-in candidates, preview and apply (implemented first slice)

The first slice implements this path through the editor service and MCP:

1. Capture an isolated state for the selected board at a checked base revision.
2. Run the existing built-in router and apply its exact result to that state.
3. Run the applied-model checks and retain the candidate with bounded runtime
   storage and an opaque identity.
4. Inspect the geometry, remaining connections and validation without editing
   the live project.
5. Apply that reviewed candidate with identity/revision checks in one undo
   operation, or discard it.

Acceptance cases:

- preparation/inspection leave live geometry, revision, files, dirty flag,
  journal and undo/redo unchanged, including when redo is already available;
- applied geometry matches the inspected candidate; routing is not rerun;
- successful apply has one undo step; undo/redo restore exact geometry and
  anchors; no-op preparation/application does not add a history entry;
- unknown, stale, cross-project, discarded and already applied candidates
  fail without changing live state;
- invalid/unacceptable candidate checks are surfaced and cannot be mistaken
  for successful validation; baseline violations and incomplete routing stay
  explicit;
- existing command groups and failure rollback preserve their boundaries;
- candidate lifetime/storage is bounded, with explicit capacity refusal and
  discard/invalidation on project replacement;
- editor integration tests and actual MCP dispatch/schema tests cover the
  lifecycle; save/reopen retains native compatibility;
- large-board CPU/memory and GUI preview remain explicitly unproven until
  measured; a JSON geometry preview alone does not claim a new GUI overlay.

#### Concrete R1 contract

The domain implementation is `librepcb_editor::routing_service`, with one
`RoutingService` owned by each `OpenProject`. GUI and MCP can share this service
through their existing shared open-project handle. Candidate storage is not
owned by individual MCP transports and never enters native serialization.

`Project::snapshot` serializes/reloads native data into a detached read-only
transactional overlay, including the current project assets. Snapshot and
candidate work do not write to disk or modify the live transactional overlay.
`RoutingJob` carries this isolated state; built-in search and full before/after
DRC run in a blocking worker after the live session locks are released. The
exact `Mutation` patch is recovered from the sandbox command history, preserving
UUIDs allocated for the preview. Apply replays that patch, not the router.

The MCP interface for this stage is:

| Tool | Contract |
|---|---|
| `routing_prepare` | Prepare one candidate using built-in parameters only; return the candidate identity and checked result metadata. |
| `routing_compare` | Prepare two or three builtin order variants from one snapshot; return ranked metrics and previewable candidate IDs. |
| `routing_staged_prepare` | Route explicit nets in checked batches on one detached snapshot, bisect rejected or unproductive groups and freeze the retained work as one candidate. |
| `routing_preview` | Inspect `candidate`, with `offset`/`limit` pagination (at most 200 mutations per page); return exact mutations and validation/request/result metadata. |
| `routing_apply` | Apply `candidate` with optional caller `expected_revision`; source binding is always checked even if the caller omits that field. |
| `routing_discard` | Release a retained candidate without changing the project or undo history. |

Other prepare tools (regional, copper, local-via and plane-stitch operations)
use the same lifecycle. Their accepted inputs are documented in
[MCP design](mcp-design.md).

Storage retains at most eight candidates, each containing at most 100,000
mutations. A full store refuses preparation/retention instead of silently
evicting a reviewed candidate. Explicit discard releases capacity. Identity
binds a candidate to its service instance, project UUID, board and base revision;
the source path and save time also have to match, because text placeholders
can affect copper without a model mutation. The detached project keeps those
attribute values without sharing the source filesystem. Closing/dropping the
open project drops its candidate store. A reopened project
with the same UUID/revision cannot reuse the previous service's candidate.

Apply requires `validation.acceptable`: full checks completed, netlist preserved,
no new non-connectivity DRC errors and no increase in air wires on any net.
Evaluate air wires after rebuilding planes, including nets outside the routing
selection. A candidate which still has missing
connections can be acceptable and remains explicitly partial. Existing DRC
errors remain visible; `acceptable` does not mean `drc_clean`. Failed gate or
stale/foreign identity checks leave the live model and history unchanged.
The candidate is consumed only after its mutation commit succeeds. After apply,
checks run again against the actual model and the response includes their
result. An empty patch consumes the candidate without an undo entry or live
revision change; its already-completed detached checks are reused. A post-commit check failure must be reported honestly; it is not a reason
to claim the commit never occurred or that the candidate remains reusable.

Preview in R1 is structured mutation/geometry data, not a PNG or GUI overlay.
Preparation is an asynchronous MCP call backed by `spawn_blocking`, not a new
background-job protocol. There are no job IDs, progress polling/cancellation
tools or a new GUI review panel. `routing_compare` does rank and recommend
retained candidates from two or three builtin order trials. Long-worker
cancellation and visual review remain planned. The eight-candidate cap bounds
retained patches only; it does not currently limit concurrent preparation workers
or total snapshot memory. Admission control and cancellation are operational
follow-up work.

Staged preparation uses the same source binding and final apply gate.
Each accepted batch supplies fixed copper for the next search. Rejected
trials are undone in the sandbox and planes and air wires are rebuilt
before further routing. The frozen patch contains only the retained batches
in their execution order.

The legacy immediate `autoroute` call remains a compatibility path with its
existing behavior; R1 does not claim it already runs through the candidate
service. Converging those paths belongs to a later bounded refactor after the
reviewed lifecycle is validated.

#### Verification evidence

The counts below describe the original R1 verification, not today's entire suite.
See [status](status.md) for the latest scoped checks.

- Core `project::project_test`: six tests, including detached unsaved snapshot
  and embedded-asset isolation; native project roundtrip corpus passes.
- MCP `routing_candidates::`: nine lifecycle tests covering live state/redo
  isolation, exact UUID-preserving patch, one undo, stale edits including undo,
  discard/capacity, changed-project worker completion, save-time invalidation
  without a model revision change, and no-op apply.
- Actual stdio MCP dispatch: tool discovery, prepare/preview/apply, consumed ID
  rejection, undo/redo, save/reopen and full DRC on a two-net board.
- Editor service test: unsafe/incomplete validation gates refuse without edits.
- Manual copied STM32 case: remove existing copper, builtin 0.1 mm candidate
  contains 559 mutations and reduces 57 connections to four, without new DRC
  or copper errors; live board/revision unchanged during prepare/preview;
  apply and native save/reopen confirm the candidate's four missing connections.

These checks establish the first service path, not a finished autorouter or GUI
review experience. The STM32 run used the earlier experiment copy, not the newer
57-to-one case; its remaining four connections are not a regression measurement.

### R2 — diagnostics and ordinary connected edits

Implemented `routing_diagnose` for one net's remaining endpoints. It reports
builtin grid access, effective width and clearance, and the first object that
blocks each local straight probe. Blockers carry their source board or
footprint object identities. An optional focused PNG shows probe results,
access points and blocking shapes on a selected copper layer.
See the [preserved STM32 observation](mcp-routing-reliability.md#local-access-diagnostics).

The same diagnostic reports effective builtin rules and their inputs for
the selected net, including DRC clamps, the conservative copper clearance,
and via dimensions. See the [rule report](mcp-routing-reliability.md#effective-builtin-rules).

Schematic `connect` and `disconnect` can resolve a package pad before
placement through the component's selected device. The workspace can
provide the device and package before either is in the project library.
See [pre-placement pin resolution](mcp-routing-reliability.md#pre-placement-pin-resolution).

Implemented `routing_edit_prepare` for explicit via dimensions and moves of
existing vias and junctions, including both in one candidate. Connected traces
follow the anchor with unchanged IDs and connectivity. Prepare runs full model
checks on the detached edit. Preview/apply use the same exact patch, revision
checks and undo history as builtin routing.

`routing_edit_prepare` can also replace an ordered chain of traces on
one layer and width with new waypoints. Both endpoints stay attached,
including any branches at those endpoints. Only the selected traces
and their unbranched internal junctions are replaced.

An optional builtin trial routes named nets after the edit on the
same snapshot. It rebuilds planes before routing and captures the two
patches in order. Preview reports the trial result and apply commits the
combined, checked geometry in one undo step.
The [STM32 replay](mcp-routing-reliability.md#reviewed-trace-replacement)
closed the last missing connection using this workflow.

Builtin now preserves failed original requests alongside rebuilt air wires.
The last strict and rip-up attempts are reported separately. They distinguish
missing access, a drained search frontier, a search budget cutoff, exact
geometry rejection, and rip-up limits. Unattempted work can report cancellation.
See [builtin search evidence](mcp-routing-reliability.md#builtin-search-evidence)
for the field contract.

`routing_prepare` can collect bounded samples of rejected moves in the
last strict search for a failed request. Samples identify whether the
grid rejected the move against existing geometry or a generated route.
Exact clearance checks can identify the board object or the generated
route request. Some conservative grid rejections have no exact hit.
Suggested remedies still need a trial on the board.

Acceptance: an agent can inspect a remaining connection, identify a concrete
blocker, make a bounded edit and verify progress without hand-editing files or
recreating unrelated copper.

#### Ground reference coverage and targeted stitching

`plane_diagnose` and `plane_stitch_prepare` are implemented. The diagnostic
locates discarded fragments and can probe a point. A stitch candidate checks
that a proposed through via and a lead to an existing same-net anchor bring
the fragment at the via center into the fill. The
[STM32G474 buck replay](mcp-routing-reliability.md#stm32g474-with-buck-converter-and-bottom-gnd-pour)
demonstrated recovered copper and unchanged DRC and air-wire counts.
`reference_coverage` now measures individual signal traces, and
`routing_preview` accepts the same reference selection for a fresh candidate.

**Find the gap.** The caller selects signal nets and the reference net and
layer. Start with straight trace segments on a signal layer next to the
selected reference layer. Project each trace centerline onto the freshly
rebuilt, connected pour on that reference layer. Report uncovered intervals,
including holes in the fill. For each interval give the signal net, trace
UUID, layer, start and end in board coordinates, and length. Include checked
and uncovered lengths per net and identify selected geometry outside the
supported layer pair. A report with no checked traces gives no coverage
result. This is a geometric check. The result describes copper under the
centerline and leaves return impedance, loop inductance and signal quality
unmeasured.

**Choose the repair.** When a trace crosses a slot in its reference copper,
first try moving the signal over continuous ground or rerouting the trace
that cut the pour. Try a stitch when a discarded fragment fills a specific
gap and a short connection to ground on another layer can restore it. A
nearby same-net ground via at a signal layer transition can also provide a
local path between two reference layers. That transition check is a later
extension of the centerline check. A via connects the copper it touches.
Rebuild and measure again to find out whether the gap under the signal
actually changed.

**Compare candidates.** Evaluate the baseline and each candidate on
snapshots of the same source revision, using the same nets and reference
layer. Rebuild the fill on each snapshot and include the before and after
coverage in `routing_preview` alongside full DRC and connectivity. Show
which gaps disappeared, shrank or appeared, with per-net uncovered lengths
so gains on one signal cannot hide a regression on another. Apply through
the existing revision-bound candidate workflow. A candidate resolves the
targeted gap when that gap disappears and no new gaps appear on the selected
nets. A candidate that shortens the gap can be reported as a partial
improvement. The candidate must still meet the existing DRC and connectivity
checks. Recovered area and via count are supporting evidence.

**Leave unused islands discarded.** Report their geometry for inspection.
Consider restoring a fragment when it supports an identified signal, return
path, or other design requirement. The number of discarded fragments is a
diagnostic, with no target count. Power distribution has its own criteria.
Evaluate a power pour against the source-to-load path, including narrow
sections and current through vias. On a buck converter, review the switching
loop and switch-node copper separately from signal ground coverage.

**Acceptance.** On a copy of the STM32G474 buck board, use `SWCLK` and
`SWDIO` as the selected nets and bottom `GND` as the reference for their
top traces. Introduce a known gap under a trace while keeping zero air
wires. The report must locate it, and a reviewed reroute that removes the
gap must show the improvement before apply. Compare the saved case with
and without the stitch at `C2.2`. Its 70.30 mm² of recovered copper counts
as a coverage improvement only where the measured intervals change.
Confirm that preview leaves the live board intact and apply reproduces the
checked result without new DRC errors or connectivity regressions.

### R3 — pad escape and planned local rerouting

The first pad escape strategy adds exact-checked local paths from a pad
to several nearby nodes outside its copper. Off-grid pads can use a
45° elbow. The main search keeps its configured grid and expansion budget.
The option is available to reviewed builtin and regional routing.
See [pad escape trials](mcp-routing-reliability.md#pad-escape-trials).

With pad escape enabled, the initial route order favors pads with fewer
exclusive exits, then fewer exits overall. Exits are shared when pads
from different nets can enter the same grid node on the same layer.
An explicit `critical_nets` tier comes first. A lower priority route
cannot rip up copper generated for a critical net in the same run.
These are ordering and protection rules; critical connectivity still
needs to be checked on the candidate.

Plan interacting LQFP fanouts jointly when the order alone is not enough.
Add local refinement for paths that need more than one bend, and compare
candidate exits that overlap geometrically even when their grid nodes
differ.

The first regional workflow accepts a rectangular routing area, named nets,
and an explicit rip-up list of trace and via UUIDs. It verifies protected
copper and uses the existing candidate, preview, apply and undo path.
See [regional builtin routing](mcp-routing-reliability.md#regional-builtin-routing).

The first multi-order comparison keeps options, placement and source revision
fixed. It ranks initial orders by DRC, connectivity (including critical nets),
via count and total trace length, retains exact candidates, and recommends a
candidate without applying it. See [routing_compare](mcp-design.md#reviewed-builtin-routing).

The optional `neck_down` rule lets a short pad escape use a smaller width
before joining a full-width grid path. The preview reports the reduced
segments and lengths by net. See [the mixed-width STM32 replay](mcp-routing-reliability.md#short-pad-escapes-on-a-wide-net).

The opt-in congestion history records contested grid tiles after successful
rip-ups and raises their costs in later soft searches. See [the DC/DC replay](mcp-routing-reliability.md#congestion-history-and-stm32-with-a-buck-converter).

Add corridors and evaluate which conflicts need joint replanning or finer
local search. Track connected copper islands per net as well as air wires,
since an air-wire tree can change during routing. Add a disturbance metric
when comparing alternatives that can change existing copper.

`routing_via_prepare` connects two selected same-net anchors on different
layers with one through via. It accepts an exact location or searches
near the first anchor and checks both short leads. It preserves existing
traces and vias and uses the usual candidate review, DRC and undo path.
See [the STM32 local via replay](mcp-routing-reliability.md#local-via-search).

Acceptance: improve fixed pad-access and congestion cases without DRC regressions;
measure against unchanged baseline placements, rules and budgets. Include the
preserved STM32 access case, without assuming a particular fix.

### R4 — local interactive routing shared by human and agent (proposal)

Evaluate `pnsrouter` through a small adapter for snapshot/rules → local route or
shove → preview/change set. Reuse the common topology construction and acceptance
path. Start with one trace, obstacle displacement, via drag, cancel and undo; test
against existing board anchors and permitted changes before adding more surface.

Mouse input and MCP waypoints drive the same domain operation. The host owns
snapping, rules, protected objects and commit. The library does not become a
second persistent board. Checked unit/range conversions and unsupported shapes
are part of the adapter contract. Retain the adapter only if real cases justify
its integration and maintenance cost.

### R5 — agent planning and bounded placement experiments

Expose analysis, candidate comparison, soft corridors versus mandatory waypoints,
region reroute and explicit budgets. An agent chooses a strategy between bounded
algorithm runs rather than submitting thousands of individual coordinates.

`routing_copper_prepare` already combines explicitly selected unlocked-device
moves/rotations and copper changes in one checked candidate and undo operation.
It does not select coordinates automatically or change footprint sides. P2 adds
durable sourced requirements, and P4 uses them for bounded placement alternatives.
Pin swaps require an explicitly permitted mapping, circuit updates and a firmware
pin-map consequence; changing board labels is insufficient. Circuit templates
remain deferred. Native file compatibility applies to all of these extensions.

See the [Foilfern agent PCB workflow](agent-pcb-workflow.md) for the staged
placement, mechanical and silkscreen checks, evidence requirements and
real-board benchmark sequence.

Acceptance: an agent resolves a blocked local region using diagnosed alternatives,
while preserving protected geometry and reporting all placement or pin changes.

### R6 — electrical specialization and reproducible quality gates

Add coupled differential routing, length/skew targets and electrical
return-path analysis after ordinary routing and topology are reliable. Use
the [geometric ground coverage check](#ground-reference-coverage-and-targeted-stitching)
as an earlier diagnostic. Numerical helpers
can come from `pcb-toolkit` through a unit-checked boundary, with model name,
validity range and validation status exposed. Formula output is not a field
solver or proof of whole-board signal integrity.

Evaluate teardrops at pad and via entries as a separate copper operation.
Preview the additional copper and verify its net connection and clearances
in the native board format.

Keep fixed synthetic problems, real native boards and independent verification.
Compare algorithm-only, agent-guided fixed placement and permitted placement
changes separately, using matched constraints and budgets. Measure fully routed
and clean boards, remaining connectivity, changed copper, vias/length, execution
and agent cost, save/reopen and optional upstream acceptance. Replay records
include source identity, algorithm revision, settings, seed and operations;
separate timing from deterministic semantic comparisons. Reserve held-out boards
to avoid improving only the familiar corpus.

## External projects and integration discipline

- `pnsrouter`: candidate reusable local geometry component in R4, not another
  full-board autorouting workflow to add immediately.
- `fastroute`: initially an external comparison oracle through existing DSN/SES.
  Consider a production executor only after matched board checks demonstrate
  enough benefit; do not maintain redundant full-board engines indefinitely.
- `metal-route`: reference for congestion, diagnostics and benchmark methodology;
  borrow bounded ideas and prove them locally rather than importing the stack.
- `pcb-toolkit`: targeted electrical calculations when a concrete workflow needs
  them, preserving stated approximation limits.
- `circuitry`: defer until Circuit JSON interchange is an actual requirement.

Native file compatibility, correctness and useful local agent operations take
priority over additional backends, 3D features, calculators and catalog surface.
