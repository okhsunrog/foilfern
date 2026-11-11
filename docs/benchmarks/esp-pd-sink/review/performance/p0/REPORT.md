# Routing stage timings on ESP-PD-Sink

Measured on 2026-11-12 with a clean release build of
`98b2c21549dc2111bb30b4916ab17ada04cc9362`, API capability version 2.
The executable was `target/release/librepcb-mcp` at the workspace root.
Its SHA-256 is `e4d1a07d0bb23a034049167477c5dc532472f8d1ab848736471460f1baf8316e`.
The host was an Intel Core Ultra 5 125H (18 logical CPUs), Linux 7.2.9 x86-64,
with Rust 1.99.1 and Cargo 1.99.1. The MCP crate reported Cargo's release
profile and optimization level 3.

## Results

Each cell is the median and observed range of five samples. The cold rows
use the dedicated cold sessions. The initial warmup calls from warm sessions
are retained in the evidence.

| Case and cache | Prepare, s | Preview, ms | Apply |
| --- | ---: | ---: | ---: |
| OLED cold | 32.202 [31.958–37.357] | 1.444 [1.193–13.743] | 17.288 s [16.061–18.106] |
| OLED warm | 16.731 [16.528–22.312] | 1.375 [1.129–1.764] | 16.073 s [15.930–24.283] |
| Builtin cold | 32.713 [31.779–35.818] | 2.194 [1.827–2.714] | 1.040 ms [0.643–2.210], refused |
| Builtin warm | 16.936 [16.623–17.571] | 1.933 [1.566–2.184] | 1.031 ms [0.880–1.330], refused |

Median warm preparation took about 48% less time in both cases. The OLED
apply ranges overlap. Both successful apply conditions ran fresh full checks.

| Case and cache | Snapshot, ms | Source checks, s | Candidate work, ms | Extraction, ms | Search, ms | Candidate checks, s | Apply checks, s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OLED cold | 83 | 15.819 | 3 | — | — | 16.573 | 17.286 |
| OLED warm | 79 | 0.474 | 3 | — | — | 16.119 | 16.071 |
| Builtin cold | 76 | 15.675 | 524 | 7 | 35 | 16.120 | — |
| Builtin warm | 77 | 0.471 | 507 | 15 | 35 | 15.864 | — |

These are stage medians. Extraction and search are included in candidate work.
Source and candidate checks together account for 96–100% of prepare wall time
across these samples. The builtin search median was 35 ms in both conditions
on this bounded two-net case.

The OLED candidate preserved the netlist, kept zero open connections,
and added one undo step in each of the ten apply attempts. Its checked result
had 996 traces, 143 vias, three inherited DRC errors and nine inherited
warnings. Prepare, preview and apply validation agreed.

In the competing builtin case, the router connected both target nets. Rebuilding
planes and connectivity exposed one new GND air wire. Full DRC reported four
errors, including that open connection, with the original three copper errors
and nine warnings. All ten attempts returned `refused` and preserved live
geometry, revision and history. The source baseline was reused in every warm
sample. Logs record 60 full DRC executions: 20 source checks, 30 candidate
checks and 10 applied-model checks. All twenty on-disk project copies matched
their input manifests after closing without saving.

[Evidence](evidence.tar.gz) contains the source and request hashes, per-sample
results, logs and structured routing responses, including complete validation
and the previewed mutations. Board reads are represented by hashes of their
canonical JSON plus trace and via counts. Duplicate text renderings are omitted
from the archived transcript. The replay writes the original transcripts to
the local output directory.

## Protocol

Use the original [handoff fixture](../../../inputs.tar.gz) and the saved
[OLED copper edit](../../candidates/oled.json) and
[competing builtin requests](../release/builtin-input.json). The earlier
[release comparison](../release/REPORT.md) used another host and different
commits. This run compares cache-cold and cache-warm conditions within one
build.

Each of the two cases has five fresh stdio sessions per condition. A cold
session prepares, previews and attempts to apply its first candidate. A warm
session first prepares, previews and discards a candidate, then repeats the
same request on the same source revision and attempts to apply the second
candidate. Each session starts from an independent copy of the same 482
native files. Condition order alternates by sample. The client reads board
and history before prepare and checks them again before apply. Cold refers
to the source-check cache within MCP. OS filesystem caching is uncontrolled.

The client times each request through JSON response parsing. Debug logs
measure editor-side stages:

| Stage | Timed work |
| --- | --- |
| Snapshot | Native serialization and reload into an isolated editor |
| Source checks | Source validation including derived-data rebuild, or derived-data rebuild with retained source evidence |
| Candidate work | Detached editor commands including explicit copper work or regional rip-up and routing |
| Extraction and search | Router problem extraction and the router call, both included in candidate work |
| Candidate checks | Fresh full model check on the detached candidate |
| Apply checks | Fresh full model check after a non-empty commit |

The OLED request edits explicit copper and placement, so it has no router
extraction or search stage. The competing builtin candidate is refused
before commit and has no applied-model check. Preview timing covers the
complete patch page returned for these two requests.

## Reproduction

Extract the `foilfern/` directory from the original archive. Use a fresh
output directory on disk, with the upstream source and resources configured
as in the [build instructions](../../../../../../README.md#build-from-source).

```sh
cargo build --locked --release -p librepcb-mcp --bin librepcb-mcp
uv run --no-project replay.py /absolute/path/to/librepcb-mcp /path/to/extracted/foilfern /path/to/new/output
```

Run the script serially without competing builds. It records the binary hash,
exact request hashes and native source manifest. Each session retains its
JSON-RPC transcript with wall and stage times and the server log. The summary
contains the per-sample times and full validation for each applied or refused
candidate. The replay verifies full validation on both sides of preparation,
fresh checks on successful apply, unchanged history before apply and unchanged
native files after closing each copy without saving.
