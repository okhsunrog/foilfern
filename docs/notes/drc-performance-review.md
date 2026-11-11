# DRC performance and architecture review

Reviewed base: `33c4dfbafaa88eae2b151c06b17f9a756a5c040c`.
Worktree branch: `review/drc-performance`.
Scope: DRC core, extraction, polygon helpers, GUI/MCP entry points, routing
validation and relevant tests. This review does not change production code.

## Findings

### 1. Reject spatially separated pairs before polygon clipping

`crates/librepcb-core/src/project/board/drc/check.rs:780` enumerates every
copper pair. Net/layer filters help, but each surviving pair invokes a complete
Clipper intersection without an object-level spatial rejection. The drill loop
at line 1150 has the same problem; device clearances at line 2163 test up to
three intersections per pair on each side. These loops have quadratic candidate
counts even on sparse boards.

Start with conservative integer bounding boxes derived from the actual flattened
copper and clearance polygons. Then introduce a per-layer spatial index or sweep
for candidate generation. Preserve both directional checks for unequal
clearances: pruning only copper A versus clearance B can miss the reverse
violation. A box miss may reject; a box hit must retain the exact existing
Clipper operation, tolerance and fill rules. Keep candidate order stable. Validate
coordinate ranges during shape preparation so pruning cannot hide an invalid-input
error that Clipper would previously report.

The copper loop also allocates a `BTreeSet<Layer>` for every eligible pair before
knowing whether it intersects. Use a compact layer mask/range for rejection and
materialize message layers only after a violation. `violations.iter_mut().find`
at line 809 scans all previous violations and compares serialized objects;
replace this with a map from canonical typed object-pair keys to vector indices,
while retaining the vector for deterministic message order. Large planes whose
bounds cover most of the board remain broad-phase candidates; profile those
intersections separately before assuming an object-level index solves all of
this rule's cost. A pad's identity
must be the same across its layer-specific geometry records.

### 2. Stop repeatedly unioning the complete accumulated copper

`clipper_path_generator.rs:48` calls `unite_with` every time a primitive is
added. `add_copper` walks the whole board per layer; `add_trace`, `add_pad`,
`add_stroke_text` and `add_plane` repeatedly rebuild a Clipper arrangement for
the growing accumulator. This is a separate scaling bottleneck from pairwise
clearance checking.

Separate object-shape generation from layer aggregation. Retain normalized
per-object geometry for the other rules; evaluate balanced/chunked unions for
layer copper. Do not concatenate raw polygons and apply EvenOdd once: overlaps
can cancel, and objects currently use different fill rules. Even geometrically
associative operations can change rounded intersection vertices and location
paths when regrouped. This needs differential geometry coverage before landing.

A per-run immutable geometry workspace can share transformed pad copper,
flattened text, drill shapes, layer masks and bounds across copper/board/zone
checks. Start with per-run reuse; it avoids cross-revision invalidation entirely.
`CalculatedJobData::copper` (check.rs:120) can borrow slices instead of cloning
whole layers for read-only consumers. Mutating operations still need owned data.

### 3. Plan only the geometric work that has consumers

All enabled copper layers are prepared unconditionally (check.rs:259), even
when the stage-2 rules are disabled or have no relevant objects. More local
examples:

- NPTH clearance unions every copper layer before discovering there are no
  board/footprint NPTH holes (check.rs:1034).
- Annular-ring checking intersects all copper layers and calls `check_pad`
  for SMD pads with no holes; those calls still pass the complete copper clip
  to a Boolean subtraction (check.rs:1346, 1354, 1407).
- Cutout checking unions and intersects top/bottom copper even on boards
  without closed cutout polygons (check.rs:2336).

Build a small rule/dependency plan from settings and object presence. Skip
pad subtraction when `holes.is_empty()`; calculate global copper combinations
lazily for rules that need them. Preserve separate via-ring checks, which are
scalar and do not require THT pad copper. Count footprint circles and polygons
as well as board polygons when deciding whether cutouts exist.

### 4. Protect completeness and snapshot identity before caching

These are correctness findings, not performance claims:

