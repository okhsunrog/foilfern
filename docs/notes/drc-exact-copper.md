# Exact copper union experiment

The optional `experimental-exact-copper` feature implements an isolated layer
copper union with `dashu-ratio 0.6.1`. It is not enabled by default. Normal DRC,
GUI/MCP/routing entry points and manufacturing exports still use Clipper, even
when this feature is compiled. No file format or stored approval identity is
changed.

## Scope and numeric contract

`BoardClipperPathGenerator::copper_operands()` captures the original copper
objects before their sequential layer-wide unions. Each operand retains its
own fill rule. Opposite winding in separate objects must not cancel; holes and
self-intersections within an object follow its original fill rule.

The new `utils::rational_union` kernel:

1. Uses a conservative x-axis bounding-box sweep to find candidate segment
   pairs, then computes exact intersections and splits overlapping edges.
2. Uses checked `i128` determinants for integer endpoints. Overflow and
   fractional endpoints fall back to `RBig`; coordinates are never converted to
   floating point. Input coordinates retain Clipper's ±(2^62−1) range.
3. Classifies the resulting noncrossing edges with per-operand winding counts,
   combining filled areas by OR. Vertical edges use the filled intervals on
   their left and right. Shared vertices are ordered by exact direction.
4. Reconstructs oriented, canonically ordered rings. `Region::operand()` keeps
   rational coordinates for another union without intermediate rounding.

Only `dashu-ratio` was added as a direct optional dependency. There is no
`num-rational` oracle dependency and no retained `Relaxed` representation.
Independent area/point-location tests provide a geometric oracle. This is an
original kernel because the existing integer boolean kernels quantize their
constructed vertices instead of retaining rational intersections.

The implementation is intentionally a bounded experiment, not a general
replacement geometry library: it currently supports union, and its active-list
sweeps and ring simplification have quadratic worst cases. Allocation counts,
peak memory and adversarial rational denominator growth have not been profiled.

## Compatibility boundary

Input pad/trace construction, transformations, offsets and arc flattening still
use the existing geometry pipeline. Exactness starts at the layer union inputs;
it does not recover precision already lost there.

`run_drc_with_exact_copper()` rounds the final union vertices to integer nm
(ties away from zero) for the existing downstream checks. This adapter is
**lossy and not topology-preserving**. A regression test demonstrates that it
can close a positive 1/3 nm gap. Exact union cannot reproduce all artifacts of
Clipper's sequential rounding, either.

The experimental API deliberately clears the result's snapshot identity.
Therefore `is_complete()` and `is_current()` are false and the result cannot
authorize removal of stored approvals. A board regression verifies unchanged
approvals and their version. Callers inspecting this API must read `errors` and
compare messages; an error-free experimental result is not production approval.

On ESP-PD-Sink, rounded copper differs from the legacy area on top, inner 2 and
bottom copper (27, 7 and 9 nonempty XOR paths respectively). Inner 1 matches.
This is a difference in geometry, not evidence of 43 changed violations.
The full ordered DRC messages, locations and approval multisets match on:

- All **37 upstream fixture boards**, **925 total messages**, with no errors.
- ESP-PD-Sink, **12 messages**, across all six timing runs below.

These comparisons establish compatibility on those inputs, not at arbitrary
sub-nanometer contacts or clearance thresholds. Export code was not switched;
the ordinary core compatibility suite remains the export/file oracle.

## Release measurement

Same release example binary for both modes:

`/home/okhsunrog/tmp_zfs/codex_tmp/drc-review/worktree/target/release/examples/drc_copper_benchmark`

Implementation commit: `2a4483da7624c50542b768d29f19a921af7982cf`, based on
`4b98b3241f4d26995c8b9435396b53f24cf38aed`. The binary was built before the
commit; subsequent changes were tests and documentation only. Cargo's standard `release` profile,
Rust `1.99.1 (58cf8f9c3 2026-10-13)`, Intel Core Ultra 5 125H, 18 logical CPUs.
The normal DRC worker scheduling is unchanged.
Measured binary SHA-256:
`9e281d593781f0b4630515f20555d327c87c9b576345d5b264dff7eef5f33996`.

