# Repeated checks and diagnostic subjects

2026-11-15. Roadmap F5/F6. The branch starts at master `6eaeb5d`, after the
fast-forward merge of purchasing-part handoff. No earlier MCP redesign branches
are involved.

## Cause and changes

Full DRC deliberately forces an air-wire rebuild. Previously that rebuild always
journaled every scheduled net, even when calculated data was identical. Repeated
checks therefore advanced the revision and invalidated subsequent guarded writes.
The builder now compares the per-net vectors of native `AirWire` values, including
anchor identities and exact integer endpoint positions. It clears scheduled dirty
nets in either case, but replaces data and journals only changed results. It does
not bypass DRC, suppress real edits or relax snapshot/revision checks. Necessary
initial derived updates and obsolete approval cleanup can still change revision.

The empty pin name came from the library pin display mode: the label is hidden
on the schematic, so `SymbolPinView::name()` legitimately returns an empty string.
Canonical upstream ERC messages remain intact. A separate readable diagnostic
title falls back to the component signal name. MCP uses that title and retains
`canonical_message` when different. ERC findings also carry typed `objects` with
scoped component, symbol, pin, signal, net and corresponding board-pad identities
and available names. Design-preview ERC reports use the same formatter. This
does not add electrical semantics or change approval identities.

Text-only device edits now publish `DeviceTextsChanged {component,texts}`. The
complete device value is compared with texts replaced; all other properties,
including future added properties, participate in deciding whether this is a
text-only update. Mixed updates retain `DeviceChanged`. Board scenes refresh the
device without marking it as moved. `board_text_move` returns a scoped `edit`
with the designator, rendered text and before/after position, rotation and bbox.
An unchanged pose produces no event or undo step. Bounded declarative legend
placement returns the same review information in the report and semantic diff.
MCP API capability version is 55; no tools were added or hidden.

## Acceptance

Automated regression scenarios cover:

- Rebuilding identical wires repeatedly without revision changes; moving an
  endpoint with unchanged anchor identities still publishes its changed net.
- Repeated full DRC before and after native save/reopen, clean-state preservation,
  a guarded write using the revision from before repeated checks, and rejection
  of that token after an actual edit.
- Hidden R1 pin labels: readable signal name, stable subject IDs, corresponding
  physical pads, unchanged canonical title, and preserved approval after native
  save/reopen.
- Scoped text before/after reports, shared template UUIDs on different instances,
  unchanged device properties and copper, undo/redo, and an unchanged-pose call.
- Declarative legend review over stdio and incremental scene equivalence with a
  fresh scene after footprint text movement, undo and redo.

The existing background-derived-update test previously depended on the redundant
air-wire event to advance revision. It now checks the unchanged rebuild first,
then calculates genuinely changed plane fragments and verifies that a prepared
DRC still accepts those background derived updates. The separate test refusing
intervening model edits is retained.

Validation passed: MCP (4 unit tests, 103 integration tests; two optional network
tests ignored), scene (10 unit tests, all 15 integration tests, including random
edit sequences), the four air-wire core regression cases and the updated
background-derived-data test. All-target Clippy for core, editor, scene and MCP
with HTTP enabled passed with `-D warnings`; formatting and whitespace checks
passed. One documentation example in each of MCP and scene is intentionally
ignored.

An additional broad core attempt passed its 120 unit tests and reached the
integration stress case that runs ten full Gerber Test DRCs in debug. That
additional run was stopped after the stress case had consumed over five minutes;
the outdated revision assertion noted above was corrected and its focused test
passed. No complete core/workspace suite or manual desktop/oracle run is claimed.

## Release replay method

The fixed request sequence opens a fresh copy of upstream `Nested Planes`, runs
full DRC, records revision, runs DRC twice more, reads the summary, then renames
N1 using the recorded revision and closes with discarded edits. Both binaries
use the same source fixture and requests. The fixture has existing findings
(35 errors, 45 warnings); this tests workflow consistency, not board correctness.
The fixture checkout is LibrePCB `99ff973ac20f3753bdb30d4e29608aadc882674f`.

