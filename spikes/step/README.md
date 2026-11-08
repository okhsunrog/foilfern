# Monstertruck STEP feasibility probe

Standalone Rust 2024 probe, excluded from the application workspace. Uses
Monstertruck 0.4.1 (`monstertruck-io`, not the deprecated
`monstertruck-step` re-export), modeling, topology and robust meshing.
No renderer, GPU or native CAD library is linked into the Rust executable.

The probe converts every unique shell with the **reported** API, checks
missing face tessellations, welds positions and counts open, nonmanifold and
inconsistently oriented mesh edges. It exports and reloads each local shell.
Those diagnostic files intentionally treat each shell as an independent
solid; they are not the assembly export and do not preserve void grouping.

The separate assembly pass walks **all paths from every root**, retaining
repeated instances and composing their placement matrices. Direct solid
items and solids reached through shape-representation relationships are
resolved explicitly; conversion losses and unreached solids fail the pass.
Solid boundary grouping, including void shells, survives assembly export.
`assembly.step` is a flattened collection of placed BRep solids, not triangles.

`check.py` runs each input in a separate child process with a timeout, so a
kernel panic or hang cannot look like a passing fixture. `reference.py` uses
OpenCascade only as an independent, optional test oracle. Its Python wheel
is **not an application dependency**. It measures original and exported
STEPs, compares solid/face counts, volume (1e-7 relative or absolute), bounds
(1e-6 mm), BRep validity and mesh approximations. The mesh bound allowance is
twice the requested tolerance; the volume allowance is twice surface area
times tolerance. These are diagnostic acceptance thresholds, not a proof of
the kernel's global error bound. A generated analytic plate additionally
checks exact dimensions and volume independently of import/round-trip code.

## Reproduce

Set `LIBREPCB_UPSTREAM_DIR` to the checkout described in the root
`AGENTS.md`. Supply an external official CAN–USB `.lppz` archive, without
copying its fixtures into this repository. For this standalone experiment:

```sh
export STEP_WORK=/home/okhsunrog/tmp_zfs/codex_tmp/librepcb-step
mkdir -p "$STEP_WORK"
export TMPDIR="$STEP_WORK" CARGO_TARGET_DIR="$STEP_WORK/target"
cargo build --manifest-path spikes/step/Cargo.toml --bins
"$CARGO_TARGET_DIR/debug/plate" "$STEP_WORK/plate.step"
uv run --no-project python spikes/step/check.py \
  --binary "$CARGO_TARGET_DIR/debug/step-spike" \
  --archive /absolute/path/to/can2usb.lppz \
  --input "$STEP_WORK/plate.step" --output "$STEP_WORK/verified"
uv run spikes/step/reference.py \
  "$STEP_WORK/verified/summary.json" "$STEP_WORK/verified/reference.json"
cargo fmt --manifest-path spikes/step/Cargo.toml --check
cargo clippy --manifest-path spikes/step/Cargo.toml --all-targets -- -D warnings
uv run --no-project --with ruff ruff check spikes/step/*.py
```

On NixOS, the reference wheel additionally needs the runtime library paths
for GL, X11 and expat. Supply them through the diagnostic environment; do not
add them or OpenCascade to the Rust application.

## Measured 2026-11-08

All 13 cases passed both the Rust geometry checks and independent OCCT
checks at 0.01 mm tessellation tolerance:

- Ten packaged STEP models from CAN–USB, official example-projects commit
  `3aad10dae67ecfea9535a2cac328497d67e4d142` (all packaged alternatives,
  not only the active assembly model choices).
- `colors.step` and `model.step` from upstream `OccModelTest`, reference
  checkout `7f1b548abb61bd59b2b9c1166eca3e6557480c17`.
- An analytic 20 x 10 x 1.6 mm plate with a diameter-3 mm through-hole and
  a 4 x 2 mm rectangular cutout: volume 295.890266447 mm3, one solid,
  fourteen faces. OCCT's bound padding is 1e-7 mm.

The ten component files contain 1, 1, 26, 1, 4, 12, 1, 8, 7 and 8 placed
solids, respectively (archive path order). Counting unique shells alone
would lose repeated leads; a lossless shell report alone is insufficient.

## Acceptance boundary

