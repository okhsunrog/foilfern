# Status and handoff

State of the port as of 2026-11-09, for continuing the work in a new
session (for example a cloud session). Read `AGENTS.md` first; this file
covers where things stand and how the work has been organized.

## Where we are

Milestone M1 (headless `librepcb-cli`, see `roadmap.md`) is in progress.
The workspace builds cleanly and 1047 tests report success (about 100k lines
of Rust). The latest local run used the official CLI 2.1.1 as the oracle,
including ERC messages, migrations and byte-identical JSON exports over
the pinned upstream test data.

Done, in `crates/librepcb-core` unless noted:

- **Base:** types, the S-expression format, the serialization traits, and utils.
  Upstream Qt semantics are dropped, with the divergences listed in `COMPAT.md`.
- **Geometry, stroke fonts, polygon clipping:** `crates/clipper` is a
  faithful port of Clipper 1, verified differentially against the C++
  library.
- **Attributes, air wires** (spade + petgraph), the math parser (evalexpr),
  and SQLite.
- **File I/O** (transactional file system, directory locks, ZIP, CSV) and
  system info; `crates/librepcb-network` (tokio + reqwest).
- **Libraries:** all element types and their checks. Check messages and
  approvals are identical to upstream `librepcb-cli --check`; the package
  check ports Qt's painter path predicates on purpose, because approvals
  are stored in files.
- **Export generators:** Gerber, Excellon, IPC-D-356A, BOM and pick & place.
- **File format migrations** v0.1 → v1 → v2. Migrated projects and
  libraries match `librepcb-cli` byte for byte, including `jobs.lp`.
- **Project model** (`project/`), designed in `project-model-design.md`:
  circuit, schematics and boards with mutations, inverses, the reverse
  index, the change journal and serde. All test projects round-trip byte
  for byte.
- **`crates/librepcb-canvas`:** a retained 2D scene rendered with vello_cpu,
  with hit testing and a Slint adapter.
- **Shared physical board geometry** (2026-11-08): an owned snapshot resolves
  outer material regions, cutouts, slots and drill spans, colors/thickness and
  optional assembly model placements. Flat regions reproduce upstream's
  physical top/bottom projection; canonical geometry remains available to a
  future 3D mesh builder. Extraction is read-only and has no graphics or STEP
  dependency. See `board-rendering.md`.
- **Slint snapshot** (2026-11-08): canvas and standalone prototypes pin
  upstream master `aebe3dd580ce0594de272bc5797b7444a2aa18fd` (package 1.20.0,
  still a development snapshot). UI builds select edition 1.20. The optional
  Slint integration, full renderer spike and bundled-translation check build
  against the same revision; translation runtime checks pass.
- **Slint runtime checks** (2026-11-08): the actual Rust canvas demo was
  inspected through its embedded MCP server (18 tools), operated with wheel,
  keyboard and rectangle-selection input, and captured as PNGs. Accessible
  state confirms zoom and deletion; the event log has no drops or unknown
  variants. `scripts/check-slint-mcp.py` repeats this headless software test.
  See `slint-tooling.md`; a desktop GPU/window-system run is still separate.
- **`crates/librepcb-i18n`, `tools/ts2po`, `lang/`:** translations from the
  upstream catalogs, with Slint bundled translations.
- **Project attribute lookup** (2026-11-01): built-in and custom attributes
  in project, assembly variant, component, schematic, symbol, board and
  device contexts, including part precedence and recursive substitution.
- **BOM generator** (2026-11-01): generic and board-specific BOMs, assembly
  variants, alternative parts, custom common/part columns, unmounted part
  fallbacks, and the glue to the existing CSV writer.
- **ERC** (2026-11-01): all upstream electrical rules, typed findings,
  translated messages, byte-compatible approvals and schematic highlighting.
  Tested on all upstream project fixtures; CLI comparison runs when the
  upstream executable is available. After migrations the loader keeps only
  approvals of findings which still occur; current-format loads preserve
  all existing approvals.
- **Project JSON export** (2026-11-01): upstream format 1.0 with project
  metadata, assembly variants and board manufacturing statistics, including
  outline bounds, physical via drill spans and automatic drill diameters.
  JSON bytes match the official CLI for all upstream project fixtures.
