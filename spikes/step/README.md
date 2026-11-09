# Monstertruck STEP feasibility probe

Standalone Rust 2024 harness, excluded from the application workspace; its
CAD implementation now lives in `crates/librepcb-step`, reused through the
`diagnostics` feature. Uses
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

- Lengths are normalized to millimetres from SI prefixes or recursive
  conversion-based units (including inches/feet). The same scale applies
  to assembly placements. Mixed length contexts and non-radian angle units
  fail explicitly; they are not silently treated as millimetres/radians.
  Missing/invalid units, cyclic definitions, duplicate entity IDs and
  multiple DATA sections are rejected before geometry conversion.
- The table leaves colors/styles in `dummy`; the explicit AST adapter now
  restores supported surface RGB and opacity. `entity_report == 0` still
  does not establish preservation of arbitrary STEP metadata.
- Export flattens instance structure and loses product names/hierarchy and
  metadata beyond the supported surface appearance.
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

The prototype reads model unit contexts and normalizes to millimetres.
It preserves supported surface colors/opacity in STEP, JSON appearance data
and OBJ/MTL. It does not preserve names/assembly hierarchy in STEP or add
copper/mask/silk meshes. There is no Bevy viewer or CLI output-job integration.

```sh
"$CARGO_TARGET_DIR/debug/board" \
  /absolute/path/to/can2usb.lppz "$STEP_WORK/can2usb-3d"
"$LIBREPCB_CLI" open-project --run-job 'STEP Model' \
  --outdir "$STEP_WORK/can2usb-oracle" /absolute/path/to/can2usb.lppz
uv run spikes/step/compare-board.py "$STEP_WORK/can2usb-3d" \
  "$STEP_WORK/can2usb-oracle/can2usb_v2.step"
cargo test --manifest-path spikes/step/Cargo.toml
```

The optional trailing board/variant indices default to zero. The read-only
project loader accepts `.lpp` or `.lppz`; it does not save migrated files.
The output directory contains `pcb.step`, `assembly.step`, `assembly.obj`,
`assembly.mtl` and a diagnostic `board.json`. Keep OBJ and MTL together.
The comparison adds `comparison.json`.

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

Next: retain analytic PCB arcs where feasible,
and establish broader fixture/error coverage before moving the adapter
into the application workspace. Rendering integration remains independent.

## Unit checks (2026-11-09)

```sh
uv run --no-project python spikes/step/make-unit-fixtures.py \
  "$CARGO_TARGET_DIR/debug/plate" "$STEP_WORK/units/fixtures"
# Add the three generated .step inputs to check.py with --input, then run
# reference.py on its summary.json as above.
```

The generated centimetre, metre and inch files describe the same 20 x 10 x
1.6 mm physical plate, with independently computed expected dimensions and
volume. All sixteen cases (the original thirteen plus these three) pass the
Rust geometry and OCCT round-trip/measurement checks at 0.01 mm. CAN–USB's
87 solids continue to match the official export as described above.
Five Rust tests cover board geometry/placement and SI/conversion-based
unit resolution, missing/cyclic units and mixed context rejection.

Very large geometry at a fixed fine tolerance remains resource-sensitive:
an initial 20-metre variant exceeded the probe's 45-second child timeout.
The equivalent-size fixtures above isolate unit correctness; they do not
establish performance for unusually large or adversarial models.

## Surface appearance checks (2026-11-09)

`materials` resolves standard RGB and named colors, surface fill/rendering
styles, transparency and explicit `OVER_RIDING_STYLED_ITEM` chains. Local
face appearance overrides shell/solid defaults. Conflicting styles, cycles,
unknown target kinds, directional/context-dependent styles and unsupported
optical properties fail explicitly. Curve/point styling is outside the
surface model. This is an RGB/opacity adapter, not a complete PBR/material
or STEP presentation implementation.

The exporter recovers **new** solid/shell/face IDs from Monstertruck's output
AST and appends AP214 presentation entities. Input entity numbers are never
reused. The same per-face material association accompanies tessellation and
becomes OBJ `usemtl` groups and MTL diffuse/opacity values. Unstyled OBJ faces
get an explicit neutral preview default; the CAD appearance data remains
`None`. CAD RGB values are retained without a gamma conversion (the OCCT
oracle reads them back in sRGB); future rendering must choose its color
space explicitly. The mechanical PCB color follows the snapshot resist
color, darkened for STEP as upstream, rather than modeling surface layers.

