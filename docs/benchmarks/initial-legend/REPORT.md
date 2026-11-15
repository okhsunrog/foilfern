# Initial device legend placement

2026-11-15. F7 is delivered on master, API 59. New device placement can correct
blocked legend text in the placement's existing undo group, without changing
device pose, text style, library definitions or copper. Unsuccessful searches
keep their original positions and explicit findings.

## Cause and implementation

The upstream Populated Library's C-0805 name anchor is (-1.5875, 0.3175) mm,
with 1 mm height and 0.2 mm stroke width. The rendered C1 overlaps a mask opening.
Baseline release reproduced one native `SilkscreenClearanceViolation` on each
of eight isolated boards: top/bottom at 0/90/180/270 degrees. The reported anchors
match the native footprint's rotated/mirrored defaults. This is a library default
problem; the placement transform is correct and is not changed.

The editor service checks the actual resolved stroke paths against board and
cutouts, configured mask clearance, other legend text and known courtyard/outline
regions. It preserves a clear original. Otherwise it tries 32 nearby translations
for each of at most 16 configured legend texts per new device. It retains font,
size, rotation, mirroring and layer. Existing/manual annotations are not targets.
Missing body geometry prevents an unqualified clear result or automatic move
under uncertain courtyard coverage. It does not invalidate an otherwise mask-clear
native placement merely because its optional courtyard coverage is incomplete.

The same service runs for initial `device_place`, `device_auto_place`, new
declarative board placements, and the editor FSM's final drop after moving,
rotating or flipping the cursor preview. Explicit `legend_placements` own their
targeted device's annotations. A new declarative device's requested final lock is
applied after text placement; existing locked text is never unlocked.

`initial_legend` gives scoped text edits, before/after bbox and checks, attempt
counts, missing geometry and verification status. Its scope is `legend_geometry`,
not whole-board DRC. A real unresolved conflict or failed mandatory check produces
`partial`; missing optional courtyard coverage alone is reported without failing
placement. Native DRC rules and approvals are not weakened or suppressed.

## Release evidence and workflow cost

- Baseline: clean source `d431f4be6d934ca297c041780c08ad26b7f2b33f`, API 58,
  release, opt-level 3. Product code equals pre-task master
  `f0f24773a7844b12ff88e6b3d9ec3bb1b4e3e436` (later report/docs only).
  Binary `/home/okhsunrog/tmp_zfs/codex_tmp/initial-legend/baseline-mcp`, SHA-256
  `b3935f79b2d3648159d72cc7eed838e3ead2cc604b7b3a164d3f6e468b981fd9`.
- Updated: clean source `48f73299ca7a16a998579db14dd732cdfa58819b`, API 59,
  release, opt-level 3. Binary
  `/home/okhsunrog/code/rust/foilfern/target/release/foilfern-mcp`, SHA-256
  `5920cc898c9a95a6ebfd858e6cf70747754f47b2c9c55bd7a10169595fcc3d6c`.
- Fixture: upstream checkout `99ff973ac20f3753bdb30d4e29608aadc882674f`,
  unchanged Populated Library C-0805. Each variant creates a fresh project,
  adds C1, places it at (20,20) mm, reads legend, runs full DRC, saves and closes.
  Both binaries execute the same requests apart from their artifact directories.
- Main flow: 57 calls per binary, including server_info and eight seven-call
  project flows. Workspace creation/rescan is outside the measured interval.
  Direct local CLI calls use the shared registry; F7 integration workflows run
  over stdio, and the full MCP suite also enables the HTTP transport. No model or delegated-agent experiments were run.

| Observation | Baseline | Updated |
| --- | ---: | ---: |
| Foilfern mask-clearance warnings, total over eight projects | 8 | 0 |
| Full DRC errors | 0 | 0 |
| Main flow calls | 57 | 57 |
| Main flow seconds, including process startup | 0.777 | 1.479 |
| All response bytes | 85,700 | 73,966 |
| Placement response bytes, summed over eight calls | 12,904 | 22,774 |
| Candidate translations per placement | none | 9–12 |

All eight updated cases finish without a repair call. Baseline stops with the
known warning and would still need correction; this replay does not measure
an agent's subsequent manual repair workflow. The additional checks cost about
0.702 s over the eight-project sequence in this single pair. The new placement
reports add 9,870 bytes, while removing verbose DRC findings reduces total response
size by 11,734 bytes (13.7%). These are compact UTF-8 JSON response bytes, excluding
local batch framing. They are not tokenizer or image-token measurements. One
observation per release does not establish a latency distribution or a speedup.

An independent script checks every saved native board text position against the
reported result. Across releases, device pose, footprint/device identity and pad
geometry are equal; text height, stroke width, rotation, mirroring, layer and
content are equal. Embedded project libraries are byte-identical. Only the
initial name's position changes among those geometric properties. Generated
project/instance UUIDs and creation metadata differ between fresh fixtures; no
whole-project byte identity is claimed. Matching-scale saved-board renders were inspected:

| Case | Baseline | Updated |
| --- | --- | --- |
| Top, 0 degrees | [PNG](baseline-top.png) | [PNG](updated-top.png) |
| Bottom, 90 degrees | [PNG](baseline-bottom.png) | [PNG](updated-bottom.png) |

Raw calls, responses, saved projects, replay/verification scripts, renders and
logs are retained under `/home/okhsunrog/tmp_zfs/codex_tmp/initial-legend/`.
Machine-readable selected observations: [measurements.json](measurements.json).

## Validation and boundaries

- MCP with HTTP enabled: 106 integration tests passed; two live-source tests
  ignored. All 110 editor integration tests passed. Clippy for editor/MCP with
  HTTP and all targets, formatting and diff checks passed.
- Coverage includes all eight orthogonal/mirrored cases with fresh native DRC,
  primitive/automatic/declarative paths, preview isolation, one-step undo/redo,
  native save/reopen, manual label and copper preservation, explicit targets,
  final locks, already-clear text, and rollback after a cancelled search.
- A continuous mask opening exhausts all 32 poses: the text stays at its default,
  `partial` and `no_fit` are explicit, and the native warning remains.
- A native MOSFET without courtyard/package outline retains its mask-clear label;
  unknown coverage is explicit and placement/pad workflows remain successful.
- Interactive coverage is a headless editor-FSM scenario at the final rotated and
  flipped drop pose, not a live desktop test. No full workspace suite or physical
  fabrication acceptance is claimed. The upstream CLI is absent from PATH, so no
  independent upstream executable comparison was run.
- The search is bounded and can return no fit, locked, unverified or budget
  exhaustion. It does not certify a globally optimal position or whole-board DRC.
  Cost on large populated boards is not established by this small RC fixture.
