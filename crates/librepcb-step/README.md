# librepcb-step

Synchronous, pure Rust CAD backend for LibrePCB's mechanical STEP output.
Uses Monstertruck 0.4.1 for BReps, curve/surface construction and tessellation;
the STEP metadata adapter uses its re-exported `step-p21` AST. No Qt,
OpenCascade/C++ FFI, GPU, windowing or async runtime is required.

`StepModel::from_step()` imports supported solid assemblies in millimetres.
`StepModel::from_board()` consumes an owned `BoardPhysicalGeometry` snapshot
and loads component models from the read-only project filesystem. It caches
package models within one export and applies footprint/model placement once
per instance. `None` assembly variant produces a mechanical board without
component bodies. Callers rebuild planes before extraction.

`to_step()` writes BRep solids with surface RGB/opacity; `to_obj(tolerance)`
independently tessellates and returns OBJ plus `assembly.mtl`. Keep both
files together. `StepModel` is owned and `Send + Sync`; callers may run it
on their worker threads. The crate does not retain a project or GPU handle.
`tessellation_stats(tolerance)` counts expanded triangle vertices and distinct
surface appearances using the same mesh as OBJ export. `librepcb-cli open-step`
validates standalone models; `--tesselate` reports these counts without a display.

`from_components(project, snapshot)` prepares placed component bodies separately
from the mechanical board. `triangle_surfaces(tolerance)` exposes owned triangle
positions, surface normals and sRGB/opacity directly to renderers, without
serializing OBJ or introducing a rendering dependency into the CAD backend.

`minify_step(bytes)` is a separate document transformation, porting upstream's
fixed-point deduplication without a BRep round-trip. It retains header/footer
records, assembly/representation identities, units and styles. References and
negative zero are rewritten only outside literals. Invalid identifiers,
duplicate IDs, unresolved links and multiple DATA sections fail explicitly.
The CLI exposes `open-step --minify [--save-to file]` and
`open-library --all --minify-step [--strict|--save]`. Library replacements are
validated before staging; `--strict` leaves the original files on disk.

The core output job runner injects this backend through `BoardStepExporter`,
without taking a CAD dependency itself. Core resolves boards/variants,
templates, file ownership/collisions, archive inputs and obsolete-file cleanup.
The CLI prepares complete bytes before opening the output file, so failed
model preparation preserves an existing export.

## Geometry and compatibility boundary

- Mechanical STEP follows upstream: non-via through holes and cutouts are
  subtracted, all via drills omitted. A physical viewer needs its own
  depth-aware drill geometry and separate copper/mask/legend surfaces.
- Complete surviving contours retain analytic arcs. Intersected boundaries
  use the existing 0.005 mm Clipper approximation and are counted by
  `approximated_contours()`; the CLI prints a warning when this occurs.
- SI prefixes and recursive conversion-based length units are normalized
  along with instance placements. Mixed length contexts and non-radian angle
  units are explicitly rejected pending their adapters.
- Standard RGB/named colors, fill/rendering styles, opacity, solid/shell
  defaults and face overrides are supported. Unsupported presentation
  semantics fail explicitly. This is not a full PBR/material implementation.
- Product names/hierarchy and arbitrary metadata are not yet preserved in
  the flattened STEP export. Surface-only/mapped models need broader support.
- Application entry points catch ordinary kernel unwinds and return typed
  errors; parser input is limited to 256 MiB and 256 nesting levels. Process
  aborts, memory exhaustion and pathological long-running operations are not
  solved by `catch_unwind`; stronger isolation/cancellation remains future work.

Upstream fixtures are referenced through `LIBREPCB_UPSTREAM_DIR`, never
copied. Rust tests cover the guarded API, read-only board preparation,
top/bottom placements, arcs/cutouts, units, styles and independent OBJ/MTL
reading with `tobj`. `spikes/step` now reuses this crate's optional
`diagnostics` feature for isolated fixture probes and independent OCCT/XCAF
comparisons; see its README for reproduction and measured results. OpenCascade
is used only as an external oracle, never an application dependency.