- **Via manufacturing data** (2026-11-01): computed drill/copper diameters,
  stop-mask openings and physical drill spans with inherited net-class and
  board rules. Shared by JSON export and subsequent fabrication exporters.
- **Board pick & place** (2026-11-01): variant selection, transformed
  placements, assembly types and footprint/standalone fiducials. Top,
  bottom and combined CSV bodies match the official CLI byte for byte for
  every board and variant in the upstream project fixtures.
- **Board IPC-D-356A export** (2026-11-01): resolves via geometry,
  footprint/standalone pad positions, net names, rotations and layer access
  into the existing generator. Original upstream PnP/D356 golden tests pass;
  D356 files match the official CLI for all fixture boards after normalizing
  the generation-date comment.
- **Plane fragments** (2026-11-01): upstream clipping, priorities, keepouts,
  thermal spokes, minimum widths, island removal and canonical contours.
  The Nested Planes golden file matches byte for byte; simultaneous builds
  pass on the upstream 40-inner-layer fixture. Builds are synchronous with
  progress/cancellation and atomic application through the change journal.

- **Board Gerber/Excellon export** (2026-11-01): fabrication layers,
  split/merged and blind/buried drills, routed/G85 slots, glue and X3
  component layers with variant selection. Original upstream Gerber golden
  files match; PCB outputs and every board/variant's glue/X3 files match
  official CLI 2.1.1 after normalizing software/date/checksum metadata.
  FontoBene glyph spacing retains upstream's float precision, preserving
  nanometer placement after whitespace. Exporters return named file contents;
  the output job runner owns writing and obsolete-file cleanup.

- **DRC rules** (2026-11-06): all upstream manufacturing, width/text,
  clearance, annular-ring, keepout, outline/cutout, via, device-overlap,
  connectivity and stale-object checks operate on owned `BoardDrcData`.
  Full fixture coverage includes 222 reference findings and official CLI
  text/severity comparisons. Missing-connection ties use equivalent spanning
  trees: tests verify identical anchors and edge weights (see `COMPAT.md`).
  Other approval nodes match byte for byte. Extraction rebuilds planes and
  air wires in a private copy and leaves the project untouched. Full/quick
  execution, progress and atomic cancellation are available through the
  common result API. Approval cleanup is journaled, preserves unknown
  approvals and waits for a full run after a file-format upgrade.

- **Output job model** (2026-11-06): all 12 job kinds, board/variant
  selections, dependency references, unknown jobs and arbitrary options.
  Projects and organizations use typed settings; project edits are mutations
  with inverses and journal entries. Current files round-trip byte for byte;
  migrated `jobs.lp` now participates in official CLI comparisons.

- **Output job runner** (2026-11-06): manufacturing, assembly, BOM,
  netlist, JSON, copy, ZIP and LPPZ jobs use the indexed output writer.
  It resolves board/variant selections in project order, rebuilds planes
  before fabrication, rejects duplicate names and removes obsolete files
  after successful jobs. Archives consume only previously executed jobs.
  Upstream security/rebuild test cases and official CLI output comparisons
  pass. Schematic and 2D board graphics are encoded by an injected renderer
  in `librepcb-graphics`, retaining indexed ownership, archive dependencies,
  collision checks and cleanup. Board graphics rebuild planes before encoding.
  Realistic top/bottom board rendering also uses this writer and the shared
  physical snapshot. Interactive BOM HTML jobs execute directly through the
  upstream Rust generator. Mechanical STEP jobs now inject `librepcb-step`
  through a separate callback, retaining the same file ownership and archives.

