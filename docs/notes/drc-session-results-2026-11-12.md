# DRC review and optimization session results

Recorded: 2026-11-12. The owner accepted the final change, including Rayon
parallelization. The agreed optimization pass is complete; further work is
paused until a concrete slow workload justifies it.

Branch: `review/drc-performance`. Work was performed in the isolated worktree
`/home/okhsunrog/tmp_zfs/codex_tmp/drc-review/worktree`.
The final implementation is commit `9aec02bbeac00ec6c2d4ed9cc676339b1f7e0b4f`;
its measurements are recorded through `0ee0022e70eb13c4292fccccb9b1726ae2cc2f39`.
All implementation and measurement commits were pushed to the branch. This
session did not merge the branch into master or deploy a running application.

## Objective and decisions

Review and accelerate DRC, fixing correctness problems first and covering them
with regression tests. Preserve file interoperability with LibrePCB and measure
release builds on identical board snapshots.

The accepted design has one production implementation. DRC boolean geometry
uses `dashu-ratio::RBig`; there is no experimental feature, backend selector or
alternate DRC runner. The initially rejected batch-union optimization became
viable after removing intermediate intersection rounding. Adding decimal or a
finer fixed-point grid alone would not remove that quantization problem.

Input dimensions remain integer nanometers. Primitive construction, arc
flattening, transformations and offsets retain the existing geometry pipeline;
the change does not make curves symbolically exact. File formats and
manufacturing exporters were not changed.

## Correctness fixes

- **Approval protection:** cleanup requires a complete, successful, identified
  result for the current project, board and revision. Failed, incomplete, quick,
  stale or unidentified results cannot remove stored approvals or advance their
  version. The original failure was reproduced by injecting a failed full DRC
  result after a successful run; it could otherwise remove supported approvals.
- **Consistent preparation:** DRC rejects intervening model edits before applying
  calculated planes, then applies the prepared result and extracts check data in
  one exclusive operation. Failed or incomplete plane rebuilding is propagated
  instead of checking partial copper.
- **Derived-data refreshes:** intervening air-wire or plane-fragment refreshes
  are allowed when the change journal proves no model input changed. Actual
  edits, edit/undo and expired journal cursors still reject preparation. Already
  extracted results retain strict revision checks for approval cleanup.
- **Exact violation decisions:** positive-area overlaps and missing copper are
  classified before display rounding. A sub-nanometer violation remains reported
  even if its display contour collapses to a line.
- **Open cutout highlights:** clipped intervals remain open; display conversion
  no longer adds an artificial closing chord. Coincident boundary intervals
  count as covered, while isolated point contacts have no positive length.

These changes and their regression tests are described in the
[review](drc-performance-review.md) and
[geometry report](drc-exact-copper.md). Intentional differences from upstream
are recorded in [COMPAT.md](../../COMPAT.md).

## Implemented optimizations

1. Skip annular-ring geometry for pads without drills and avoid cross-layer
   work when the relevant holes or cutouts are absent. Via annular rings retain
   their direct diameter check.
2. Collect independently filled copper operands and union them as a batch.
   Exact vertices survive subsequent unions, intersections and differences.
   Checked integer determinants handle suitable inputs; `RBig` handles
   fractional coordinates and arithmetic overflow without narrowing the accepted
   coordinate range. Each operand retains its own fill rule and hole winding.
3. Select copper-pair candidates through an `rstar` R-tree. Bounds include both
   copper and clearance areas, so asymmetric clearances remain covered.
   Rational bounds are rounded outward for indexing, using `i128` arithmetic;
   exact geometry still decides violations. Layer overlap uses 64-bit masks.
4. Cache each contour's bounds, reject remote contours before cloning and skip
   unnecessary clipping operations. NPTH and annular-ring checks combine only
   the geometry clipped to each local query box, retaining enclosing contours,
   holes and missing-layer semantics.
5. Remove collinear vertices with cyclic neighbor links and a work queue,
   followed by one compaction. This replaces repeated scans and vector removals
   with a linear number of vertex checks.
6. Evaluate independent copper-pair groups through Rayon, already a project
   dependency. Workers retain violations rather than a global quadratic list of
   pairs. Indexed collection and a sequential final merge preserve original
   pair order, layer merging, locations and approvals. Errors are selected in
   input order rather than worker completion order.

The last step changes pair evaluation only. It does not replace the entire DRC
scheduler or parallelize all object-geometry construction.

## Performance evidence

The main input is the unchanged ESP-PD-Sink snapshot: four copper layers,
86 devices and 1,009 traces. Input archive:
`docs/benchmarks/esp-pd-sink/inputs.tar.gz`, SHA-256
`3a74dd46af84f76a59828e2e2a0b63ab695dea81142c7e7bde557e5d3247ccc0`.

Measurements use standard Cargo release builds, Rust 1.99.1
(`58cf8f9c3 2026-10-13`), on an Intel Core Ultra 5 125H with 18 logical CPUs.
Full DRC includes plane preparation, air wires, extraction and checks; it
excludes project loading. These are core replays, not MCP request-to-response
latencies. Own builds/tests finished before the final timing series; unrelated
activity on the shared host was not stopped.

Historical comparisons were separate interleaved series, with three
fresh-process samples per version:

| Series | Implementation | Median full DRC |
| --- | --- | ---: |
| Rational geometry comparison | Master `dfee64b` | 15.365 s |
| Rational geometry comparison | LibrePCB 2.1.1, revision `06465bf` | 13.897 s |
| Rational geometry comparison | Rational DRC `c610b68` | 3.332 s |
| Local-query comparison | Previous rational DRC `0636f99` | 3.530 s |
| Local-query comparison | Local queries `dc9577d` | 2.367 s |
| Local-query comparison | Master `dfee64b` | 15.434 s |
| Local-query comparison | LibrePCB 2.1.1, revision `06465bf` | 13.867 s |

Within the local-query series, full DRC became 32.9% faster than the preceding
rational implementation, 6.52 times faster than master and 5.86 times faster
than LibrePCB. These ratios apply to that series, not the final Rayon revision.

The final comparison used five alternating before/after pairs per project:

| Project | Before Rayon (`a674dd2`) | Rayon pairs (`9aec02b`) | Change |
| --- | ---: | ---: | ---: |
| ESP-PD-Sink | 2.259 s | **1.645 s** | **−27.2%** |
| Gerber Test | 1.302 s | 1.305 s | +0.2% |
| Nested Planes | 0.358 s | 0.307 s | −14.3% |
| v1 | 0.746 s | 0.759 s | +1.8% |

ESP-PD-Sink ranges were 2.255–2.359 s before and 1.583–1.776 s after.
Whole-process CPU time increased from 5.377 to 5.949 CPU-seconds (+10.6%):
parallelism reduced latency with additional CPU cost. Gerber Test and v1 were
effectively unchanged; their small differences are not established regressions.
Master and LibrePCB were not rerun after the final Rayon change, so no fresh
final-version speedup ratio against either is claimed.

Sample records, binary hashes and build provenance:

- [Rational DRC versus master and LibrePCB](../benchmarks/esp-pd-sink/drc-timings.json).
- [Local queries, including repeated master/LibrePCB measurements](../benchmarks/esp-pd-sink/drc-local-query-timings.json).
- [Final Rayon before/after measurements](../benchmarks/esp-pd-sink/drc-parallel-pair-timings.json).

## Validation and compatibility boundaries

Final implementation validation:

- `cargo test --locked -p librepcb-core`: 120 unit and 894 integration tests
  passed, one integration test ignored. Upstream CLI comparisons were enabled.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`,
  `cargo fmt --all --check` and `git diff --check` passed.
- Parallel-pair regressions cover 1, 2 and 4 Rayon workers, message-pair order,
  merged layer masks, location order, maximum clearance and first-error choice.
- The latest corpus comparison covers 37 boards and 925 messages: full ordered
  messages, locations and approvals match the immediately preceding version,
  with no check errors. All 40 final timing runs also preserve their respective
  project results.

The initial migration from rounded Clipper booleans to rational geometry
preserved all 925 message identities, texts, severities, descriptions, ordering
and approval multisets, but changed 13 display-location arrays on six boards.
Those included contour ordering/start vertices and the corrected cutout chord.
Subsequent local-query and Rayon changes preserve the rational baseline's
locations exactly. Equality with upstream on all possible coordinates is not
claimed: exact sub-nanometer decisions intentionally differ where intermediate
rounding would suppress a violation.

The two headless application tests `live_mcp_rule_checks_and_outputs` and
`drc_removes_disappeared_approvals` passed during the rational-geometry stage.
They were not repeated after the local-query and Rayon follow-ups. The full
workspace test suite was not run; workspace linting/compilation and the full
core suite were checked.

The upstream CLI does not expose serialized approval keys or location polygons.
Its ESP-PD-Sink comparison establishes matching severity/text pairs (three errors
and nine warnings), not equality of those unavailable fields. File migration,
roundtrip and export comparisons remain covered by the enabled core oracles.

## Main implementation commits

| Commit | Result |
| --- | --- |
| `f2fb53d` | Snapshot consistency, preparation failure propagation and approval protection |
| `e670266` | Skip empty annular-ring geometry |
| `143b74c` | Permit verified derived-only refreshes during preparation |
| `c610b68` | Replace production DRC boolean geometry with exact rational operations |
| `dc9577d` | Spatial indexing, contour bounds, local queries and linear collinear cleanup |
| `9aec02b` | Rayon pair evaluation with deterministic merging |

## Reproduction and stopping point

```sh
cargo build --locked --release -p librepcb-core --example drc_copper_benchmark
target/release/examples/drc_copper_benchmark /path/to/project.lpp full
```

Use separate revision builds with the same input and build profile for timing
comparisons. The final benchmark binary SHA-256 is
`25c2169f172f4bf42abf92910a72e679b4b5a3aa42a1981dada0c324e4dd61d8`.
Raw logs, replay scripts and saved comparison binaries are under
`/home/okhsunrog/tmp_zfs/codex_tmp/drc-review/`, particularly `exact-main/`,
`local-queries/` and `parallel-pairs/`.

Further geometry reuse, incremental DRC, broader scheduler changes and other
review suggestions are deferred. They were not implemented by this session.
The owner accepted the completed change; the next action is ordinary branch
integration when requested, rather than another speculative optimization pass.