- `Project::drc_approvals_update` checks `quick` but never `result.errors`
  (check.rs:400–444). After a successful run registers supported approvals,
  a partial full run can remove approvals for messages from failed checks.
  Both GUI and MCP call cleanup on such results. Skip destructive cleanup
  unless the run is complete, and preserve approvals belonging to failed rules.
  Reproduced with upstream DRC board `checkCopperCopperClearances`: register
  the successful result, inject an incomplete full `DrcResult` with one error
  and no messages, and apply the proposed cleanup. Stored approvals drop
  from **71 to 0** without a board edit. This is fault injection, not a claim
  that all checks naturally fail together; one missing rule loses its own
  supported approvals by the same path.
- `drc_worker` builds planes off-lock, applies them, then extracts a fresh
  snapshot under another lock (`librepcb-app/src/rule_check.rs:380–422`).
  `apply_plane_job_result` still applies fragments when revisions differ;
  it merely leaves the layers dirty (`plane_fragments_builder.rs:443`).
  An edit while pouring can therefore produce a snapshot containing new
  object positions and old plane fragments. The GUI may mark the eventual
  result outdated, but the worker has already checked mixed data and
  `drc_finished` can still clean up approvals against it. Reject/retry stale
  plane jobs for DRC, or run all preparation on one isolated snapshot.
- `DrcResult` carries neither board/revision identity nor per-rule completion
  (`check.rs:67`). Add a snapshot token and explicit completeness. Validate
  them when publishing results and cleaning approvals. Cancellation must
  produce an incomplete result, never a valid empty result.
- Plane-job errors are discarded by the full-check preparation when applying
  `job.run()` (check.rs:359). `PlaneFragments` already has `errors`, `finished`
  and `check()`. Preserve those failures in the DRC outcome even if partial
  fragments are retained for display. Routing acceptance currently derives
  completeness from `DrcResult::errors` (`routing_validation.rs:176–196`).
  The omission follows the upstream preparation; changing it is a deliberate
  reliability improvement to document in COMPAT.md.

The routing service already caches a revision/context-bound baseline
(`librepcb-editor/src/routing_service.rs:231, 339`); do not propose that as a
missing feature. Extend reuse only after establishing a common snapshot
contract. A global project revision also changes for derived-data updates, so a
future persistent cache needs a content generation/key that cannot invalidate
itself merely by applying its own calculated fragments.

### 5. Bound concurrency and make cancellation useful

`run_drc` creates one OS thread per copper layer plus ten independent checks
and three stage-2 checks on a full run. Each invocation recreates these threads;
multiple simultaneous boards amplify this. One slow check still serializes its
own entire inner loop. Prefer bounded jobs in the existing Rayon infrastructure,
with explicit preparation dependencies and stable result slots. Profile this
on small and multilayer boards before claiming a speedup. Optimize candidate
counts before parallelizing pair loops.

Add cooperative cancellation between jobs and candidate batches, and propagate
it as incomplete. Keep the synchronous core and caller-owned scheduling model.
Do not hold project locks while waiting for worker jobs.

### 6. Build temporary adjacency data once during extraction

`data.rs:671,696` scans a segment's traces for every junction and via;
`data.rs:630` asks for connected layers for every pad. The underlying
`traces_at` is a linear filter (`net_segment.rs:224`). This is inexpensive for
small segments but grows as O((junctions + vias + pads) × traces) for large
connected nets. Build an extractor-local anchor → count/layer map in one trace
pass. Use a temporary anchor → segment map for air-wire extraction as well
(data.rs:935). This avoids adding another persistent index to every mutation.

### 7. Measure the complete operation and strengthen equivalence gates

`DrcResult::elapsed` starts inside the snapshot checker. It excludes plane
rebuild, air wires and extraction from `Project::run_drc`; callers must not
interpret it as end-to-end latency. Record those phases separately, plus rule
timings, candidate pairs, rejected bounds, exact intersections and vertex counts.
Do not sum parallel job durations as wall time.

Existing DRC approval tests and CLI comparison are valuable. The CLI test checks
approved counts and normalized unapproved text, not every approved message
identity or location geometry. Add baseline-versus-optimized multiset comparisons
of kind, severity, approval, message and locations, preserving duplicate counts.
Cover unequal clearances, disconnected nets (`None`), multilayer vias and pads,
mirrored footprints, holes/slots, arcs, self-intersections and exact tolerance
boundaries. Include incomplete checks, edits during plane preparation and
cancellation before enabling persistent or incremental caches.

