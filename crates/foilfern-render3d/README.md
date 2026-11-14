# foilfern-render3d

Interactive physical board rendering with Bevy 0.20. Core stays independent of
the renderer: `BoardSnapshot` owns the shared `BoardPhysicalGeometry` and STEP
model bytes; capture it while reading the project, then release the project
lock before meshing and rendering on a worker thread.

`BoardMesh` triangulates material regions with earcutr and extrudes the canonical
outline/cutouts/drills. Via drills use their resolved layer spans. Since the file
format does not specify dielectric thicknesses, inner layers are evenly spaced
through the board thickness. Copper, solder resist, silkscreen, solder paste and
STEP component bodies have independent material groups. Component placement,
tessellation, normals and surface appearance come from `foilfern-step`; no OBJ
text conversion or second STEP importer is involved. A failing component model
is reported while retaining the PCB and other components.

`Renderer` creates a headless Bevy app with an offscreen sRGB target, PBR
materials, two directional lights and an orbit camera. It does not create a
window or take over the event loop. The current Slint integration reads the
GPU frame back into an RGBA image, so the UI can use any Slint renderer,
including its headless software platform. This incurs a GPU-to-CPU transfer;
it is not the shared-texture zero-copy path of Slint's Bevy example. GPU work
is performed only for requested frames, rather than continuously while idle.

The viewer is distinct from mechanical STEP export: it includes via drills and
thin material surfaces. Layer thicknesses, plated barrel thickness, solder
menisci and full manufacturing stackups are not modelled. Internal slab faces
at blind/buried drill depths remain hidden inside the opaque substrate.

An offscreen GPU is sufficient for headless validation; a window server is not
required. A host without a compatible Vulkan/Metal/DX12 adapter still runs all
CPU geometry tests, but cannot render a 3D frame.

```sh
cargo test -p foilfern-render3d
cargo test -p foilfern-render3d -- --ignored --nocapture
cargo run -p foilfern-render3d --example render_board -- /path/to/project.lppz /path/to/top.png
cargo run -p foilfern-render3d --example render_board -- /path/to/project.lppz /path/to/bottom.png bottom
```

The application opens the view through the existing `3D` button or
`--tab board3d`. Drag with the left button to orbit, with the middle/right
button or Shift+left to pan, and scroll to zoom. `1`/`2` select the top/bottom,
`0`/`F` fit the board, and `+`/`-` zoom. Toolbar sliders control the material
groups, and the `2D` button returns to the layout editor. Project changes,
including MCP edits and undo/redo, replace the scene snapshot while retaining
the camera. Closing the tab disconnects its worker.