- **Native CLI core** (2026-11-06): `crates/librepcb-cli` opens `.lpp`/`.lppz`
  projects and libraries, runs ERC/DRC and library checks, executes output
  jobs, exports manufacturing/assembly/BOM/netlist files, selects boards and
  variants, removes boards and changes the default variant through mutations,
  checks canonical files and saves/migrates transactionally. Runs without a
  display server. Symbol/package graphics and schematic exports are supported.
  Standalone `open-step` loads/validates models, copies with `--save-to`, and
  tessellates with `--tesselate` using the same CPU mesh as OBJ export.
  `open-step --minify` and library `--minify-step` retain the STEP document
  graph, units and surface styles; library changes validate before staging and
  persist only with `--save`. `--strict` detects changes without writing them.
  `scripts/check-cli-core.sh` covers all 154 implemented-command cases.
  The full upstream suite reports 158 passed / 20 failed, with all functional
  cases passing. Remaining failures are Qt parser/help/version snapshots;
  native clap behavior and the real package version are intentional (COMPAT).
  All thirteen output-job cases pass.
  M1 is not complete.

- **Graphics export backend** (2026-11-08): `crates/librepcb-graphics`
  provides owned drawing lists, native text, layer palettes and page layout,
  SVG/PNG and vector multipage PDF encoding. Bounds, scale/orientation,
  image page names, decoded raster pixels and selectable PDF text are tested.
  Library symbol/footprint and schematic painters use resolved model views,
  attribute contexts, embedded images and stroke fonts. Their CLI exports pass
  23 unmodified upstream tests, including output naming/collision behavior.
  Model-content tests cover pins, overlines, images, pad holes and read-only
  schematic export. Demo symbol PDF renders were visually compared with the
  official CLI. The board painter covers placed footprints, copper fills,
  pads/vias, masks, traces, holes and stroke text. Schematic and board PDF
  jobs match the official CLI's filenames and page counts; both sides of
  Gerber Test's assembly PDF were visually compared. Encoding retains native
  font/raster differences documented in `COMPAT.md`.
- **Realistic 2D board rendering** (2026-11-08): top/bottom output jobs consume
  the shared physical snapshot. Board colors, copper, mask, clipped silk and
  paste become retained even-odd material paths; PNG rasterizes with Vello CPU,
  while SVG and PDF retain vector contours. Raster/vector agreement is tested
  for mirrors, rotations, fixed paper and transparency. Realistic PDF job
  names/page counts match official CLI 2.1.1, and both sides of Gerber Test's
  PNG output were visually compared. This does not implement 3D or a viewer.
- **Interactive HTML BOM** (2026-11-08): resolves footprints, pads,
  slots, nets, tracks, plane fragments, stroke drawings and grouped assembly
  rows into the existing `interactive-html-bom` Rust crate. The original
  `BoardInteractiveHtmlBomGeneratorTest` HTML matches byte for byte; fixture
  checks cover variants, custom columns, DNP and read-only extraction. Output
  jobs rebuild planes and preserve indexed ownership, archives, collision
  checks and cleanup. Official CLI comparisons cover emitted configuration
  and PCB data, excluding time. CAN–USB was exercised offline in headless
  Chromium: filtering, footprint highlights, sides, themes and persistent
  checkboxes, with no HTTP requests or JavaScript errors. Desktop/compact
  light/dark screenshots were reviewed. `scripts/check-ibom-browser.py`
  repeats this check; see `interactive-bom.md`.

## Next steps

`crates/librepcb-step` now provides the application CAD backend. Its guarded
`StepModel` API imports models, prepares mechanical boards and exports
STEP/OBJ/MTL without a display or GPU. Ordinary kernel panics become typed
errors; input size/nesting limits are explicit. Core has no CAD dependency:
the native CLI injects the backend into the indexed output-job runner.
Preparation failures preserve previous output files; archives consume STEP
files through the normal job dependency mechanism. Board-only variants work,
and project revision/files stay unchanged during CAD preparation.