## Implementation order

1. Add phase/rule metrics and a stable release replay; close the incomplete
   approval cleanup and mixed-revision snapshot issues.
2. Add precomputed bounds and empty-input skips, preserving exact checks.
3. Add per-layer candidate indexing and typed violation deduplication.
4. Share normalized object geometry and investigate hierarchical layer unions.
5. Measure extraction indexing and bounded scheduling; only then consider
   incremental checks with explicit invalidation domains.

Do not remove full candidate/applied-model DRC from routing validation merely to
improve a benchmark. Keep the current acceptance contract while making each run
cheaper.

## Measurements and validation

The measurements are isolated core replays, not MCP end-to-end benchmarks.
They use an instrumented release example calling the same plane rebuild,
air-wire rebuild, extraction and `run_drc` APIs in that order. The experiment
adds timers plus runtime switches for two deliberately small prototypes:
conservative AABB rejection in the local `intersection` helper, and immediate
return for hole-free pads in the annular-ring check. It does not implement a
spatial index, change clipping tolerance, or batch unions.

- Compiler: Rust 1.99.1 (`58cf8f9c3`), standard release profile, no overrides.
- CPU: Intel Core Ultra 5 125H. This is a shared desktop; unrelated builds
  occurred during parts of the experiment. Treat timings as diagnostic samples,
  not a statistically isolated performance guarantee.
- Build: `cargo build --locked --release -p librepcb-core --example drc_review`.
- Binary: `/home/okhsunrog/tmp_zfs/codex_tmp/drc-review/worktree/target/release/examples/drc_review`.
- Input: tracked `docs/benchmarks/esp-pd-sink/inputs.tar.gz`, extracted under
  the task directory and opened read-only. Archive SHA-256:
  `3a74dd46af84f76a59828e2e2a0b63ab695dea81142c7e7bde557e5d3247ccc0`.
- Board: ESP-PD-Sink, `default`, 4 copper layers, 86 devices, 1009 traces.
- Upstream checkout: `7f1b548abb61bd59b2b9c1166eca3e6557480c17`;
  test data: `b0724e58b6755354d93c60f3ed3afdf109069927`.

First experiment: three fresh-process samples per mode, interleaved
base/bounds/bounds/base/base/bounds. Medians:

| Phase / job | Baseline | AABB prototype |
| --- | ---: | ---: |
| Full preparation + checks | 17.237 s | 16.720 s |
| Snapshot checks only | 16.723 s | 16.134 s |
| Slowest copper preparation layer | 10.293 s | 10.024 s |
| Copper–copper rule | 8.537 s | 6.454 s |
| Annular-ring rule | 6.230 s | 6.196 s |
| Drill–drill rule | 119.8 ms | 14.3 ms |

The pairwise optimization helps the individual rules but barely changes wall
time here: the critical path goes through copper preparation and then annular
rings. Parallel job timings must not be added together. Plane rebuilding itself
is only about 0.5 s, and extraction about 12 ms on this board. This distinguishes
DRC layer copper aggregation from the separate plane-pour phase.

Second experiment: two fresh-process samples per mode, interleaved
base/skip/both/both/skip/base, in a second instrumented binary with separate
copper shape-generation and pair-loop timers. Medians (two-sample midpoint):

| Phase / job | Baseline | Skip hole-free pads | Skip + AABB |
| --- | ---: | ---: | ---: |
| Full preparation + checks | 18.153 s | 12.458 s | 12.519 s |
| Snapshot checks only | 17.526 s | 11.881 s | 11.927 s |
| Slowest copper preparation layer | 10.936 s | 10.688 s | 11.181 s |
| Annular-ring rule | 6.588 s | 1.192 s | 0.745 s |
| Copper pair loop | 9.788 s | 9.457 s | 7.582 s |

Full-time sample ranges are 17.038–19.267 s (base), 11.702–13.213 s (skip),
and 11.170–13.868 s (combined). The direct skip is the strongest low-complexity
candidate on this board. Bounds alone do not fix the critical path; combining
them with the skip does not demonstrate an additional total-time win in this
small noisy sample. The remaining roughly 11-second layer aggregation is the
next substantial target. No speedup factor is extrapolated to other boards.

