# Contextual pin previews and output-path review

2026-11-15. F8 and F9 are delivered on master. Acquired-part previews show
actual mapped pin/pad numbers, board overlay captions identify the viewing
orientation, and configurable STEP/graphics jobs reveal their exact filenames
and page identities before writing.

## Provenance and resource limit

- Baseline release: clean source `48f73299ca7a16a998579db14dd732cdfa58819b`,
  API 59, opt-level 3, product-code-equivalent to pre-task master
  `c4385d88c339595d4026dcdb31aef5bbb4577d52` (later evidence/docs only).
  Binary `/home/okhsunrog/tmp_zfs/codex_tmp/render-output-plan/baseline-mcp`.
  SHA-256 `5920cc898c9a95a6ebfd858e6cf70747754f47b2c9c55bd7a10169595fcc3d6c`.
- Updated release: clean source `0b5bb62a8615ea9dd3bf30d8835da76c1eafc479`,
  API 61, opt-level 3. F8 code was delivered as `647392d`; F9 as `0b5bb62`.
  Binary `/home/okhsunrog/code/rust/foilfern/target/release/foilfern-mcp`.
  SHA-256 `41c4088d48cbf1d3ded1756e7159892460ec1203171cd9d9237725e62c451694`.
- All builds, tests and control runs were restricted to logical CPUs 0–1,
  nice 10; Cargo jobs, test threads, Rayon threads and Tokio worker threads
  were limited to two. Cargo/test/rustc processes were inspected; runtime
  harnesses recorded affinity `[0,1]` and nice 10. Heavy commands ran sequentially.
  Existing Cargo target/cache paths were retained. The release build took 5m21s;
  no build-speed comparison is claimed.
- Upstream fixture checkout: `99ff973ac20f3753bdb30d4e29608aadc882674f`.
  Work artifacts, scripts and logs remain under
  `/home/okhsunrog/tmp_zfs/codex_tmp/render-output-plan/`.

## F8: what was wrong and what is now visible

The symbol numbers were not truncated. The generic symbol painter literally
renders `1…`, because a reusable bare symbol has no package pin assignment.
Acquired-part previews already have the component, device and package, so they
now resolve each symbol pin through its component signal and device pad mapping
to the actual package pad name. Missing mapping stays blank and is reported as
incomplete rather than receiving invented numbers. Generic library editing and
export keep their context-free upstream semantics.

Footprint previews now draw pad names as preview annotations, with one font scale
bounded by neighboring pad spacing. An initial diagnostic render exposed overlap
between adjacent two-digit labels; the spacing-aware version separates them while
retaining the full names. A test also covers very long labels without ellipsis.
The native library geometry and text definitions are not changed.

The release replay uses offline C8734 (STM32F103C8T6), source SHA-256
`a99b0a4abd493f08c20aa320d0332f6a14e50726b36d501e6268d1e1e68614c7`.
Both releases use that same snapshot and cached STEP. The new number map contains
all 48 numbers exactly once. The release images were inspected at their native
1000×800 resolution, including two-digit labels and the footprint's pin-1 marker.
This validates presentation of the declared source mapping, not datasheet or
mechanical alignment correctness.

| Preview | Baseline | Updated |
| --- | --- | --- |
| Symbol | [PNG](baseline-symbol.png) | [PNG](updated-symbol.png) |
| Footprint | [PNG](baseline-footprint.png) | [PNG](updated-footprint.png) |

`render(side:"both")` still returns one image of the configured visible-layer
stack, viewed from top. It now says **visible-layer overlay viewed from top**,
rather than **top side**. Metadata identifies the overlay, selected board,
viewing side, mirroring, dimensions and layer-selection policy. Bottom views
explicitly identify their mirrored coordinate frame. Neither top/bottom nor
`both` silently changes saved layer visibility or becomes an isolated physical
surface renderer. For all three requests, PNG bytes match the baseline.

## F9: review predicts the actual native outputs

A copied upstream "Project With Two Boards" fixture configures a single-board
`board_rendering_png` job with the template `board.png`. Both releases run the
same four-call flow: server_info, jobs_configure, jobs_run, project_save.
Baseline configuration reports only the template; concrete names appear after
execution. Updated configuration immediately reports `board1.png` and
`board2.png`, their absolute native output paths and top/bottom page identities.
The subsequent export writes exactly that list, and reports a matching plan.
Both output PNG files are byte-identical between releases.

Native page selection/substitution is shared in core. Graphics generation and
review use the same filename-numbering function. The template is preserved;
files are not renamed to conceal native multi-page behavior. Tests cover merged
PDFs (one file with several identified pages), STEP board/variant contexts,
native placeholders, extension case, no-page cases, native save/reopen and
existing ownership/collision behavior.

`jobs_run.dry_run:true` can review a selected output directory without rendering,
rebuilding planes, creating the output directory/index or changing the project.
A release dry-run over an override directory took 0.011 s and left both that
directory absent and all saved project files unchanged. Integration checks also
compare revision, history and board state. An export with a stale
`expected_revision` is refused before writing. Selected-job path collisions are
shown explicitly without creating files.

Exact review supports STEP and graphics: all current `jobs_configure` presets.
Other native job types still execute normally, but review explicitly marks their
planning incomplete. A complete filename plan is not a model-load, rendering,
permission or filesystem-ownership guarantee. The native writer retains those
checks at execution. Plans use the current open project's attribute context;
DATE/TIME-dependent names should be reviewed again if that context changes.

## Cost and workflow observations

These are single observations, with both binaries constrained to the same CPUs
and priority. Times include process startup; preview includes server_info and
offline import/PNG generation. Workspace setup/rescan is excluded. No statistical
latency or speedup claim follows from one pair.

| Flow | Baseline seconds | Updated seconds | Calls | Baseline / updated response bytes |
| --- | ---: | ---: | --- | ---: |
| Offline part preview | 1.169 | 1.476 | 2 / 2 | 167,040 / 224,754 |
| Configure, export and save | 0.505 | 0.286 | 4 / 4 | 2,306 / 4,341 |
| `side:both` render response | — | — | 1 / 1 | 3,541 / 3,819 |

The richer preview adds 57,714 response bytes, mostly complete image labels.
The export flow adds 2,035 bytes; the configuration response alone grows from
379 to 1,246 bytes and contains the useful pre-write path/page list. Ordinary
configuration/export needs no additional data-fetch call. Explicit dry-run is
optional when reviewing a directory override or seeking a separate review step.
Bytes are compact UTF-8 JSON tool responses, including text and base64 PNGs,
excluding local batch framing. Preview byte counts exclude server_info; export
counts include all four responses. These are not model tokenizer/image-token
measurements. No delegated/model usability runs were made.

## Validation boundary

- MCP with HTTP enabled: 108 integration tests passed, two live-source tests
  ignored. F8/F9-focused workflows ran over stdio and the shared local dispatcher.
- Graphics: 22 tests passed. Core output runner: four focused tests passed.
  Scene export: five tests passed. The collision/stale/dry-run case was repeated
  after extending its assertions.
- Clippy with warnings denied for core, graphics, scene, MCP and CLI, all targets
  (MCP HTTP enabled), formatting and diff checks passed.
- The verification script compares the 48-number map, source snapshot, exact
  pre-write/actual paths, output PNG hashes, board-view hashes and read-only
  dry-run filesystem state. This is not a full workspace, live desktop,
  datasheet or physical manufacturing acceptance run.

[Machine-readable measurements](measurements.json).
