# Board comparison and STEP assembly evidence

2026-11-15. F3 and F4 are delivered on master. A board-only preview now
returns its own before/after images without selecting or changing a schematic.
STEP jobs distinguish model coverage from mechanical verification and identify
missing models in the successful export response.

## Provenance and bounded replay

- Baseline binary source: `d96dfcf50836a33f20c4d9f9fac2078a79a41083`, clean,
  API 56, release, opt-level 3. This is product-code-equivalent to pre-task master
  `3d6eb84059ae82688e0a698975342297688b1024`; that later commit changes docs only.
  Binary: `/home/okhsunrog/tmp_zfs/codex_tmp/board-preview-step/baseline-mcp`.
  SHA-256: `3a92db54d6e1fece0880aecf68497d261e61d7519d784b8633e57c18c4445485`.
- Updated binary source: `d431f4be6d934ca297c041780c08ad26b7f2b33f`, clean,
  API 58, release, opt-level 3. Binary:
  `/home/okhsunrog/code/rust/foilfern/target/release/foilfern-mcp`.
  SHA-256: `b3935f79b2d3648159d72cc7eed838e3ead2cc604b7b3a164d3f6e468b981fd9`.
- Both use copies of one saved native project containing the upstream Populated
  Library R-0805 and C-0805. The resistor has a selected model; the capacitor
  does not. Upstream fixture checkout:
  `99ff973ac20f3753bdb30d4e29608aadc882674f`.
- The requests use the shared registry through local CLI batch calls. No model
  or delegated-agent runs were made. The equivalent transport path is covered
  by the stdio integration tests. Project setup is outside measured operations.
- Raw calls, responses, PNGs, original STEP files, logs and replay script remain
  under `/home/okhsunrog/tmp_zfs/codex_tmp/board-preview-step/`.
  `baseline-final/` and `updated/` are the accepted comparison runs.
  The earlier `baseline/` is exploratory and is not used in the table below.

Build command (using the configured Cargo output/cache paths):

```sh
cargo build --locked --release -p foilfern-mcp --bin foilfern-mcp
```

## F3 result

Both releases receive the same board-only comparison request. It moves C1 from
(20,10) to (25,15) mm. The top candidate stays on top; the bottom candidate also
flips C1 to the bottom and asserts its original top side. Both use the crop
(0,0)–(40,30) mm. Each preview is a fresh process over an unchanged saved project.

Baseline returns `invalid_argument`, requiring a schematic, with no image.
The updated release returns two 1600×1200 PNGs in after/before order. Metadata
identifies board, side, state, indices 0/1 and the same region for both images.
Visual inspection confirms that stationary R1 stays fixed in each pair, C1 moves,
and the bottom pair shares the same mirrored coordinate frame. The schematic
is never selected. Saved native project files remain byte-identical after each
preview and after export; their hashes are retained in `measurements.json`.

| View | After (attachment 0) | Before (attachment 1) |
| --- | --- | --- |
| Top | [PNG](top-0.png) | [PNG](top-1.png) |
| Bottom | [PNG](bottom-0.png) | [PNG](bottom-1.png) |

The default without comparison still returns one after-image. Combined
schematic/board documents offset board attachment indices after schematic images.
Integration tests also cover legend-only changes, unchanged schematic/live board,
shared crop metadata, ordinary undo/replay and refusal after revision changes.

## F4 result

One `jobs_run` request exports the original mixed assembly on both releases.
Baseline says `complete` with no warnings despite omitting C1. The updated
release writes the STEP file and returns `partial`, with one represented model
(R1), one fitted missing model (C1), named component/package/model references,
`alignment:unverified` and `mechanical_validation:not_performed`. No second call
is needed to find the omitted component or verification limit.

Both STEP files are 66,174 bytes. Their DATA sections are byte-identical; the
only file difference is the FILE_NAME creation timestamp. New warnings and
coverage do not change exported geometry. Integration coverage reads the mixed
assembly back through the CAD backend and verifies PCB plus component solids;
coverage survives native save/reopen. PCB-only exports explicitly identify their
scope, rather than claiming complete assembly coverage. Variant exclusions are
checked against physical-geometry extraction over upstream project fixtures.

The runner now visits every requested assembly variant, matching upstream's
nested board/variant loop. Coverage events are emitted only after a successful
file write. Tests preserve previous output on preparation failure, reject empty
solid models, and retain the component name for unavailable model resources.
Model alignment, clashes and real mechanical fit have not been verified.

## Workflow, time and response cost

Elapsed times are single observations, include process startup and, for previews,
`server_info`. These are bounded correctness replays, not a statistical speed
benchmark. The failed baseline previews do less work, so their time is not a
render-performance comparison. No speedup claim is made for STEP either.

| Operation | Baseline seconds | Updated seconds | Baseline / updated calls | Baseline / updated response bytes |
| --- | ---: | ---: | --- | ---: |
| Top comparison | 0.084 (failed) | 0.108 | 2 / 2, including server_info | 419 / 53,882 |
| Bottom comparison | 0.038 (failed) | 0.081 | 2 / 2, including server_info | 419 / 53,899 |
| Mixed STEP export | 0.113 | 0.044 | 1 / 1 | 462 / 1,857 |

Response sizes are compact UTF-8 JSON for the measured tool response, including
text and base64 images, excluding local batch framing and the server_info response.
The updated preview structured envelopes alone are 3,108 and 3,129 bytes;
most of the response is the two images. STEP adds 1,395 bytes for immediately
usable coverage and warnings. Coverage grows with the board's device count.
No catalog changes or deferred data-fetch calls were introduced. Token use was
not measured: JSON/base64 byte counts are not model token or image-token counts.

## Automated validation

- MCP integration: 103 passed, two live-source tests ignored.
- Core: two STEP job tests (including both requested variants and successful-write
  events) and the read-only physical snapshot/model-selection corpus test passed.
- STEP: 17 unit tests and two integration tests passed.
- Clippy with warnings denied: core, MCP (HTTP enabled), scene, STEP and CLI,
  all targets; final STEP checks repeated after its additional negative fixture.
- Formatting and diff checks passed. No full workspace suite, desktop interaction
  or physical mechanical acceptance is claimed.

Machine-readable observations and identities: [measurements.json](measurements.json).