In the second baseline, copper item generation takes about 0.47 s while the
pair loop takes about 9.79 s. Shape caching helps, but candidate reduction is the
larger opportunity within that rule. Both optimizations preserve the full
ESP-PD-Sink message digest in all 12 runs across the two experiments:
`6dd1dfbf412031954d88a3374bdd25f0387d5e014a0c3a17d722a9a22cd04648`.
The digest is SHA-256 of the harness JSON-encoded full Debug-message array,
including message identity and locations, not merely a message count.

All 30 boards of the upstream `DRC` fixture produced exactly identical ordered
Debug representations of every message (including approvals and location
paths) between baseline and the combined prototype, with no check errors.
The ESP-PD-Sink replay also checks identical full message digests across modes.
The failure-injection approval reproduction is described in finding 4.

The combined prototype also passed all **9 focused release DRC tests**, including
both comparisons with the official LibrePCB CLI 2.1.1 / file format 2, quick
checks, forced plane rebuilding, approval behavior and multithreading. Command:

```sh
DRC_REVIEW_BOUNDS=1 DRC_REVIEW_SKIP_EMPTY=1 \
  cargo test --locked --release -p librepcb-core --test unittests drc -- --test-threads=1
```

`LIBREPCB_UPSTREAM_DIR` selected the checkout above; `LIBREPCB_CLI` selected the
working local oracle wrapper. All test temporaries used the task directory via
`TMPDIR`. No workspace-wide suite was run for this documentation-only delivery;
the experiment does not establish exhaustive equivalence for arbitrary input.

Local diagnostic artifacts, deliberately outside tracked production source:
`/home/okhsunrog/tmp_zfs/codex_tmp/drc-review/`. This contains `drc_review.rs`,
`instrument-v1.patch`, `instrument-v2.patch`, `run_benchmark.py`,
`run_benchmark_v2.py`, binary copies, raw JSONL results, logs and summaries.
Apply the instrumentation patch to the reviewed base and copy the example to
`crates/librepcb-core/examples/drc_review.rs` to reproduce; provide the upstream
checkout through `LIBREPCB_UPSTREAM_DIR`. Prototype code is not proposed as a
finished implementation.

## Implemented follow-up

The correctness fixes landed first in `f2fb53d`:

- Failed, unidentified, wrong-board or stale DRC results cannot update supported
  approvals, delete saved approvals, or upgrade their file-format version.
- Full checks report plane-calculation errors instead of checking partial copper.
- DRC preparation captures the project UUID, board and revision before off-thread
  plane calculation. It validates that token before applying anything, then
  applies planes, rebuilds air wires and extracts the snapshot under one exclusive
  project borrow. The GUI uses this same core path and marks late results stale.
- Background plane results are also rejected when stale, aborted or failed,
  preserving both the current fragments and dirty-layer markers.

Regression tests exercise failed checks with approval versions 1 and 2, an
unidentified result, a wrong board, edit/undo, an edit during plane calculation,
post-extraction edits, real out-of-range plane geometry, late background results,
and aborted/failed plane jobs. The full core integration suite passed **877 tests
with 1 ignored**, including file roundtrips, migrations, exports and official CLI
comparisons. The board subset passed **46 tests**.

The annular-ring optimization landed in `e670266`. Hole-free pads return before
any Clipper operation; boards with no drilled pads also skip the all-layer copper
intersection used by this rule. Via checks still run. A dedicated regression
removes all pad drills, makes every via ring too small, and verifies that every
via violation remains. All **14 focused release DRC tests** passed, including the
official LibrePCB CLI comparisons. Baseline and optimized binaries also produced
exactly the same ordered full messages, approvals and location paths on all
**37 upstream fixture boards**, with no check errors.

### Copper union experiments rejected on correctness grounds

Two implementations were tested and removed before committing production code:

1. Normalize each operand with its original fill rule, then combine the operands
   with one NonZero union. An exact geometric XOR comparison with sequential
   union failed on upstream `v1`, `default`, `top_cu`.
