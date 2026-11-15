# Acquired purchasing part acceptance

2026-11-15. F1 covers direct use of a native imported purchasing part through
primitive tools, without a declarative design document or raw mutations.

## Delivered behavior

`part_import.add_component` is an executable `component_add` argument object:
`part` selects the device and `selected_part` selects its exact native MPN,
manufacturer and attributes. Callers can add instance choices such as designator
and schematic position. Native part objects are copied intact, preserving
attribute types and units, rather than reconstructed from the response strings.
The single-device official KiCad import hint also uses the correct `part` key.

For device-only addition, one nonempty purchasing part is selected automatically.
Multiple alternatives produce candidates, a warning and a partial outcome; no
arbitrary part is selected. An explicit selector matching zero or multiple native
parts fails and rolls back the entire edit. `component_update.selected_part`
chooses a part for an existing instance with the same safeguards. Native library
edits remain authoritative on repeated import. Generic devices without purchasing
parts still work. Desktop addition defaults are unchanged.

## Correctness evidence

The MCP suite with HTTP enabled passed: 101 tests, two optional network tests
ignored. Focused part tests passed again after excluding blank MPNs. MCP and
editor all-target Clippy with HTTP and `-D warnings`, formatting and whitespace
checks passed. No full-workspace test result is claimed for this branch.

The new integration scenario verifies directly executable acquisition arguments,
sole-part auto-selection, ambiguity without guessing, exact manufacturer
disambiguation, one-step undo/redo, failed-add rollback, failed-update rollback
including accompanying value edits, board BOM and native save/reopen. It checks
all purchasing attributes, including source identity. The supplier is mocked for
repeatable failure cases.

## Bounded real client trial

One actual Codex CLI 0.169.0 run used `gpt-6.1-sol` with xhigh reasoning and only
the Foilfern MCP server. Shell, web, delegation and other integrations were
disabled. The prompt requested one C8734 instance, placement at (15,15) mm,
board BOM with `LCSC[]`, save, and usability feedback. The bound was 12 calls and
three minutes; routing, checks, rendering and unrelated imports were excluded.
The prompt did not prescribe the handoff fields.

Measured source: `8cdc14ba2d5fe0d771129a22b2a71ec201de40d3`, tracked source clean.
Binary: `/home/okhsunrog/code/rust/foilfern/target/release/foilfern-mcp`, Cargo
release profile, opt-level 3. SHA-256:
`5c097aeafa69c1b77c193631791cbb9d0a3ad04903bc9124f5e3e8a4ef453047`.
The process started after the release build finished.

The fresh workspace contained a previously captured real supplier source and
STEP cache, but no installed native device or project. `part_import` ran with
`cache:"offline", require_model:true`; this tests conversion and handoff without
network variability, not current supplier availability.

The completed workflow used seven successful calls:

1. `server_info`
2. `project_create`
3. `part_import`
4. `component_add`
5. `device_place`
6. `export_bom`
7. `project_save`

There were no failed tools, retries, design documents or raw mutations. An
independent release CLI process reopened the native project and verified:

- U1 uses the acquired device and exactly one selected native purchasing part.
- MPN `STM32F103C8T6`, manufacturer `ST(意法半导体)` and LCSC `C8734`
  agree between import, selected part and CSV BOM; package is populated.
- All native purchasing attributes agree, including `SOURCE_SHA256`:
  `a99b0a4abd493f08c20aa320d0332f6a14e50726b36d501e6268d1e1e68614c7`.
- U1 is at (15,15) mm, the saved project has no unsaved changes, and its native
  library contains a STEP file with the ISO-10303-21 header.

| Measurement | Value |
| --- | ---: |
| MCP calls | 7 |
| Approximate elapsed time | 50.28 s |
| Input tokens, including cached | 264,737 |
| Cached input tokens | 219,776 |
| Fresh input tokens | 44,961 |
| Output tokens | 1,529 |
| Reasoning output tokens, included in output | 567 |

Token counts come directly from the client's completed-turn usage event.
An initial invalid telemetry setting was rejected before any model turn; its
configuration was corrected before this run. After the successful run, a
multiline OTLP JSON parsing error interrupted metrics postprocessing. Counters
were recovered from the original completed event without another paid run.
The exact process timer was not saved: elapsed time above was reconstructed from
invocation-file and final-event-file timestamps and is approximate. These data
establish bounded completion, not a speed or token improvement against master;
no A/B comparison was performed.

## Usability result and limits

The client reported that the ready import arguments made direct addition
convenient and required no workaround. Its remaining confusion was that
`component_add` places a schematic symbol while physical placement needs
`device_place`; it suggested optional combined import and board placement.
That suggestion is recorded, not implemented in F1. The tool workflow documents
the separate placement step. Full feedback is retained in [feedback.md](feedback.md).

The source-projection provenance warning remained visible. This trial does not
validate electrical metadata, datasheet pin correctness, model alignment or
complete mechanical/electrical acceptance; these are separate roadmap gates.

Raw local run, metrics, verifier results, source cache and test logs are under
`/home/okhsunrog/tmp_zfs/codex_tmp/part-acquisition/`. They are diagnostic artifacts,
not required runtime dependencies or portable fixtures.