Baseline is the retained release binary built from `8cdc14b` with clean tracked
source, equivalent product code to merged master `6eaeb5d` (the latter only adds
acceptance documentation). SHA-256:
`5c097aeafa69c1b77c193631791cbb9d0a3ad04903bc9124f5e3e8a4ef453047`.
Path: `/home/okhsunrog/tmp_zfs/codex_tmp/check-diagnostics/baseline-mcp`.

The baseline reproduces revisions 3 → 4 → 5 for identical full checks, followed by
`stale_revision` on the guarded rename. Each DRC returns a 65,033-byte response
when serialized using the replay's JSON settings.

Updated binary: `/home/okhsunrog/code/rust/foilfern/target/release/foilfern-mcp`,
source `f0c53d470c3cf0d32ce4f2592c1e193156fb4aa5`, tracked state clean, Cargo
release/opt-level 3. SHA-256:
`b79c7b3b9d5bf43d601eb51dee3ee93598f3eb8fc7085f6d78c60d5bb0ac8246`.
Build command: `cargo build --locked --release -p foilfern-mcp --bin foilfern-mcp`.
The upstream directory environment points at the fixture checkout above.

The updated replay gives 3 → 3 → 3 and completes the guarded rename, advancing
revision to 4. All three DRC result objects are identical between builds,
including findings, locations and empty check-failure lists. On this legacy
fixture the initial full check marks the project unsaved in both builds; further
identical checks preserve that state. Clean-state preservation with current
approvals is verified separately by the warm-check/save/reopen regression test.

A second four-call replay opens a saved native R-0805 fixture, runs ERC, moves its
reference text and closes with discarded edits. The setup uses `connect` with
`assign_only:true`, creating an electrically assigned pin with no visible wire.
Both builds receive copies of the same saved project and the same text target.
The old build returns `Connected pin without wire: 'R1:'` and `DeviceChanged`.
The updated build returns `R1:1`, the unchanged canonical title and subject IDs,
the component/signal/net/pad names and IDs, and the scoped `DeviceTextsChanged`
event plus matching before/after text data. A separate verifier checks these
fields against the original fixture and exact requested pose, not only tool
success flags. See [measurements.json](measurements.json).

After the initial reproduction, one final baseline/updated pair was run
consecutively for each scenario:

| Measurement | Baseline | Updated |
| --- | ---: | ---: |
| Full DRC/guarded-write workflow, 9 calls | 1.283 s | 1.755 s |
| ERC/text workflow, 4 calls | 0.065 s | 0.034 s |
| One DRC response | 65,033 bytes | 65,033 bytes |
| ERC response for the resistor fixture | 2,179 bytes | 3,338 bytes |
| Text-move response | 957 bytes | 1,623 bytes |

The DRC workflow's updated timing includes executing the edit; the baseline
refuses it. These are not equivalent successful-throughput measurements. Initial
probes also showed substantial timing variation under background build load;
the final pair is retained without selecting the fastest run. There is no
supported speedup or slowdown conclusion from these single pairs. The confirmed
gain is that identical checks no longer invalidate a valid guarded write.
The diagnostic responses add 1,825 bytes across ERC and text movement in this
fixture, intentionally including the names and before/after state in the original
calls instead of requiring separate model queries.

Timing uses Python `perf_counter` around each complete CLI process, including
startup/open/close. It is one final functional pair per scenario on a host with
other builds running, not a statistical speed benchmark. No model clients are invoked:
there is no model-token measurement or claim of token savings. F5 avoids a forced
read/retry after identical checks. F6 deliberately adds subject and edit detail
so the caller can understand the operation without an extra model lookup; its
net token impact on autonomous work is not measured here.

Raw logs, binary, fixture copies and JSONL responses are retained locally under
`/home/okhsunrog/tmp_zfs/codex_tmp/check-diagnostics/`.