2. Batch only consecutive operands with the same fill rule and strictly disjoint
   bounding boxes, flushing before touching/overlapping operands. Exact XOR still
   failed on upstream `v0.1`, `default`, `bot_cu`: one edge endpoint changed from
   Y = 48,282,196 nm to Y = 48,282,197 nm.

Both passed simple fill-rule tests involving holes, overlapping contours and
opposite winding. Those tests alone would have missed the problem. Clipper's
mixed arithmetic stores integer-nanometer vertices but computes slopes and
intersections in `f64`, rounding the output back to integers. Intermediate
quantization can make the operation sequence observable even when the new
operands are mutually disjoint (they still intersect accumulated copper).
The precise arithmetic step responsible for each fixture difference has not
been isolated; the exact-XOR failures establish the compatibility discrepancy. The committed generator therefore retains
its original operation order; no geometric tolerance was relaxed to accept a
faster result. A future union optimization needs to preserve these intermediate
numeric semantics, or first establish a separately reviewed compatibility
policy. Optimizing the existing Clipper execution and reducing unnecessary
per-rule work are safer next targets than unconditional batch union.

The rejected patches and exact-XOR failure logs remain under the task artifact
root (`implementation/rejected-full-batch.patch`,
`implementation/rejected-disjoint-batch.patch`, `union-geometry-tests.log`,
`union-disjoint-tests.log`). They are experiments, not installed optimizations.

### Release measurements of the implemented skip

These measurements use separate, uninstrumented binaries of the actual
implementation, not the earlier runtime-switch prototypes. The baseline is
`f2fb53dddcf7fbe7e61f4e0bcd5297f4a4ebb2cc` (correctness fixes, no skip); the
optimized version is `e6702663d37e0c826d40685c361bbdaae5696ca4`. Both were built
with the standard Cargo **release** profile and the same temporary
`librepcb-core` example calling `Project::run_drc(board, None, false, ...)`.
Project loading is outside the timer; plane rebuilding, air wires, extraction
and checks are inside. This is a core replay, not MCP transport or GUI latency.

The input archive, upstream/test-data revisions and machine are the same as
above. Six fresh processes ran in the order base/skip/skip/base/base/skip after
this task's build and test processes finished. The board was opened read-only.

| Measurement | Baseline | Skip empty annular-ring work |
| --- | ---: | ---: |
| Full DRC median | 17.109 s | 10.769 s |
| Full DRC range | 16.098–17.988 s | 10.763–11.042 s |
| Checks-only median | 16.580 s | 10.251 s |
| Process CPU time median | 32.904 s | 26.940 s |

Full elapsed time decreased **37.1% (1.59× speedup)**; process CPU time decreased
**18.1%**. CPU time includes process startup and loading and is not the same
interval as the wall-time DRC timer. All six full-message hashes equal
`6dd1dfbf412031954d88a3374bdd25f0387d5e014a0c3a17d722a9a22cd04648`.
This small, interleaved sample on a shared desktop is evidence for this board,
not a performance guarantee for other designs.

Binary copies under
`/home/okhsunrog/tmp_zfs/codex_tmp/drc-review/implementation/`:

- `drc-base`, SHA-256
  `b94b745f4c1a3a4928748b9d842a6a70bf371d81daec6355a183f20d5be98e58`.
- `drc-skip`, SHA-256
  `457ae0c3a095d148a0ff117915fda0c3152da1390867a2e11025ee3cafd74a9a`.

The same directory holds `compare.py`, `measurements.json`, raw JSONL results,
and `check_messages.py` with both 37-board message manifests. The example source
is preserved as `../drc_benchmark.rs`; it was removed from the source tree after
building the binaries.

### Derived-data race regression

One headless GUI run failed because a repeated DRC retained its old messages;
a second run passed, exposing a timing-sensitive case. A deterministic core
regression then captured a DRC preparation, rebuilt air wires without changing
any model input, and applied the preparation. The initial revision guard rejected
it with `StaleSnapshot`.

Preparation and plane-result application now consult the change journal and
permit intervening `AirWires`/`PlaneFragments` events only. Any actual model edit,
including edit/undo, or an expired journal cursor still rejects the job. The
prepared full check must apply its own complete plane result before extraction;
the regression compares both fragments and dirty layers with a fresh full
rebuild. It also covers quick preparation. Already-extracted DRC results retain
the strict revision guard for approval cleanup. The GUI regression now explicitly
checks that the repeated run has no execution error and is up to date, before
checking disappeared messages and approval deletion.

