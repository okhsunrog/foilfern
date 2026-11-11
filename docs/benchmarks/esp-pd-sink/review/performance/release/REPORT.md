# Release validation of routing baseline reuse

The baseline-reuse implementation in `eab6191`, merged with current master at
`e321b4d`, reduces repeated OLED preparation from **25.598 to 13.915 seconds**
(mean of two repeated samples): **45.6% less time, 1.84× speedup**. Release alone
does not remove the repeated source-DRC cost. This supplements the earlier
[optimized-dev experiment](../REPORT.md) with the original handoff fixture and
release builds; it does not change the PCB design.

## Identical input and builds

Before: master `8cb635e4b30bfdfc8ea95d997e453aba88ef5b40`.
After: `e321b4d2d8a2a0f55a53aba2f109e6ae97ef584b`, containing the baseline cache.
Both use Rust 1.99.1 and the unmodified default release profile (`opt-level=3`),
with identical Cargo manifests/lockfiles and no package optimization overrides:

```sh
cargo build --locked --release -p librepcb-mcp --bin librepcb-mcp
```

The before build used a detached worktree and the shared target directory. Its
binary was preserved at `/workspace/esp-pd-sink/release-review/before/librepcb-mcp`.
The patched binary is `/workspace/foilfern/target/release/librepcb-mcp`, also
preserved under `release-review/after/librepcb-mcp`. Both MCP processes were
started after their builds. All runs were serial, with no concurrent builds.
[Context](context.json) records compiler, machine, commits, profile and input
hashes; [comparison](oled-comparison.json) records binary hashes and measurements.

All 482 source-project files match `inputs.tar.gz` byte for byte. Each process
opens its own copy of that original project and uses the accepted
[OLED request](../../candidates/oled.json). Each executes three identical
`prepare → preview → discard` cycles on one revision. History remains identical.
A fourth preparation, outside the repeated timing samples, verifies non-empty
revision-guarded apply followed by an independent full DRC. Neither run saves
native files; complete before/after native manifests remain identical.

## Timing and checks

| Wall time, seconds | Master before | Cache after |
| --- | ---: | ---: |
| First prepare | 25.356 | 25.919 |
| Repeated prepare 1 | 25.437 | 13.224 |
| Repeated prepare 2 | 25.759 | 14.606 |
| Mean of repeated prepares | 25.598 | 13.915 |
| Separate verification prepare | 24.335 | 14.256 |
| Non-empty checked apply | 13.274 | 16.309 |

On the first master preparation the source core DRC occupies 12.298 seconds
(48.5% of preparation) and candidate DRC 12.093 seconds (47.7%). Later source
checks still cost 12.258 and 11.816 seconds. Patched repeat calls contain only
the fresh candidate DRC, taking 12.339 and 13.764 seconds. Plane rebuilding,
air-wire reconstruction and snapshot work remain. Core DRC log timings exclude
some surrounding extraction/rebuild work; the complete MCP wall times include it.

Reuse flags are `false, true, true`; discarding a candidate preserves only its
still-current baseline. The extra verification preparation also reuses it.
Core logs show **10 full DRC executions before and 7 after** across the four
preparations, apply and independent diagnostic. Each run still performs a fresh
candidate check, a fresh applied-model check and the independent DRC.

Every complete candidate-validation object, applied validation, final DRC
message list and canonical geometry digest matches across versions. Both finish
with 996 traces, 143 vias, zero unrouted connections, zero new DRC errors, and
the same inherited three errors and nine warnings. The netlist is preserved.
Canonical geometry keeps nets, layers, widths, endpoint kinds/coordinates and
pad addresses/identities, all via dimensions, device placement and plane data;
only generated copper-object identities are omitted. This is independent of
the full DRC/connectivity comparison.

These are one cold and two repeated samples per binary, not distributions.
Apply was slower in the patched observation, 16.309 versus 13.274 seconds;
no apply acceleration is claimed. Process high-water memory was 167920 versus
177096 KiB; no memory improvement is claimed. The earlier focused editor/MCP
correctness tests cover stale publication, save/path context, board/revision/
session isolation and detection of a lost plane-fed connection after cache warmup.
Their commands/results are documented in the implementation report above.

## Reproduction

Extract `foilfern/` from the handoff `inputs.tar.gz`, build/preserve both binaries,
and run [benchmark.py](benchmark.py) serially with an output directory outside
the repository. Each label must have a fresh project-copy directory:

```sh
python3 benchmark.py /path/to/before-mcp oled-before /tmp/input/foilfern /path/to/review/candidates/oled.json /tmp/release-review
python3 benchmark.py /path/to/after-mcp oled-after /tmp/input/foilfern /path/to/review/candidates/oled.json /tmp/release-review
```

[oled-evidence.tar.gz](oled-evidence.tar.gz) contains exact JSON-RPC requests and
responses, per-call wall time and DRC-log byte ranges, complete logs, native-file
manifests and both release build logs. Generated project copies and binaries are
not committed. The subsequent [bounded builtin experiment](BUILTIN.md) exercises
two competing nets and real rip-up; this OLED case measures explicit copper.