```sh
"$CARGO_TARGET_DIR/debug/appearance" "$STEP_WORK/plate.step" \
  "$STEP_WORK/known-appearance.step"
# Include known-appearance.step as another --input in check.py.
uv run spikes/step/appearance_reference.py "$STEP_WORK/verified/summary.json" \
  "$STEP_WORK/verified/appearance-reference.json" \
  --negative-control "$STEP_WORK/negative-control"
```

All seventeen inputs (the original thirteen, three unit variants and a
four-color/translucent plate) pass geometry and independent XCAF appearance
checks. The oracle compares individual placed faces by geometry, surface
area, RGB and alpha; inherited solid colors and face overrides resolve
through OCCT's own XCAF style collector. A deliberately recolored STEP is
rejected as a negative control. The generated plate separately checks known
RGB/opacity face counts, so a self-consistent round-trip is insufficient.

CAN–USB retains 87 valid solids / 53,400 closed-mesh triangles. All 86
component solids still match the official export, and all 2,024 component
face colors/opacity now match individually. The PCB's approximated contours
remain excluded from exact face correspondence; its geometry acceptance
boundary above is unchanged. `compare-board.py` checks component appearance
in addition to geometry.

Nine Rust tests pass, including independent OBJ/MTL parsing with `tobj`,
material/triangle assignment, opacity, override priority, conflicts and
cyclic style rejection. Both the parsed document and placed BReps have
compile-time `Send + Sync` assertions. Monstertruck remains isolated in this
prototype pending exact PCB curves, broader coverage and application wiring.

Official LibrePCB 2.1.1 also reads and tessellates both new exports with
`open-step --tesselate`: CAN–USB reports 133,200 vertices / five colors, and
the known-appearance plate reports 732 vertices / four colors. This checks
that the original application can consume the output, in addition to the
independent OCCT 8 oracle; it does not complete native CLI STEP integration.

## Analytic PCB contours (2026-11-09)

Whole source contours surviving planar subtraction now retain their analytic
line/arc wires. Contour matching is independent of winding and cyclic start
vertex, using Clipper's normalized boundary solely for classification. The
profile kernel constructs the exact circular curves from source tangents;
there is no custom Boolean/curve-intersection kernel. Boundaries changed by
clipping still use 5 um polylines, counted in `approximated_contours` instead
of being silently advertised as exact. This is not general analytic CSG.

CAN–USB needs zero approximated contours. Its PCB volume differs from the
official export by 0.000011741 mm3 (relative 7.6e-9), and bounds by 1e-7 mm;
the previous 0.1466 mm3 approximation difference is gone. Small differences
remain from analytic curve construction vs upstream's nanometre-rounded
circle radii. Component BReps and all 2,024 face appearances remain matched.
The analytic-body mesh has 67,932 total triangles at 0.01 mm tolerance, with
no open/nonmanifold/misoriented edges. Ten Rust tests now also cover both
arc winding directions, analytic side-face count and explicit fallback on
intersecting curved cutouts.

## Workspace and native CLI integration (2026-11-09)

The guarded application API is `librepcb_step::StepModel`; see
`../../crates/librepcb-step/README.md`. The harness no longer owns a separate
copy of the CAD implementation. All seventeen probe inputs still pass
geometry and independent XCAF checks after the move.

The native CLI now runs `3d_model` jobs through the ordinary indexed writer,
including board-only variants, archives and stale-file cleanup. Geometry
preparation errors leave existing STEP output intact. All thirteen unmodified
upstream output-job tests pass; the full CLI suite is 149 passed / 29 failed
(remaining standalone STEP/minification and parser/help/version coverage).

The native CAN–USB export matches all 2,048 face appearances, including PCB
surfaces, against the official export. PCB bodies are opaque; newly derived
board colors undergo upstream's linear-to-sRGB conversion. Imported STEP
colors/opacity retain their source values. The earlier 2,024-face check
covered only components and did not establish PCB color correctness.

To validate the native CLI file using a probe run's board metadata:

```sh
cargo run -p librepcb-cli -- open-project --run-job 'STEP Model' \
  --outdir "$STEP_WORK/native" /absolute/path/to/can2usb.lppz
uv run spikes/step/compare-board.py "$STEP_WORK/can2usb-3d" \
  "$STEP_WORK/can2usb-oracle/can2usb_v2.step" \
  --assembly "$STEP_WORK/native/can2usb_v2.step" \
  --output "$STEP_WORK/native/comparison.json"
```

For zero-approximation boards the comparison now includes every face's RGB
and opacity, in addition to the component-only and geometry checks. The
native CLI generates STEP here; OBJ/MTL and board metadata are diagnostic
outputs of the harness, not extra files emitted by the STEP output job.
