# Reuse full routing baseline checks on unchanged sources

The Rust change in [`eab6191`](https://github.com/okhsunrog/foilfern/commit/eab6191)
retains one complete source DRC result in `RoutingService`. Repeated preparation
on this ESP-PD-Sink fixture improves from **29.967 to 15.075 seconds on average**
for the two repeated samples: **49.7% less time / 1.99× speedup**. The first
preparation and applied-model checks still run full DRC.

This is a Foilfern implementation change driven by the measured PCB workflow.
The [selected gate-clamp project](../gate/REPORT.md) is the test input. Its
native files remain byte-identical after both benchmark runs; no new PCB
revision is selected here.

## Implementation and correctness

The cache belongs to one open-project routing service. Its key includes project
UUID, board, source revision and save-time/file/directory context. It is filled
only after successful publication into that unchanged session, and only from a
complete source check. Incomplete or stale worker completions cannot seed it.
The single retained entry bounds memory; discarding a proposal can leave its
still-current source check available. Closing/reopening creates a new service.

A native detached snapshot omits derived copper and connectivity. A cache hit
therefore still rebuilds planes and forces air-wire reconstruction before the
routing operation. Every candidate receives a fresh full after-check, and every
non-empty applied patch receives a fresh full live-model check. Baseline finding
identities, warnings and per-net connectivity remain part of validation.
`baseline_reused` exposes reuse in candidate metadata and comparison results.
No check is approved away, and no new route-search shortcut is introduced.

Focused validation passed:

- Six editor unit tests cover reuse without live edits, board/revision isolation,
  incomplete/stale publication, save/path context, service ownership, comparison
  reuse and refusal of unsafe candidates.
- All 29 MCP routing-candidate integration tests pass. The plane-connection
  regression test now warms the cache before moving a via outside its reference
  plane, then verifies that the new open connection is detected and apply refuses.
  The save-context test verifies reuse before save and a cache miss afterwards.
- `cargo clippy --locked -p librepcb-editor -p librepcb-mcp --all-targets -- -D warnings`
  and formatting checks pass. The same package optimization overrides listed
  below were used for compilation. Unrelated workspace/UI suites were not run.

## Controlled replay

Both binaries run serially in fresh processes, using separate copies of exactly
one native project. Each process prepares the same explicit two-segment gate
route replacement three times without changing the live model, previews every
candidate, applies the last non-empty patch with its revision guard, and runs
an independent full DRC. The replacement has the same geometric centerline;
it exercises real mutations and checked apply rather than a no-op fast path.

| Wall time, seconds | Before | After |
| --- | ---: | ---: |
| First prepare | 28.427 | 28.035 |
| Repeated prepare 1 | 28.685 | 15.518 |
| Repeated prepare 2 | 31.249 | 14.632 |
| Mean of repeated prepares | 29.967 | 15.075 |
| Non-empty apply | 13.563 | 13.475 |

After reuse flags are `false, true, true`. Core DRC logs show eight full DRC
executions before and six after: three preparations originally required six
checks, now four; both versions still execute the applied-model check and the
independent diagnostic check. Cold prepare and apply remain comparable.

Full candidate validation objects, applied validation, independent DRC messages
and normalized final copper geometry match exactly between versions. Both have
zero unrouted connections and zero new findings, retaining three inherited
errors and nine warnings. Geometry normalization removes netsegment/generated
junction identities but retains nets, layers, widths, endpoint coordinates,
pad identities and all via geometry/identities. Full DRC/connectivity checks
are separate from that geometric comparison.

The whole-process observed memory high-water marks are 213284 KiB before and
202912 KiB after. These are individual observations, not evidence of a memory
optimization. Likewise, one cold and two repeated timing samples per binary
are not statistical distributions. This case measures repeated explicit
candidate preparation; it does not establish an A* or congested-router speedup.

Both binaries use the same source-independent package overrides:

```sh
cargo build --locked -p librepcb-mcp --bin librepcb-mcp \
  --config 'profile.dev.package.librepcb-core.opt-level=2' \
  --config 'profile.dev.package.clipper.opt-level=2' \
  --config 'profile.dev.package.librepcb-editor.opt-level=2' \
  --config 'profile.dev.package.librepcb-autoroute.opt-level=2'
```

The preserved pre-change binary corresponds to unchanged Rust sources at
`c331f17`; those sources were unchanged throughout the earlier layout commits.
[Context](context.json) records compiler/profile, source commits and native-tree
identity. [Comparison](comparison.json) records exact timings, binary hashes,
check equivalence and the resulting geometry hash.

For reproduction, extract the gate report's native archive, build the two
revisions with the same compiler/profile, preserve each binary, and run
[benchmark.py](benchmark.py) sequentially:

```sh
python3 benchmark.py /path/to/before-mcp before /path/to/native/selected candidate-input.json
python3 benchmark.py /path/to/after-mcp after /path/to/native/selected candidate-input.json
```

The script writes process-local project copies, exact requests/replies,
summaries and DRC logs beside itself. It never saves the edited native copy.
[candidate-input.json](candidate-input.json) refers to the selected gate-clamp
fixture's exact trace IDs. [evidence.tar.gz](evidence.tar.gz) contains raw replies,
DRC logs, test/build logs and the native-input file hash manifest.

Remaining work: atomic component side changes with connected copper, named
shared junctions for explicit copper, and moving the external review measurements
into the shared API. DRC sub-check profiling remains useful: the fresh after/apply
checks still dominate this fixture. Further acceleration must preserve the same
freshness and plane-fed connectivity protections.
