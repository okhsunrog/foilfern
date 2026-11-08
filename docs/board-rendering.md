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

The intended consumers are independent:

```text
Project -> BoardPhysicalGeometry -> realistic 2D paths -> Vello / SVG / PDF
                                -> future mesh builder -> 3D renderer
```

A future 3D mesh builder uses canonical outlines, resolved drill spans and
model references. It must not extrude a flat projection: upstream's flat
blind-via treatment differs from physical depth. Bevy is a candidate for
lighting, materials and camera control; no Bevy dependency, mesh builder,
STEP tessellation or 3D viewer is implemented yet.

The snapshot is synchronous and `Send + Sync`, so a caller can prepare it on
a worker thread. It is derived data, with no file-format or journal changes.
The project revision records its origin, rather than providing an automatic
cache invalidation mechanism.
