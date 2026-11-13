# Foilfern roadmap

Updated 2026-11-12. Capability baseline reviewed against `f3c3fda` (including
the merged `work` branch); the P1a transaction decision was added after that review.
This is the delivery order for new work. [Status](status.md) describes what
exists; [routing service design](routing-service-design.md) owns the routing
contract; [COMPAT.md](../COMPAT.md) records upstream differences and gaps.
The old M1–M5 porting labels and R0–R6 routing labels remain useful historical
references, but are not a second priority queue.

## Goal and baseline

Let an engineer or agent create, update and finish an editable PCB design,
with reviewable changes and evidence tied to the saved result. Preserve native
LibrePCB interoperability, integer geometry, the shared project model and undo.
Domain operations belong in core/editor; GUI and MCP consume the same behavior.

Already available: native project/library editing, migrations, ERC/DRC,
manufacturing outputs, standalone/embedded MCP, desktop and library editors,
physical 3D/STEP output, and revision-bound routing candidates. Local moves and
rotations can already be combined with explicit copper repairs. Connectivity
progress, builtin-order comparison, baseline-check reuse and trace-path
measurement are implemented. These are foundations, not new milestones below.

The remaining product gap is turning compact design intent into useful checked
changes with less coordinate-by-coordinate planning. No stage is complete merely
because its tools exist or one board has zero air wires.

## Delivery order

| Stage | Result | Dependencies | State |
| --- | --- | --- | --- |
| P0 | Identifiable builds, concise responses and measured operation costs | Existing API | Complete; [release replay](benchmarks/esp-pd-sink/review/performance/p0/REPORT.md) |
| P1 | Declarative schematic/design updates which preserve manual work | P0 evidence conventions; P1a transaction foundation | In progress; 36-component creation and local rewiring verified |
| P2 | Persisted requirements with explicit enforcement and evidence | P0; shared identity decisions with P1 | Planned; external review prototypes only |
| P3 | Bounded long operations with progress and cancellation | P0 timing; existing candidate lifecycle | Planned; router callback only |
| P4 | Diagnosed local repair with bounded placement alternatives | P2, P3 | Planned; explicit repair primitives already implemented |
| P5 | Visual candidate review in the desktop editor | Existing candidates; P2/P4 extend the displayed evidence | Planned |
| P6 | Reproducible end-to-end acceptance across boards | P1–P5 for final acceptance | Corpus/evidence partly available |

Start with P0, then P1. P2 and P3 can be developed independently once their
contracts are defined. Select and freeze the P6 corpus before tuning P4.
A P5 overlay for existing candidates can ship before automatic P4 planning.

## P0 — build identity, response size and operation costs

Scope: extend `server_info` with source commit, dirty/unknown source state,
Cargo profile and an explicit API capability version. Define a compact response
mode with detailed queries still available. Measure snapshot/extraction, source
checks, search, candidate checks and apply checks separately.

Done when:

- **Implemented.** An installed binary reports its own build identity without
  reading a checkout at runtime. Builds outside Git report unknown provenance
  explicitly. See the [MCP contract](mcp-design.md#tool-surface) for the scope of
  the tracked-source check and Cargo profile label.
- **Implemented.** `device_place` accepts `include_pads: false` and returns
  identity, placement, revision, warnings and a change summary. `component_get`
  provides detailed read-back. Schema and stdio tests cover the option and the
  default full response.
- **Verified.** The release report identifies the snapshot, requests, binary
  and profile. It records five samples per case and cache condition, with
  separate prepare, preview and apply times and full validation evidence.
- **Verified.** The original OLED and competing-route requests reproduce
  baseline reuse. All competing-route candidates expose the plane-fed GND
  break and refuse apply. Candidate checks and non-empty apply checks ran fresh.

The existing 25.598 → 13.915 s OLED measurement is a small-sample result for
repeated preparation, not a target for every host or evidence of faster apply.
Further optimization follows stage measurements; do not assume A* is dominant.

## P1 — declarative design updates

Scope: a versioned structured document for components, selected library identities,
values, named pin connections and optional placements in mm. Resolve it through
existing editor commands, preview the semantic diff and apply one transaction.
Choose the wire/schema representation before adding convenience YAML syntax.
Use the P1a transaction boundary for reconciliation.

The [EN/RESET document](examples/esp32-c3-en-reset.json) binds existing component
UUIDs, checks net memberships and previews two local wire groups. It normalizes
the pull-up value and places and rotates the capacitor and switch. Undo/redo
and native save/reopen preserve circuit connectivity and the routed board.
Reapplication gives an empty diff and leaves unmanaged placement intact.
The document also assigns conventional capacitor and push-button artwork to
two embedded symbol UUIDs. The preview reports all uses of each shared symbol.
The [ESP32-C3 crystal document](examples/esp32-c3-crystal.json) selects artwork
with explicit roles for two resonator terminals and two case terminals. Net
assertions verify the existing circuit, including the case ground connections.
The [decoupler document](examples/esp32-c3-add-decoupler.json) adds a component
to an existing sheet and connects exact package pads to existing nets. The
frozen preview includes any required project library additions.
The ESP-PD-Sink USB Power Delivery block is the readability reference.
Preview can report annotation overlaps for a selected group of components and
move a reference or value independently with an expected-position check.
`design_preview` can place selected members in a bounded row or column with
an optional alignment coordinate. It keeps unrelated geometry fixed and
carries each movable symbol's independent wire stubs with it. A preview can
also draw explicit orthogonal wire paths, junctions and notes around existing
pins, with checks for new wire crossings and symbol collisions. Placement
and explicit rewiring can share a preview, with bends and labels anchored
to final pin coordinates. On the ESP32-C3 dev board, moving R1, C3 and RESET
redrew three segments and preserved the other 24, the netlist and the fixed
module and supply symbols. Undo/redo restored both layouts, and replay after
save/reopen gave an empty diff.

From a blank native sheet, one document created all 36 components and 29
nets of the saved ESP32-C3 comparison, matching its physical-pad membership.
The replay used the corrected comparison library embedded in the original
project. A [second document](examples/esp32-c3-devboard.json) created the
ESP32-C3-WROOM-02 dev board with standard library symbols, explicit routes,
notes and initial reference and value placement. Both saved projects
passed ERC in Foilfern and upstream LibrePCB 2.1.1, and both documents replayed
with an empty diff after reopening. These runs covered the schematic.

Done when:

- The saved 36-position, 29-net ESP32-C3 design can be created from one document.
  Ambiguous parts/pins and unsupported fields produce actionable errors before
  live edits; resolution records exact library and physical-pad identities.
- Reapplying the same document, including after save/reopen, gives an empty diff
  and adds no undo entry. UUIDs remain stable across a designator rename.
- Changing one value preserves existing component UUIDs, placements and routed
  copper. Adding a decoupler changes only the explicitly affected objects.
- Ownership of fields is documented: omitted fields preserve manual edits;
  conflicting edits to managed fields are reported for review. Deletion and
  connectivity changes explicitly show any copper that must be invalidated or
  removed. Unrelated objects are not recreated.
- Preview changes no live state. Stale apply refuses; apply/undo/redo include
  library additions and assets as well as model mutations. A failing batch
  restores model and file contents without committing an undo entry or changing
  the public revision, subject to the P1a atomic transaction boundary.
- Native save/reopen and an upstream CLI check pass on the created/updated
  example, with inherited and unverified findings listed separately.

The native project stays authoritative. Reconciliation is a domain service,
not a second PCB model or a sequence of unrelated MCP writes.

### P1a — atomic failure rollback without published changes

Implemented in core batches, standalone editor commands and grouped MCP writes.
The public revision advances for published changes.
For an explicit atomic transaction whose intermediate state is not externally
observable, buffer change events until commit. Successful rollback discards the
buffer and leaves the public revision unchanged; do not rewind or reuse published
journal sequence numbers.

Ordinary undo/redo and cancellation of already-visible interactive edits publish
new events and advance the revision. An interactive undo group is not automatically
an isolated transaction. Detached snapshots remain the preview mechanism, allowing
long work without changing or locking the live project for the whole operation.

Done when:

- The atomic boundary covers model mutations, project-library additions/removals,
  asset writes and nested commands. Intermediate state cannot reach scene/MCP
  consumers or candidate/cache capture. Nested success remains unpublished until
  the outer commit; nested failure discards only its own pending changes.
- A successfully rolled-back failure restores data, transactional files, dirty
  state and undo/redo history, with no public journal/revision change. Derived
  data is restored or safely invalidated. Rollback failure is reported explicitly
  and cannot be presented as an unchanged, successful rollback.
- Commit publishes the accumulated events with fresh increasing sequence numbers.
  Failed transactions cannot evict previously published journal entries. Tests
  cover nested failures and batches larger than the journal capacity.
- Existing routing candidates and reusable baselines remain valid after an
  invisible failed transaction when all source context is restored. A published
  edit, its undo, and a different subsequent edit never reuse a revision or admit
  a candidate from the wrong state. Save/path/session guards remain effective.
- Scene consumers observe undo/redo and cancellation of visible interactive edits
  correctly. Tests exercise these paths alongside atomic command failures and
  retained redo history. `UndoStack::state_id()` is not substituted for the public
  revision or treated as a complete candidate/cache identity.

## P2 — persisted requirements and evidence

Scope: start with fixed devices, allowed placement regions, protected copper,
allowed routing layers, via prohibition and selected reference-coverage targets.
Reuse native DRC/keepout/lock fields where their semantics match. Additional
requirements need a versioned project-associated representation with object UUIDs,
source, units, severity and the revision of their evidence.

Done when:

- ESP32 RF/crystal restrictions and ESP-PD-Sink fixed interfaces and local
  placement regions survive save/reopen, renames and undo/redo.
- Every supported requirement is evaluated during relevant candidate preparation
  and apply. A hard violation refuses apply; unsupported or unmeasured properties
  report `not_checked`, never a pass. Impedance/thermal claims remain unverified
  unless a separate supported check actually runs.
- A constraint edit invalidates candidates and cached evidence made under the
  old requirements, even when copper has not changed. Deleted/replaced targets
  produce an explicit unresolved-reference result.
- A sidecar, if selected, participates in transactional save, undo and archive
  handling. Upstream open/save preserves it in a round-trip test; no unsupported
  native S-expression fields are introduced.
- A requirement-to-evidence report identifies affected objects, measured value,
  threshold, source and artifact revision. Existing native rules have one owner,
  not conflicting copies in native settings and the requirement document.

The archived `review/intent.json` files are inputs to this design, not an
implemented persistence or enforcement API.

## P3 — progress, budgets and cancellation

Scope: connect worker lifecycle to MCP and the shared service. Report stage,
current net when applicable, elapsed/remaining budget, best fully checked candidate
and stop reason. Limit active workers and queued work independently of the existing
limit of eight retained candidates.

Done when:

- A long preparation exposes progress while the project remains usable. Competing
  requests hit a documented admission/queue bound rather than starting unlimited
  snapshots; stdio and embedded HTTP exercise the same service semantics.
- Cancellation and budget expiry are exercised during search and validation.
  They stop at documented bounded checkpoints; the acceptance report measures
  acknowledgement and worker-stop latency on the frozen fixtures.
- Cancellation leaves live model, history and files untouched. Only an already
  fully validated candidate may remain applicable; unfinished work is explicitly
  incomplete and cannot pass the normal apply gate.
- Closing/replacing a project or editing during work prevents stale publication.
  Worker capacity is released on success, refusal, cancellation and failure.
- Stop reasons distinguish completion, budget exhaustion, cancellation and error.
  A transport timeout is not presented as proof that CPU work stopped.

## P4 — diagnosed local repair and placement alternatives

Scope: use diagnosed blockers to generate a bounded set of local-via, authorized
rip-up, rotation and allowed-area movement alternatives. Build on the existing
candidate service, pad escape and exact checks. Do not change hard requirements
internally to make a route succeed.

Done when:

- The preserved STM32 local-via case and all six ESP32-C3 comparison gaps can be
  repaired through the product's diagnosed alternatives without an external
  scratch coordinate-search program or native-file editing. Record the exact
  allowed movements, copper permissions and budgets used.
- Each proposed repair identifies its blocker, strategy and affected objects.
  Locked devices, protected RF/clock geometry and unlisted copper are preserved;
  whole-board checks catch newly severed plane connections outside the target.
- Results separately report validity and objective progress (`improved`,
  `unchanged`, `regressed`). Compare target connectivity/requirements before
  softer costs: disturbed objects, vias and length. A safe no-op is not described
  as a useful repair merely because it is acceptable.
- Bounded placement candidates apply and undo placement and copper together.
  Side changes and pin swaps are explicitly unsupported until their own circuit,
  attachment and requirement checks exist.
- Failed/budget-limited cases retain useful diagnostics. Cheap search filters
  may reject trials; only full model validation can approve a candidate.
- Matched replays preserve netlist, check results, save/reopen compatibility and
  protected geometry, with runtime and agent effort reported alongside quality.

Extract pad-access, ordering and rip-up responsibilities from the existing router
when needed for these cases. A new engine or wholesale rewrite needs matched
benchmark evidence beyond the familiar fixtures.

## P5 — shared visual candidate review

Scope: display the existing frozen candidate in the desktop board view, with
added/removed copper, moved components, remaining connections and DRC locations.
Use the same IDs, revisions and apply/discard operations as MCP.

Done when:

- Selecting a candidate overlays its changes without modifying the live board,
  dirty state or undo/redo. Baseline and candidate geometry are distinguishable.
- The user can inspect changes and evidence, apply once, discard, undo and redo.
  A concurrent GUI or MCP edit marks the proposal stale and refuses apply.
- GUI application reproduces the same geometry and validation as MCP application
  of that candidate; no search is rerun on apply.
- Headless interaction tests cover the lifecycle and screenshots are reviewed.
  A real desktop pass covers navigation, selection and review on a populated board;
  software/headless success alone is not claimed as platform acceptance.

## P6 — board completion and repeatable acceptance

Scope: make the entire creation/update/review/save/export workflow reproducible.
Use ESP32-C3 for declarative updates and last connections, ESP-PD-Sink for
placement/requirements, and a held-out board selected before planner tuning.

Done when:

- Portable manifests identify source commits, input hashes, requests, constraints,
  build profile and expected semantic results. Run the replays from fresh copies;
  generated UUIDs and volatile export metadata have explicit comparison policies.
- Record fully checked results, remaining connections, per-net regressions,
  changed copper, vias/length, runtime, MCP call count and manual interventions.
  Separate algorithm-only, agent-guided and placement-changing runs.
- For ESP-PD-Sink, review the selected gate-clamp continuation, mechanical
  interfaces, silkscreen, USB/RF references and inherited findings. Every finding
  is fixed or has a sourced, reviewed disposition; zero new errors alone does
  not close the board acceptance. Manufacturing parameters are not relaxed just
  to silence checks.
- Reopen saved native projects, run independent upstream checks and compare
  manufacturing outputs. List unverified electrical, thermal and hardware
  properties explicitly; file/export acceptance is not hardware qualification.
- The held-out result is published even if routing remains incomplete. Keep it
  out of tuning until this evaluation, and retain any new failure as a regression.

Cheap FOCer 2 follows the ESP-PD-Sink mechanical and engineering review, using the
published stack and interfaces for the first comparison. Its power/thermal
qualification requires separate analysis and hardware evidence.

## Deferred scope and maintenance

Keep upstream compatibility fixes and regressions active throughout all stages.
The old M2–M4 UI gaps remain in COMPAT.md; their existence does not make the
implemented editors pending milestones again.

After the stages above, prioritize by observed design friction:

- Table-driven part creation, verified R/C/L/connector templates, and richer
  import/provenance reports using the existing library model.
- Typed mechanical hole/slot and mixed-arc contour editing; connected-footprint
  side changes where real replays require them.
- Interactive shove routing, coupled differential/length routing and electrical
  calculations after their requirements and acceptance cases are specified.
- Package-editor 3D, shared GPU textures, broader STEP/KiCad import coverage,
  printing and ordering integrations.

Additional routing engines and calculators do not bypass native validation.
Update this roadmap when a stage's acceptance evidence exists; keep measurements
in benchmark reports and current API behavior in the owning design documents.