### Arithmetic and rounding alternatives

Moving rounding is more relevant to batching than replacing the scalar type.
There are two independent approximations today: arc flattening (DRC tolerance
5,000 nm) and rounding polygon intersections back to the 1 nm grid. Changing the
second does not make the first exact. Neither small coordinate differences nor
an area tolerance alone prove identical decisions: touches, narrow gaps, hole
nesting and clearance-threshold cases need separate checks.

| Approach | What it changes | Compatibility and performance implications |
| --- | --- | --- |
| Keep current rounding; optimize candidate selection, caching and Clipper execution | Preserves the current numeric pipeline | Best fit for strict upstream equivalence. Does not itself enable regrouping unions. |
| Exact rational intersection, then round after every operation | Removes floating-point error from constructing intersection points | Still has intermediate quantization and order dependence. Can differ from upstream at half-grid boundaries. Exact arithmetic is not automatically faster. |
| Finer fixed-point grid | Moves the quantization scale below 1 nm | Reduces individual grid error but does not remove rounding or guarantee associativity/topology. Scaling reduces coordinate headroom and changes upstream geometry. |
| Decimal arithmetic | Changes the finite number representation | Non-terminating fractions such as 1/3 still need rounding. It does not solve the geometric problem on its own. |
| Preserve rational vertices between polygon operations; round only for display/output | Removes intermediate grid quantization from the linear-polygon stage | Makes exact set operations associative geometrically, enabling different grouping. Requires exact constructions, predicates and topology handling throughout the algorithm; replacing `IntPoint` alone is insufficient. Ring order/serialization still need deterministic canonicalization. |
| Keep original operands and evaluate one union | Avoids feeding rounded intermediate unions into later unions | A cheaper experiment with the existing Clipper, but already changes geometry on fixtures. Exact rational evaluation would remove its intersection quantization as well. |
| Exact predicates/constructions with an adaptive fast path | Uses cheap arithmetic when an error bound proves the answer, exact arithmetic otherwise | Potential way to contain the cost of a new exact kernel. Requires certified bounds, not a guessed epsilon, and still needs a decision about matching legacy rounded results. |

Rational vertices do not require floating-point arithmetic or a hand-written
big-integer implementation. `num-rational::BigRational` and `dashu-ratio::RBig`
provide exact fractions over arbitrary-precision integers. `dashu-ratio::Relaxed`
avoids requiring a fully reduced fraction after every operation; this can reduce
GCD work but may grow intermediates. Both candidates are pure Rust. The
comparison prototype uses these crates only in a temporary diagnostic package,
added using `cargo add`; no new production dependency was introduced.

For original integer line segments, determinants can be computed on integers,
and the intersection coordinates retained as numerator/denominator pairs. A
checked `i128` implementation is useful for bounded coordinates, but its
intermediate products do not cover the full Clipper input range of
±(2^62−1) nm. An extreme diagonal crossing whose exact answer is simply (0,0)
already overflows the straightforward `i128` formula. Arbitrary precision (or a
formally bounded wide-integer path with an exact fallback) avoids silently
shrinking the accepted input range.

