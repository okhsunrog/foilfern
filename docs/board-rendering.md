# Board rendering

The editor's technical 2D canvas, realistic flat board rendering and 3D view
have different purposes. Realistic flat rendering supplies top/bottom board
images and vector output jobs, matching upstream `RealisticBoardPainter`.
It does not render component bodies, lighting or perspective.

`BoardPhysicalGeometry::from_project()` is the shared, owned input for
physical renderers. It resolves footprints, pad rules, stroke fonts, plane
fragments, vias and slots once, without toolkit or renderer state. Extraction
does not mutate the project or read STEP files. Callers rebuild planes before
extraction and discard stale snapshots after relevant project changes.

The snapshot contains:

- Canonical board outlines and cutouts, thickness and material colors.
- Drill paths, diameters, plating, requested depth and resolved manufacturing
  span on enabled copper layers, including blind and buried vias.
- Outer copper, mask openings, paste and selected silkscreen geometry.
- Prepared material regions for each flat projection, clipped with the
  upstream 5 micrometre arc tolerance.
- Optional assembly-variant component model references, placement and
  footprint model offsets. With no variant, component models are omitted.

The consumers are independent:

```text
Project -> BoardPhysicalGeometry -> realistic 2D paths -> Vello / SVG / PDF
                                -> future mesh builder -> 3D renderer
```

`librepcb_graphics::realistic_board_drawing()` selects material colors and
paint order from the snapshot and export settings. Output jobs reuse a
snapshot across top/bottom pages. Transparent mask/legend colors request the
board's fabrication colors; absent roles remain hidden. Copper and paste
regions use the viewed face's drill projection, even if the job enables
layers from both faces.

The resulting `Drawing::filled_areas()` retains ordered compound paths.
PNG uses `librepcb_canvas::render::rasterize_filled_paths()` and `vello_cpu`;
SVG/PDF use those same paths and the same page transform without embedding
a board bitmap. The raster path preserves even-odd cutouts directly instead
of inserting areas into the editor scene, whose fill batching normalizes
subpath winding for unions. General drawings containing text/strokes/images
continue through the existing SVG/resvg pipeline.

This headless export needs no display server or GPU. GPU texture presentation
in Slint is a separate integration; the retained paths carry no CPU/GPU state.

A future 3D mesh builder uses canonical outlines, resolved drill spans and
model references. It must not extrude a flat projection: upstream's flat
blind-via treatment differs from physical depth. Bevy is a candidate for
lighting, materials and camera control. The application workspace now has
mechanical BRep construction and STEP tessellation in `librepcb-step`;
it has no Bevy dependency or interactive 3D viewer yet.

`librepcb_step::StepModel::from_board()` consumes the canonical snapshot
to build a mechanical BRep board and placed component solids with
Monstertruck, with STEP and OBJ output. CAN–USB's component placements match
the official STEP export individually in independent OCCT checks. This
backend retains analytic arcs on whole surviving contours and explicitly
reports 5-micrometre approximation for clipped boundaries. It normalizes supported
model unit contexts to millimetres and preserves supported surface RGB/opacity
through STEP and OBJ/MTL export. Product hierarchy and other metadata are not
retained. It omits via drills to match the
upstream mechanical STEP policy; a physical viewer requires its own
depth-aware drill geometry. Mechanical PCB bodies are opaque, with the
upstream linear-RGB-to-sRGB color encoding; mask alpha belongs to the
separate physical surface renderer. The CLI injects STEP preparation through
the core job runner, retaining indexed ownership, archives and cleanup.
See `../crates/librepcb-step/README.md` and `../spikes/step/README.md` for
API boundaries and independent validation. The probe reuses the workspace
backend through its optional diagnostics feature.

The snapshot is synchronous and `Send + Sync`, so a caller can prepare it on
a worker thread. It is derived data, with no file-format or journal changes.
The project revision records its origin, rather than providing an automatic
cache invalidation mechanism.