The `spikes/step` diagnostic harness reuses the workspace implementation and
validates Monstertruck 0.4.1 geometry
on all ten CAN–USB STEP files, two upstream OccModelTest fixtures and an
analytic drilled plate. Reported conversion, watertight tessellation,
assembly instances/placements and BRep re-export pass independent OCCT
solid/face-count, volume, bounds and validity checks. Its board executable
also consumes the shared physical snapshot and produces a CAN–USB assembly
with 27 devices / 87 valid solids and a watertight 67,932-triangle mesh.
Every component solid matches the official STEP export individually; whole
surviving PCB contours retain analytic arcs. CAN–USB needs zero flattened
contours, with a 0.000011741 mm3 PCB volume difference from the official
export. Intersecting boundaries retain an explicit 5 um approximation count.
Mechanical export intentionally omits via drills, as upstream does.
The unit adapter now normalizes uniform SI/conversion-based contexts to
millimetres (16 independent fixture comparisons); mixed contexts and
unsupported angle units fail explicitly. Surface RGB/opacity now survive
STEP re-export and OBJ/MTL tessellation, including solid defaults and face
overrides. Seventeen models pass independent geometry/XCAF checks; all
2,048 appearances, including the opaque substrate, of the native CLI's
assembled CAN–USB match the official export. New board colors reproduce
upstream's linear-RGB-to-sRGB encoding; imported STEP RGB is retained.
Fifteen CAD tests, two callback/archive tests and compile-time thread-safety
assertions cover the integration. Exact curve intersections, broader
metadata/representation coverage and an interactive
viewer remain pending. See `../crates/librepcb-step/README.md` and
`../spikes/step/README.md` for boundaries and reproduction.

Standalone STEP validation/tessellation and document minification are now
implemented. The minifier ports upstream's fixed-point deduplication and
representation-identity rule; literal contents are protected from regex-style
rewrites. All 18 independent probes (including the 87-solid CAN–USB assembly)
produce byte-identical minified output to the official CLI, with unchanged
OCCT geometry metrics and XCAF face appearances. The upstream small-model
fixture shrinks from 28,521 to 18,336 bytes and remains idempotent. Native tests
also verify that failed library geometry validation retains the original model.
Artifacts: `/home/okhsunrog/tmp_zfs/codex_tmp/librepcb-step/open-step/`
(`verification.json`, `upstream-cli.log`).

1. **Wave 4 acceptance:** lock down native parser/help/version contracts and
   record intentional differences from Qt snapshots. Mechanical STEP jobs,
   standalone tools, minification and all mixed job cases now execute.
   Keep running the implemented paths with `scripts/check-cli-core.sh`.
2. **Integrate the existing M1.5 implementation.** The separate remote branch
   `origin/claude/ecstatic-carson-u1ye4i` (tip `3adbf58` at inspection) already
   contains `librepcb-mcp`, editor/library/import/autoroute and application work.
   Its MCP server supports stdio/HTTP, project reads/writes, undo/redo, library
   operations and embedding. These crates are absent from this checkout's
   `master`; reconcile the branches and validate the integration instead of
   implementing a second server. `roadmap.md` describes this branch's baseline.

Deliberately deferred: live viewer/editor integration (M2), Specctra
(M1.5), interactive 3D viewing, and the Eagle/KiCad importers
(M5).

## How the work is organized

- Work goes in **waves** of parallel agents, each in its own git worktree
  (`../librepcb-rs.w<N>-<name>`, branch `wave<N>/<name>`) with a clearly
  separated file ownership. The coordinating session reviews each branch
  (fmt, clippy, tests, no hardcoded paths), merges it into `master` and
  pushes.
- Related small tasks are batched per agent (about 5–10 agents per wave),
  rather than one agent per tiny task.
- Architecture and design work (design documents) runs on the strongest
  available model, and the design is reviewed with the user before
  implementation starts.
- Acceptance is always mechanical where possible: upstream unit test
  vectors, byte-identical round trips over upstream `tests/data`, and diffs
  against the official `librepcb-cli`.
- Each intentional divergence from upstream goes into `COMPAT.md`.

## Environment

Run `scripts/cloud-setup.sh` on a fresh machine. It sets up the pinned
upstream checkout next to the repository and the official `librepcb-cli`
behind Xvfb; after that, `cargo test --workspace` runs everything,
including the upstream comparisons.

## Open items to remember

- Adding and removing project library elements are `Project` methods, not
  `Mutation`s. They must become undoable when the command bus is designed
  (M1.5), because agents will add parts through MCP.
- Upstream bugs found along the way, which could be reported upstream:
  Qt/Transifex plural form order mismatches in several languages, `"1e-3"`
  rejected as a length, and a v0.1 circular board outline that cannot be
  migrated.
