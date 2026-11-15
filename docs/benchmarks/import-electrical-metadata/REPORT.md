# Electrical import evidence and ERC scope

2026-11-15, roadmap F2. The feature starts at master `ae917ee`; implementation
commit `d96dfcf50836a33f20c4d9f9fac2078a79a41083` adds MCP API capability 56.

## Source trace

The retained real C8734 source snapshot has SHA-256
`a99b0a4abd493f08c20aa320d0332f6a14e50726b36d501e6268d1e1e68614c7`.
Every one of its 48 symbol pins has electrical type `0`. In the
[EasyEDA Standard pin format](https://docs.easyeda.com/en/DocumentFormat/2-EasyEDA-Schematic-File-Format/index.html#pin),
that means Undefined; the other categories are Input, Output, I/O and Power.
This is evidence from a pinned snapshot, not a claim about today's live source.

`easyeda/shapes.rs` correctly mapped `0` to intermediate KiCad `unspecified`.
`kicad/library_converter.rs` then mapped that to native Passive because the
native LibrePCB role enum has no Unknown variant. Before this change there was
no record distinguishing this fallback from an explicitly passive source.
The source pin schema also supplies no datasheet-required-connection flags;
imported native `required:false` must not be read as datasheet-proven optionality.

Native ERC checks required native flags, forced net names, open nets, gate
placement and visible wiring. It does not check pin-role compatibility, power
drivers or datasheet requirements. The original unwired MCU therefore produced
zero findings. Changing all pins to required based on their names or type would
invent unsupported constraints and is not the fix.

## Delivered behavior

KiCad import preserves each original pin type token, including unknown tokens,
alongside source name/number, native signal/gate/pin IDs, the imported native
role and mapping fidelity. EasyEDA replaces the intermediate tokens with the
original Standard-format codes and records the acquisition snapshot hash.

The optional component-directory file `foilfern-electrical.json` is copied by
the normal element transaction into the project and survives save/reopen and
preview snapshots. Native `.lp` syntax and signal-role tokens are unchanged.
For KiCad, tri-state outputs now map to native Output with reduced semantics
recorded. Power input/output, free, open-emitter and no-connect semantics remain
visible even where the native model cannot express them fully. No required
connection is inferred from pin type or name.

Library/component details expose a compact evidence summary and per-signal
`source_pins`. Acquisition preview/import/addition expose limitations immediately.
The summary distinguishes recorded, missing, invalid, modified and native data;
it detects current role/pin-map drift and missing signal coverage. Older known
imports without evidence are reported as missing. Existing elements and manual
edits are preserved on replay; no automatic library migration is performed.

`erc_run` names its scope `native_structural_erc`, reports the unperformed
electrical/datasheet checks, and returns `partial` with a notice for incomplete,
lossy, missing or modified import evidence, even with zero native findings.
Preview ERC reports the same before/after evidence with a notice. These notices
are separate from native findings and cannot be approved away through ERC
approvals. Valid editing remains possible; this is not a new apply gate.

Explicit passive source types are known data. Ordinary authored native passive
components are not marked as unknown imports. Source connection requirements
and native required flags are reported separately from pin role.

## Automated validation

- Import: 14 unit tests and 60 integration tests passed; one opt-in test ignored.
- MCP with HTTP enabled: 4 unit tests and 103 integration tests passed; two
  optional live-source tests and one documentation example ignored.
- Core ERC/attributes/BOM: all six focused tests passed.
- All-target Clippy for core/import/MCP with HTTP and `-D warnings`, formatting
  and whitespace checks passed. A full workspace suite was not run.

Tests cover all five EasyEDA codes, all supported KiCad pin-type categories and
an unknown vendor token; known passive versus unspecified; preserved input,
output, bidirectional and power roles; reduced/unsupported semantics; absence of
invented mandatory connections; native role edits; corrupt/missing evidence;
atomic native install and reopen; direct acquisition-to-addition; ERC `partial`
with zero findings; unchanged revision/clean state on repeated checks; and
evidence surviving save/reopen and detached previews. The import fixture's
native-file count is unchanged, with three additional component sidecars.

Native ERC message/approval generation is untouched. Upstream CLI comparisons
remain optional; no manual upstream or desktop acceptance is claimed here.

## Release acceptance method

The retained baseline release binary was built from `f0c53d4`, equivalent product
code to master `ae917ee` (the latter only adds the previous acceptance report).
Its SHA-256 is
`b79c7b3b9d5bf43d601eb51dee3ee93598f3eb8fc7085f6d78c60d5bb0ac8246`.
The updated binary is built using
`cargo build --locked --release -p foilfern-mcp --bin foilfern-mcp` from the
implementation commit above, with tracked source clean. Source identity, binary
hash, Cargo profile and measured results are retained with the replay evidence.

The bounded replay uses the CLI dispatcher shared with MCP, a fresh workspace
for each build, and the same supplier snapshot/STEP cache in offline mode. It
imports C8734 with a required model, directly adds U1 using returned acquisition
arguments, runs ERC, saves, closes/reopens, reruns ERC and reads clean state.
A separate passive control preserves authored required flags: the unwired
native R-0805 legitimately has two required-signal findings; connecting two
resistors removes them. Neither passive control is labelled unknown import data.
No power-driver or datasheet correctness claim follows from that structural pass.

No model clients or paid agent experiments are used in these replays. Time is
measured with `perf_counter` around complete CLI processes, and response bytes
are recorded; bytes are not converted into speculative model-token counts.
These are bounded functional probes, not statistical speed benchmarks.

## Release results

The updated release binary reports the implementation commit above, clean
tracked source, API 56 and Cargo release/opt-level 3. SHA-256:
`3a92db54d6e1fece0880aecf68497d261e61d7519d784b8633e57c18c4445485`.
Path: `/home/okhsunrog/code/rust/foilfern/target/release/foilfern-mcp`.
The source data is exclusively offline, with a required cached STEP asset.

The same 11-call acquisition/add/ERC/save/reopen/summary workflow completes on
both builds. Baseline ERC has zero findings and returns `complete`. Updated ERC
has the same zero native findings but returns `partial`, explicitly reports 48
unspecified source types and absent source-required-connection intent, and marks
pin-role compatibility, power-driver and datasheet checks as not performed.
Evidence remains `recorded` after reopening and the saved project is clean.

An independent verifier checks all 48 saved source records, their hash, IDs and
passive fallback roles. All four imported native `.lp` element files match the
baseline byte for byte; no guessed roles or mandatory flags were written. It
also opens a copy of the earlier saved PartUX project: ERC returns `partial`
with `missing` evidence and 48 unrecorded signals, while all native files and
clean state remain unchanged. There is no implicit migration of that project.

The nine-call passive control retains two genuine required-signal findings on
the initially unwired native R-0805. After connecting two resistors, both builds
return zero findings and `complete`; updated coverage has no import metadata
gaps. The explicit-passive KiCad conversion test separately verifies recorded
passive source types are known evidence, not unspecified fallbacks.

| Measurement | Baseline | Updated |
| --- | ---: | ---: |
| MCU workflow, 11 calls | 2.140 s | 2.611 s |
| Passive control, 9 calls | 0.138 s | 0.139 s |
| `part_import` response | 53,727 bytes | 66,707 bytes |
| `component_add` response | 16,350 bytes | 29,346 bytes |
| Initial MCU `erc_run` response | 290 bytes | 1,566 bytes |
| All MCU workflow responses | 76,411 bytes | 104,925 bytes |

Sizes use compact UTF-8 serialization of response objects, excluding JSON-RPC
framing. Additional per-pin evidence intentionally increases import/addition
payloads; the workflow requires no extra metadata-retrieval calls. The MCU replay
took 0.471 s longer in this single pair. No statistically established speed
change or token saving is claimed. These changes improve evidence and prevent
an unsupported electrical-success conclusion; they are not a payload reduction.
The report retains the measured cost rather than estimating model tokens.

The initial passive probe incorrectly expected zero findings before wiring;
inspection established that the library already marked both signals required.
The final control preserves those flags and verifies the findings disappear only
after actual connections. This was a harness expectation correction, not a
relaxation of ERC. Raw initial and final results are retained. Source identities,
native hashes and final measurements are in [measurements.json](measurements.json).

Diagnostic artifacts, source cache copies and raw JSONL results are under
`/home/okhsunrog/tmp_zfs/codex_tmp/import-electrical-metadata/`.
