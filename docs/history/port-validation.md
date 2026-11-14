# Historical port validation

These records were retained from `docs/status.md` at `f3c3fda`, whose full-port
summary was dated 2026-11-10. They describe earlier implementation snapshots;
they are not current test totals or new runs made during documentation upkeep.
Current capabilities and delivery priorities are in [status](../status.md) and
[roadmap](../roadmap.md).

## Graphics, STEP and MCP merge before the board 3D viewer

The recorded validation reported 1,499 Rust tests passing across the full
workspace run and focused graphics/i18n reruns after two merge fixes; eight
were ignored. It included 53 headless application scenarios and the stdio
STEP/PDF regression. The unmodified upstream CLI suite reported 177 passed
and one failed snapshot expecting Qt/OpenCascade version lines, intentionally
different in the Rust implementation. Formatting, workspace Clippy with warnings
denied, and production CLI/MCP/application builds passed in that run.

The regenerated CAN–USB mechanical assembly was independently checked with
OCCT/XCAF: 87 valid solids and 2,048 face appearances were retained. Realistic
board PNGs were inspected. Logs/artifacts were recorded at the host-local path
`/home/okhsunrog/tmp_zfs/codex_tmp/foilfern-mcp-integration/`; that path is not a
portable fixture or a promise that the files remain available elsewhere.

## Interactive board 3D viewer

The subsequent record reported 1,503 tests passing, ten ignored by default,
across the application run and a separate remaining-workspace run. The initial
overall shell was interrupted by SIGTERM; the separate Cargo run completed
with status 0. Formatting, workspace Clippy and the application build with
`ui-debug-mcp` passed. Both GPU tests passed when explicitly enabled on the
Intel Arc (MTL), Mesa Vulkan host.

The GPU tests covered both sides, camera-only updates and unaligned viewport
resizing; the app scenario included navigation, material sliders, a command-bus
color edit, undo and closing the 3D view. The actual application accepted all
40 recorded pointer/wheel events for CAN–USB orbit/zoom, bottom view and return
to 2D. The board had 44,031 render triangles in 115 material groups. Dark/light
screenshots were inspected. Artifacts were recorded under the host-local path
`/home/okhsunrog/tmp_zfs/codex_tmp/librepcb-bevy-3d/`.

This establishes the recorded GPU/UI scenarios, not general Wayland/X11,
clipboard, native-dialog, DPI or every Slint GPU-backend acceptance.

## Earlier MCP end-to-end acceptance

A historical stdio session built a two-transistor LED multivibrator using the
official libraries: library installation/search, schematic wiring and tidy,
board outline and placement, Freerouting, a GND plane, checks and Gerber export.
The official CLI reported ERC 0 and DRC 0. This was an earlier branch acceptance,
not a substitute for the later routing-candidate regressions.

## Later board evidence

The [routing reliability record](../mcp-routing-reliability.md) retains the
STM32 and ESP32 cases with their individual settings and limits. The
[ESP-PD-Sink index](../benchmarks/esp-pd-sink/HANDOFF.md) distinguishes the frozen
performance fixture from the selected layout. Use those manifests/reports for
replays rather than inferring an input revision from the historical test totals.