Board: the unchanged ESP-PD-Sink snapshot from
`docs/benchmarks/esp-pd-sink/inputs.tar.gz`, archive SHA-256
`3a74dd46af84f76a59828e2e2a0b63ab695dea81142c7e7bde557e5d3247ccc0`;
4 copper layers, 86 devices, 1,009 traces. Both modes already include the earlier
annular-ring skip optimization. These are not comparisons against the older
17-second baseline.

Three fresh-process samples per mode, interleaved
`legacy, exact, exact, legacy, legacy, exact`. No task compilation ran during
these six samples. Wall time includes plane/air-wire preparation and checks;
process CPU time includes project loading and all worker threads as well.

| Metric | Legacy union | Exact union + final rounding |
| --- | ---: | ---: |
| Median full DRC | 10.890 s | 8.603 s |
| Full DRC range | 10.272–11.004 s | 8.415–8.833 s |
| Median checks only | 10.388 s | 8.032 s |
| Median process CPU time | 26.606 s | 12.079 s |

Full DRC wall time improves **21.0% (1.27×)**; process CPU time falls **54.6%**.
The improvement in the whole check is smaller than in copper preparation:
independent checks run concurrently and still determine much of the elapsed
runtime. This experiment does not identify the next critical check by timing.

The SHA-256 of compact JSON containing the ordered Debug message strings is
`ee338ed698a3a9ed00c91dcde0a3cc3d283865f552fb7f78b6c990ecf57df7d1`
for all six runs. The benchmark also compares complete approval multisets, not
just message counts.

Reproduce the build and either mode:

```sh
cargo build --locked --release -p librepcb-core \
  --features experimental-exact-copper --example drc_copper_benchmark
target/release/examples/drc_copper_benchmark /path/to/ESP-PD-Sink.lpp legacy
target/release/examples/drc_copper_benchmark /path/to/ESP-PD-Sink.lpp exact
target/release/examples/drc_copper_benchmark /path/to/ESP-PD-Sink.lpp layers
```

The `layers` mode reports input segments, candidate pairs, atomic and boundary
edges, fractional output vertices, collection/union durations and the Clipper
XOR comparison. `intersections` counts discoveries per candidate pair, including
shared endpoints; it is not a count of unique geometric intersection points.
Local raw records and replay scripts are under
`/home/okhsunrog/tmp_zfs/codex_tmp/drc-review/exact-copper/`.

## Tests and next acceptance gate

The kernel regressions cover holes and all four fill rules, opposite winding
across operands, partial/full coincident edges, point/edge touches, self-crossing
polygons, repeated union and operand order, the full coordinate range, a known
half-grid floating-rounding discrepancy, and the explicit final-rounding hazard.
Seeded generated tests independently verify rectangle coverage and point
location; 256 polygon arrangements also check exact union reassociation.

Validation on 2026-11-12:

- Full `cargo test --locked -p librepcb-core --features experimental-exact-copper`:
  111 unit tests and 889 integration tests passed, one integration test ignored.
  The upstream CLI oracle was present, including file/export and DRC comparisons.
- After adding the final half-grid regression, all 10 kernel tests passed again.
  The approval-protection regression also passed in the full integration run.
- `cargo clippy --locked --workspace --all-targets --features
  librepcb-core/experimental-exact-copper -- -D warnings` passed.
- `cargo fmt --all --check` and `git diff --check` passed.
- `cargo check --locked -p librepcb-core --all-targets` passed with the feature
  disabled, checking the ordinary build independently.

The oracle used `LIBREPCB_UPSTREAM_DIR` at upstream commit
`7f1b548abb61bd59b2b9c1166eca3e6557480c17` and tests/data commit
`b0724e58b6755354d93c60f3ed3afdf109069927`; `LIBREPCB_CLI` pointed to the
same-version CLI wrapper. Full workspace tests were not rerun for this core-only
experiment; workspace compilation/linting and the full core suite were checked.

Before enabling this for production, the final rational-to-integer boundary
needs an explicit correctness contract. Keeping exact vertices through the
relevant predicates avoids this particular quantization step; a grid adapter
instead needs well-defined treatment of collapsed gaps, holes and contacts.
Either choice requires targeted threshold/topology regressions and separate
behavioral compatibility review. Faster aggregation alone is insufficient.
