# Slint development and runtime checks

The workspace and both standalone prototypes pin Slint to master commit
[`aebe3dd580ce0594de272bc5797b7444a2aa18fd`](https://github.com/slint-ui/slint/commit/aebe3dd580ce0594de272bc5797b7444a2aa18fd)
(2026-11-08). Its Cargo package version is 1.20.0, a development snapshot.
UI builds select edition 1.20. Use this revision's source and documentation
for signatures; the connected documentation MCP currently indexes released
Slint through 1.18.1. Installed editor/viewer binaries may also be older.

## Check the actual Rust canvas demo

The embedded runtime MCP server can inspect and operate the running Rust
application, including its canvas callbacks. Compile it as a development
option; `mcp` is not a default dependency feature:

```sh
SLINT_EMIT_DEBUG_INFO=1 cargo build -p foilfern-canvas \
    --example canvas_demo --features slint,slint/mcp
uv run --no-project python scripts/check-slint-mcp.py \
    --binary target/debug/examples/canvas_demo \
    --artifacts /path/to/artifacts/slint-mcp
```

On NixOS, build in a development shell providing `pkg-config`, `fontconfig`,
`freetype`, `libxkbcommon`, `wayland`, OpenGL libraries, OpenSSL, udev and
D-Bus. Libraries loaded dynamically also need to be discoverable at runtime.

The script launches its own disposable demo scene with the software renderer,
without X11/Wayland. It chooses an available loopback port, reads the runtime
tool schemas and element tree, then checks zoom, selection and deletion
through UI input and accessible status read-backs. It saves PNGs for zoom,
mirror, selection and deletion, together with the event log and state summary.
Look at those images before declaring visual acceptance. The child process
is stopped on success or failure. Avoid rebuilding the binary during a run.

The 2026-11-08 run exposed 18 tools. Zoom changed from 10.67 to 13.87 px/mm;
a rectangle selected nine items and Delete reduced the scene from 46 to 37.
The event log recorded 146 events without drops or unknown variants. These
checks and screenshots cover the native Rust application with the headless
software backend; desktop GPU/window-system behavior needs its own run.

For an interactive MCP client, start the demo with a fixed port instead:

```sh
SLINT_EMIT_DEBUG_INFO=1 SLINT_MCP_PORT=9315 SLINT_BACKEND=headless-software \
    cargo run -p foilfern-canvas --example canvas_demo \
    --features slint,slint/mcp -- --no-stress
```

Connect with Streamable HTTP at `http://127.0.0.1:9315/mcp`. The server's
`initialize` response documents handles, query syntax and enum values.

## Useful tools

| Capability | Runtime tools | Use here |
|---|---|---|
| Discover windows and controls | `list_windows`, `get_window_properties`, `get_element_tree`, `get_element_properties` | Inspect geometry, accessible labels and current state |
| Find specific controls | `find_elements_by_id`, `query_element_descendants` | Target stable qualified IDs such as `DemoWindow::status-text` |
| Verify rendering | `take_screenshot` | Inspect real Rust-rendered frames after input |
| Pointer input | `click_element`, `drag_element`, `hover_element`, `move_pointer`, `scroll_element`, `dispatch_pointer_scroll` | Exercise canvas hit testing, selection, panning and zoom |
| Keyboard and semantic input | `dispatch_key_event`, `invoke_accessibility_action`, `set_element_value` | Shortcuts, buttons, inputs and sliders |
| Observe delivery | `start_event_recording`, `stop_event_recording` | Check events reached the application and were accepted/rejected |

Canvas primitives are drawn into a Slint `Image`; they are not separate Slint
elements. UI input exercises the canvas's own hit testing. Model entities,
UUIDs, undoable project edits and ERC/DRC will be exposed through the project's
separate MCP server described in [mcp-research-konnect.md](mcp-research-konnect.md).

## Other upstream tooling

- **Documentation MCP:** `search` and `fetch` find API documentation. Check the
  returned release before applying an example to a development snapshot.
- **Preview MCP:** the upstream plugin includes `preview_slint`,
  `get_preview_screenshot` and file/resource helpers. It can preview saved
  `.slint` files with imports through its Wasm interpreter. Previewing a file
  does not execute the Rust callbacks used by our canvas.
- **`@testable`:** an experimental property annotation exposes typed values
  through runtime MCP and testing APIs, including otherwise private values.
  It needs the named `testable` experimental feature and debug info. We have
  not enabled it globally; the current check uses accessible status text.
- **System testing:** Slint provides a separate testing transport and the
  `slint_testing` Python client for automated UI tests.
- **LSP and viewer:** a matching `slint-lsp` gives editor diagnostics, while
  `slint-viewer --check` and `--screenshot` give fast source checks and renders.

These additional tools are available upstream; only documentation MCP and
the embedded runtime MCP were exercised in this update.

Upstream references at the pinned revision:
[runtime MCP architecture](https://github.com/slint-ui/slint/blob/aebe3dd580ce0594de272bc5797b7444a2aa18fd/docs/development/mcp-server.md),
[runtime tool definitions](https://github.com/slint-ui/slint/blob/aebe3dd580ce0594de272bc5797b7444a2aa18fd/internal/backends/testing/mcp_server.rs),
[`@testable`](https://github.com/slint-ui/slint/blob/aebe3dd580ce0594de272bc5797b7444a2aa18fd/docs/astro/src/content/docs/guide/experimental/testable.mdx),
[preview MCP](https://github.com/slint-ui/slint/blob/aebe3dd580ce0594de272bc5797b7444a2aa18fd/ai-plugins/mcp/slint-preview/server.mjs).
