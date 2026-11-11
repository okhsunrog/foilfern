# Exact DRC copper geometry

DRC has one geometry path. `dashu-ratio` is a regular dependency;
`experimental-exact-copper`, the alternate runner and the rounded-copper adapter
were removed. `run_drc()` returns an ordinary identified result, subject to the
existing failed/stale/quick-result approval protections.

## Implementation

`GeometryGenerator` accumulates independently filled operands, then unions them
as a batch. There is no repeated union of a growing integer contour. Each
operand retains its fill rule: opposite winding in separate objects cannot
cancel, whereas holes and self-crossings within an object obey its fill rule.

`rational_union` computes a noded arrangement with checked `i128` determinants
for integer endpoints and `RBig` fallback for fractional inputs or overflow.
The sweep classifies union, intersection and difference with per-operand winding
counts. Coordinates remain rational through chained operations, including
intersections across all copper layers and subtraction of required annular areas.
Cached exact bounding boxes reject disjoint positive-area intersections.
Overlapping queries clip contours to the common query box with exact
Sutherland-Hodgman clipping before arrangement construction. Signed contour
winding and cancelling bridges preserve holes and disconnected pieces; tests
compare these results with the unreduced operation. This avoids rebuilding a
whole plane arrangement for each small pad or trace.

The consumers now use exact regions:

- Copper-to-copper and copper-to-board clearance intersections.
- Copper-to-hole clearance, including the union across layers.
- Annular-ring coverage: required area minus copper present on every layer.
- Top/bottom copper coverage of plated and non-plated cutout boundaries.
- Silkscreen-to-stopmask intersections using the same geometry generator.

Cutout polylines are split at exact boundary intersections and classified by
exact midpoint winding. Coincident boundary intervals count as covered copper;
isolated point contacts do not create positive-length violation intervals.
Checks without holes or cutouts avoid the corresponding cross-layer preparation.
The earlier skip of annular preparation for hole-free pads remains in place.

The previously rejected batch-union idea is now the primary algorithm. The
integer Clipper variant was rejected because intermediate rounding changed its
result; this implementation retains the intersection coordinates. There is no
need to keep a separate variant restricted to disjoint operands.

## Where integer geometry remains

Input primitives, arc flattening, transformations and offsets still use the
existing upstream-derived shape construction. This work replaces the boolean
composition and its consumers; it does not make circles or offsets symbolically
exact. In particular, plane/polygon clearance shapes are constructed from their
integer source outlines before exact composition, not by rounding a rational
union back into an offset operation.

After an exact violation has been established, its coordinates are rounded to
nanometers for display. Clipper is used there to arrange display rings and add
cut-ins for holes. This rendering step cannot decide whether to emit a violation:
if the integer display geometry collapses, the rounded original paths are kept.
The regression for a 1/3 nm-wide violation verifies it remains reported even
when its displayed polygon becomes a line. Cutout highlights keep clipped
polylines open. The old conversion closed those paths and added an artificial
chord (or a duplicated reverse segment); a regression checks that an open
three-point interval produces two strokes, not three.

Unrelated DRC checks, primitive construction and manufacturing exporters still
use their existing geometry operations. They are not alternate copper backends.
No project serialization, Gerber/Excellon pipeline or file version was changed.

## Correctness and compatibility

Kernel tests cover four fill rules, holes, coincident edges, point/edge touches,
self-crossings, full coordinate range, the known half-grid floating rounding
case, reassociation of repeated unions, and independent point-location/area
oracles. New tests cover rational intersection/difference area identities,
sub-nanometer missing coverage, open boundary clipping, coincident intervals and
collapsed display locations.

The ordinary DRC fixture test checks expected approvals. Failed, stale and quick
results retain their existing cleanup restrictions; there is no special result
identity for this geometry implementation.

Differences at sub-nanometer thresholds are intentional and documented in
`COMPAT.md`: exact nonempty areas or intervals are not suppressed to emulate
Clipper's intermediate rounding. Existing fixture compatibility and real-board
comparisons are recorded below; they do not prove equality on all coordinates.

Final validation:

- `cargo test --locked -p librepcb-core`: **113 unit + 894 integration tests
  passed**, one integration test ignored. `LIBREPCB_CLI` was set, including
  upstream DRC, migration, roundtrip and export oracles.
- `cargo clippy --locked --workspace --all-targets -- -D warnings` passed.
- `cargo fmt --all --check` and `git diff --check` passed.
- Both headless application tests passed: `live_mcp_rule_checks_and_outputs`
  and `drc_removes_disappeared_approvals`. These exercise the standard GUI/MCP
  entry points, result freshness and approval removal after a violation disappears.
- All **37 upstream fixture boards**, **925 total messages**, retain the same
  message identities, severity, texts, descriptions, order and approval multisets
  as the previous regular DRC. Thirteen display location arrays differ on six
  boards, including contour start/order and the open-cutout display correction.
  There were no check errors. Full Debug strings are therefore not byte-identical.

