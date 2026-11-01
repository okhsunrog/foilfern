# Status and handoff

State of the port as of 2026-11-01, for continuing the work in a new
session (for example a cloud session). Read `AGENTS.md` first; this file
covers where things stand and how the work has been organized.

## Where we are

Milestone M1 (headless `librepcb-cli`, see `roadmap.md`) is in progress.
The workspace builds cleanly and 973 tests report success (about 85k lines
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
  libraries match `librepcb-cli` byte for byte, except `jobs.lp`.
- **Project model** (`project/`), designed in `project-model-design.md`:
  circuit, schematics and boards with mutations, inverses, the reverse
  index, the change journal and serde. All test projects round-trip byte
  for byte.
- **`crates/librepcb-canvas`:** a retained 2D scene rendered with vello_cpu,
  with hit testing and a Slint adapter.
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
  output jobs will own writing and obsolete-file cleanup.

## Next steps

1. **Wave 3c:** DRC (the largest remaining core piece, about 8k lines
   of C++).
2. **Wave 4:** the `job` module (output jobs, which also fixes the pending
   `jobs.lp` round trip), the output job runner and the CLI. Acceptance:
   upstream `tests/cli` (pytest, run with `uv`) passing against our CLI,
   plus output diffs against the official `librepcb-cli`.
3. **Then M1.5** (MCP, library manager, Specctra) per `roadmap.md` and the
   decisions in `mcp-research-konnect.md`.

Deliberately deferred: the schematic/board painters (M2), Specctra
(M1.5), the interactive HTML BOM, 3D/STEP, and the Eagle/KiCad importers
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