Rounding only at the output boundary also needs care. Independent coordinate
rounding can alter topology. Snap rounding transforms the segment arrangement
as a whole; iterated snap rounding additionally separates vertices from
nonincident edges, at a cost in geometric approximation. These are different
semantics from preserving the existing Clipper output. See the primary
references: [CGAL exact predicates and constructions](https://doc.cgal.org/latest/Kernel_23/classCGAL_1_1Exact__predicates__exact__constructions__kernel.html),
[CGAL 2D snap rounding](https://doc.cgal.org/latest/Snap_rounding_2/index.html), and
[Clipper's rounding-artifact discussion](https://www.angusj.com/clipper2/Docs/Robustness.htm).
The last reference also discusses Clipper1 artifacts; this repository still
ports Clipper1, not Clipper2. CGAL is a conceptual reference here, not a proposed
C++ dependency.

The architecture experiment proposed here was an isolated DRC copper-aggregation
prototype with retained exact vertices and rounding only at its interface.
Keep the existing geometry/export implementation available as
the upstream oracle. Compare actual violations, approval identities, locations,
topology and exported data separately; file-format interoperability alone does
not establish behavioral equivalence. This is a larger numeric-contract change,
not a compatible scalar substitution ready to enable by default. The subsequent
[exact copper experiment](drc-exact-copper.md) implements this behind an optional
feature using only `dashu-ratio`, with independent geometric correctness tests.
`BigRational` was used only in the standalone scalar probe below.

### Rational-crate probe

The standalone Rust 2024 diagnostic package
`/home/okhsunrog/tmp_zfs/codex_tmp/drc-review/implementation/rational-probe/`
compares `num-rational 0.4.2` (`BigRational`, using `num-bigint 0.4.8`) with
`dashu-ratio 0.6.1` (`RBig` and `Relaxed`, using `dashu-int 0.6.2`). Its lockfile
pins all versions. Build: `cargo build --locked --release`; binary:
`target/release/rational-probe` under that package. Compiler and host are the
same as the DRC measurements.

Two algebraically equivalent implementations are measured: all intermediate
values represented as rationals, and integer determinants with rationals
constructed only for the two output coordinates. Inputs are 2,000 generated
crossing segment pairs and 2,000 deliberately constructed half-grid crossings.
An additional extreme crossing at ±(2^62−1) verifies the full coordinate range.
All six implementations agree exactly on all **4,001** outputs. This tests
arithmetic construction only, not a polygon sweep or DRC compatibility.

Three interleaved samples per implementation, 40,000 intersections per sample,
standard release optimization, black-boxed inputs/results. Median time per
intersection:

| Representation | Rational arithmetic throughout | Integer determinants, then rational output |
| --- | ---: | ---: |
| `num-rational::BigRational` | 24.340 µs | 3.194 µs |
| `dashu-ratio::RBig` | 2.426 µs | 0.737 µs |
| `dashu-ratio::Relaxed` | 1.448 µs | 0.471 µs |

These are small diagnostic implementations using the same algebra, not a general
ranking of crate performance. No claim is made about full polygon throughput,
memory growth across chained unions, or whole-DRC speed. This task's builds had
finished; unrelated work on the shared host was not stopped. The integer-first
formulation itself matters substantially, independent of crate selection.

A separate bounded line-intersection probe compared the existing-style `f64`
formula and checked `i128` construction with immediate rounding: median 24.46 ns
and 79.51 ns respectively. These have different output representation and input
mix from the rational table and are not directly comparable benchmark cases.
Of 220,000 inputs (including 20,000 deliberate half-grid cases), 6,045 rounded
outputs differed. This is not an estimate of real-board error frequency. For
example, two segments
`(620776482,382053621) → (455267721,359395578)` and
`(451017960,427449599) → (625026243,313999600)` meet at
`(538022101.5,370724599.5)` nm. Exact ties-away rounding gives
`(538022102,370724600)`, whereas the floating expression gives
`(538022102,370724599)`. This illustrates why even replacing the arithmetic
without moving rounding can change the upstream result. It is not a minimized
reproduction of the earlier complete-union fixture failures.

The subsequent kernel uses **checked integer determinants plus `RBig` output**,
falling back to `RBig` arithmetic on overflow or fractional inputs. It does not
add `num-rational` or use `Relaxed`: faster transient arithmetic in this short
probe does not establish bounded intermediate growth across operation chains.
The crate APIs and representation guarantees are documented in
[`num-rational`](https://docs.rs/num-rational/latest/num_rational/type.BigRational.html)
and [`dashu-ratio`](https://docs.rs/dashu-ratio/latest/dashu_ratio/).
Raw timings are in `implementation/rational-probe-final.log` and
`implementation/intersection-probe-final.log`; the latter's source is
`implementation/intersection_probe.rs`.

Final validation of the derived-data follow-up (`143b74c`): **48 board tests**,
**2 headless GUI/MCP tests**, `cargo fmt --all --check`, and
`cargo clippy --locked --workspace --all-targets -- -D warnings` passed.
The entire workspace test suite was not rerun. Existing Slint layout build
warnings and the dependency's future-compatibility notice remain unrelated.
