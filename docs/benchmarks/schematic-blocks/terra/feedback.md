**Observed:** The bounded block workflow produced readable, separated banks with continuous local rails. Both creation documents replayed with empty diffs, and the saved netlist matched the requested memberships. Automatic wiring placed the pull-up signal labels vertically; the name and value fields remained horizontal.

The preview correctly reported three ERC warnings for the single-pin SDA, SCL and IRQ nets. Those warnings are expected for this deliberately incomplete circuit, but the tool could make that context clearer without implying a connection was missed.

**Session and review friction:**

- Parallel CLI invocations against the same workspace and project emitted lock-file removal errors. Subsequent sequential calls worked. The CLI documentation should explicitly warn that local calls share locks and should be serialized.
- The guide requires preview and apply in one batch because preview IDs are session-local. Inspecting before applying therefore required a separate preview followed by a repeated preview in the apply batch. A persistent session or a documented review-and-apply mechanism would remove that duplication.
- The full-page render displayed poorly through one image-viewer call, although its PNG was byte-identical to the readable preview already inspected. That points to a visual-review display issue; I cannot determine its cause from this run.
- The preview image cropped the bottom of the signal labels close to the frame. A configurable margin or automatic fit around text would make review more reliable.
- The selected tool schema was long enough to be truncated in the terminal output. Smaller schema queries or examples focused on block creation would help.

**Beyond MCP:** The task instructions could state that one-pin signal-net warnings are expected and that signal-label orientation may differ from field orientation. The native editor and rendering pipeline should also provide predictable image margins and consistent display at readable scale.