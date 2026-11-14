# Canvas rendering spike

Throwaway prototype to choose the board/schematic canvas architecture for the
Slint UI. It loads LibrePCB boards from `../LibrePCB/tests/data` (hand-written
loader, not foilfern-core) or a synthetic stress scene, and renders them with
several approaches, with pan, zoom and R-tree hit-testing.

## Results

Hardware: Intel Core Ultra 5 125H (Arc iGPU), Mesa 26.2, KDE Wayland, window
2161×1351 px. Stress scene: 124k items (100k traces, 16k pads, 4k vias).
Frame times in ms (16.7 = 60 Hz vsync floor):

| Approach | Full view | 8× zoom | 40× zoom |
|---|---|---|---|
| A: Slint `Path` elements, femtovg | 102 | 17 | 19 |
| A: Slint `Path` elements, femtovg-wgpu | 130 | 17 | 19 |
| A: Slint `Path` elements, skia | 330 | 79 | 82 |
| A: Slint `Path` elements, Slint's vello | 143 | 30 | 34 |
| A: Slint `Path` elements, software | 330 | 190 | crashes |
| B: vello → wgpu texture, Slint on femtovg-wgpu | 52 | 16.7 | 16.7 |
| vello_cpu (8 threads) → `slint::Image` | 31 | 16.7 | 16.7 |
| tiny-skia → `slint::Image` (current C++ architecture) | 400 | 64 | 24 |

All test boards (at most 267 items) run at 60 fps with every approach.
Hit-testing (`rstar` + exact `kurbo` shape tests) takes about 4 µs per query on
the stress scene.

## Decision

Use a retained scene in Rust: items with `kurbo` geometry, an `rstar` index,
and vello-encoded pieces per layer, style and tile.

1. Start with `vello_cpu` rendering into a `SharedPixelBuffer` shown as a
   `slint::Image`. It works with every Slint renderer (including software),
   and needs neither vello from git nor matching wgpu versions.
2. Add GPU vello rendering into a wgpu texture on Slint's device
   (`femtovg-wgpu`) once vello is released on the same wgpu version as Slint.
   The 3D board view needs Slint's wgpu integration anyway.

Slint `Path` elements (approach A) are fast enough for small scenes but are not
viable as the single canvas implementation:
- femtovg and vello re-parse every path's SVG text each frame, and skia drops
  its path cache when the viewbox changes;
- the software renderer has no transforms and panics beyond ~32k px paths;
- Slint's vello renderer culls wrongly under `transform-scale`;
- a `Path` has one stroke width per element: no dashes, no per-vertex widths.

## Slint pitfalls found

- Slint's wgpu setup requests WebGL2-level limits, which fail vello's shader
  validation. Request `wgpu::Limits::default()` in the wgpu configuration.
- Slint's vello renderer has no rendering notifier and ignores imported
  textures, so B cannot run on it.
- femtovg-wgpu is on wgpu 30 while released vello 0.10 is on wgpu 29, so B
  needs vello from git for now.
- Skia's `take_snapshot` leaves imported textures black (they display fine).
- `cache-rendering-hint` works only on skia/femtovg and is invalidated by
  panning.

## Running

```sh
cargo build --release
# Windowed (drag pans, wheel zooms, click prints the item):
SLINT_BACKEND=winit-femtovg-wgpu ./target/release/canvas-spike --scene gerber --mode gpu30
# Other modes: transform, viewbox, tiny-skia, vello-cpu, gpu29; tune with --tiles N --threads N.
# Scenes: gerber, stress, or a path to a board.lp.
# On-screen frame times: add --animate 2; snapshot PNGs: --bench --out-dir out/win
# Headless PNG:
./target/release/canvas-spike --scene stress --raster vello-cpu --png out/stress.png
# Check the loader against all test boards:
./target/release/canvas-spike --check-boards
# Full matrix (opens windows): scripts/matrix.sh
```