The source fixtures use upstream commit
`7f1b548abb61bd59b2b9c1166eca3e6557480c17`, tests/data commit
`b0724e58b6755354d93c60f3ed3afdf109069927`. The CLI oracle is the official
LibrePCB 2.1.1 binary (revision `06465bf`, file format 2). Full workspace tests
were not repeated; the full core suite and workspace compilation/linting were
checked.

## Reproduction

```sh
cargo build --locked --release -p librepcb-core --example drc_copper_benchmark
target/release/examples/drc_copper_benchmark /path/to/project.lpp full
target/release/examples/drc_copper_benchmark /path/to/project.lpp layers
```

The example exposes no backend selector. Compare revisions using separately
built binaries and identical project snapshots. `full` includes DRC preparation
and emits full Debug messages and approvals; `layers` reports the aggregation
time and retained fractional vertices for each layer.

Local raw records and replay scripts are under
`/home/okhsunrog/tmp_zfs/codex_tmp/drc-review/exact-main/`.

## Comparison with master and upstream LibrePCB

Input: the same unmodified ESP-PD-Sink snapshot for all three implementations,
from `docs/benchmarks/esp-pd-sink/inputs.tar.gz` (SHA-256
`3a74dd46af84f76a59828e2e2a0b63ab695dea81142c7e7bde557e5d3247ccc0`),
4 copper layers, 86 devices and 1,009 traces. Every process opens
`/home/okhsunrog/tmp_zfs/codex_tmp/drc-review/input/foilfern/ESP-PD-Sink.lpp`.
No save or project modification is requested.

Versions and builds:

- Current implementation: `c610b68d00c6ed4a9e06cbf910c0ec0d189a88e8`.
  `worktree/target/release/examples/drc_copper_benchmark`, SHA-256
  `7748eb95fd60f457a09c3318b62510bb1b55444d211f0cdcbaa39f485eb56d85`.
- Master pinned at `dfee64b16e1897bfcd0f73a6750a7928f3a39cac`, built in the
  detached `../master-benchmark` worktree with a read-only timing example calling
  `Project::run_drc()`. No production source changes. Binary
  `master-benchmark/target/release/examples/drc_master_benchmark`, SHA-256
  `777b51005581eea7a0939d8938c3bc241779a0bc594c498da2ef0a1505b6bfcc`.
- Official LibrePCB CLI **2.1.1**, revision **06465bf**, file format 2,
  Qt 6.11.1, prebuilt Linux x86-64 distribution. Binary
  `/home/okhsunrog/tmp_zfs/codex_tmp/librepcb-attributes/librepcb-2.1.1-linux-x86_64/bin/librepcb-cli`,
  SHA-256 `0626428de6f793acdf1f27d1ed8dd9a44a025229b6db0159a1fa0f899412a66b`.

Both Rust versions use Cargo's standard **release** profile and
`rustc 1.99.1 (58cf8f9c3 2026-10-13)` on the same Intel Core Ultra 5 125H
(18 logical CPUs). CLI command: `librepcb-cli -v open-project --drc PROJECT`.
`QT_LOGGING_RULES` enables default-category debug messages so the upstream
internal DRC elapsed time is recorded; other Qt debug categories remain disabled.
The distribution's library-path wrapper is used unchanged.

Three fresh-process samples per implementation in rotated order:
`master/current/LibrePCB`, `LibrePCB/master/current`, `current/LibrePCB/master`.
Task builds and tests finished before these samples. The shared host's unrelated
processes were not stopped. Full DRC includes plane rebuilding, air wires, data
extraction and all checks; project loading is excluded. Upstream's internal
`QElapsedTimer` starts before plane rebuilding, matching that scope. Whole-process
wall/CPU time also includes project loading, printing and process teardown.

Median of three samples (seconds):

| Version | Full DRC | Full DRC range | Whole process wall time | Whole process CPU time |
| --- | ---: | ---: | ---: | ---: |
| Master `dfee64b` | 15.365 | 15.205–15.531 | 15.452 | 29.409 |
| LibrePCB 2.1.1 `06465bf` | 13.897 | 13.631–13.977 | 14.987 | 28.727 |
| Current `c610b68` | **3.332** | 3.310–3.431 | **3.449** | **8.797** |

On this board, full DRC is **4.61× faster than master (−78.3%)** and
**4.17× faster than LibrePCB (−76.0%)**. These numbers compare complete checks,
including preparation, rather than just the union kernel. They are not a
prediction for every board size or geometry distribution.

All six Rust runs emitted the exact same ordered Debug messages, locations and
approval multisets (12 messages). The upstream CLI reported the same 12
severity/text pairs: three errors and nine warnings. Its exit code 1 denotes
these violations, not a failed calculation. The CLI does not expose serialized
approval keys or location polygons, so their equality with the C++ result is
not claimed for this board.

[Individual samples](../benchmarks/esp-pd-sink/drc-timings.json) record timings,
versions and Rust message/approval hashes. Full stdout/stderr and the rotated
three-way replay script are preserved in the local `exact-main` artifact
folder listed above.