This demonstrates geometry feasibility on these fixtures, **not a general
production STEP importer**. In particular:

- Coordinates are used as stored. The tested files use millimetres; unit
  contexts and conversions need an explicit adapter before general import.
- The table leaves colors, styles and other unsupported records in `dummy`.
  `entity_report == 0` does not prove metadata preservation. Even the
  `colors.step` result above establishes geometry only.
- Export flattens instance structure and loses names, colors and metadata.
- Surface-only models, mapped representations, additional DATA sections,
  unsupported analytic entities and adversarial/resource-heavy input need
  dedicated coverage and explicit rejection or support.
- Kernel internals may panic; the process boundary belongs to the probe,
  not an in-process production error strategy.

No CLI STEP acceptance case is closed by this isolated experiment.

## Shared physical board prototype

The `board` executable consumes the canonical `BoardPhysicalGeometry` from
the real project loader. It subtracts cutouts and non-via through-drills
with the existing Clipper adapter, then extrudes the resulting regions
with Monstertruck. Overlapping cutouts, cuts crossing the board edge and
disconnected material regions are supported. It never extrudes a flat
surface projection. Non-via drills with unsupported depth fail explicitly.

This is the upstream **mechanical STEP policy**: all via drills are omitted,
as in `StepExport::run()`. A physical visualization mesh will need a separate
depth-aware policy, including blind/buried via geometry. It must not use this
mechanical simplification as its physical drilling model.

Component BReps are cached by package/model, expanded by assembly instance
and placed using `OccModel::addToAssembly()`'s transform order: footprint
translation/rotation, bottom-side rotation about Y, model offset and Z/Y/X
model rotations. Top-side models start at PCB thickness; bottom-side models
start at zero. STEP generation and OBJ tessellation consume those same
placed solids. Missing models and failed conversions/tessellations fail
instead of disappearing silently. The generated PCB BRep is reloaded to
unify Monstertruck's distinct modeling and STEP-import geometry enums.

The prototype requires `--assume-mm` because it has no unit-context adapter.
It does not preserve model colors, names/assembly hierarchy in STEP, or add
copper/mask/silk meshes. There is no Bevy viewer or CLI output-job integration.

```sh
"$CARGO_TARGET_DIR/debug/board" --assume-mm \
  /absolute/path/to/can2usb.lppz "$STEP_WORK/can2usb-3d"
"$LIBREPCB_CLI" open-project --run-job 'STEP Model' \
  --outdir "$STEP_WORK/can2usb-oracle" /absolute/path/to/can2usb.lppz
uv run spikes/step/compare-board.py "$STEP_WORK/can2usb-3d" \
  "$STEP_WORK/can2usb-oracle/can2usb_v2.step"
cargo test --manifest-path spikes/step/Cargo.toml
```

The optional trailing board/variant indices default to zero. The read-only
project loader accepts `.lpp` or `.lppz`; it does not save migrated files.
The output directory contains `pcb.step`, `assembly.step`, `assembly.obj`
and a diagnostic `board.json`. The comparison adds `comparison.json`.

On CAN–USB's default assembly, measured 2026-11-08:

- 27 placed devices, 86 component solids and one PCB solid;
  all 87 BReps valid according to OCCT.
- 53,400 triangles; no boundary, nonmanifold or inconsistently oriented
  mesh edges, checked per solid.
- All 86 component solids individually match the official LibrePCB export's
  bounds, volume, center of mass and face count. Maximum component bound
  difference: 7.2e-15 mm. Full assembly bounds are identical:
  `(0, 0, -1.4)` to `(60, 16.44, 9.6)` mm.
- PCB volume difference: 0.146637487 mm3 out of 1563.788673793 mm3
  (0.0094%), from flattening PCB arcs within 0.005 mm. It has additional
  planar side faces in place of analytic curved surfaces; this is not exact
  BRep equivalence for the PCB itself.

Three focused Rust tests cover canonical geometry vs flat projections,
overlapping/edge-crossing cutouts, mechanical drill policy and non-through
rejection, and independently computed top/bottom model placements.

Next: adapt units and materials, retain analytic PCB arcs where feasible,
and establish broader fixture/error coverage before moving the adapter
into the application workspace. Rendering integration remains independent.
