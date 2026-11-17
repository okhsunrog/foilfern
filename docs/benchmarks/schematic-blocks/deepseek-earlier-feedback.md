**Observed issues**

- `foilfern-mcp tools design_preview` ignored the filter and printed the entire catalog; I parsed `tools.json` instead.
- `library_element` with the supplied `symbol_variant` UUID returned `not_found`. The variant is not a symbol UUID; pin geometry required first reading the component and then its gate symbol UUID. The part-identity block should include symbol UUIDs or pin coordinates.
- Automatic `wire_groups` failed with “No clear route for wire group on VCC” even after increasing block gap to 15.24 mm. I had to switch to explicit relative-coordinate paths, contrary to the task’s “draw automatically” instruction.
- Those explicit rails at pin height overlapped every name/value field (8 overlaps). Changing `InitialField.offset.y` and replaying `add_components` did not move the existing text, though the block did reposition symbols. Replay is therefore not idempotent for field styling, and there is no clean way to restyle fields on existing components except per-text `symbol_text_move`.
- `design_preview` returned `partial` with overlaps but `design_apply` still applied them. This is risky; blocking or requiring explicit override would be clearer.
- Block spacing was unexpectedly large (27.94 mm with a 15.24 mm gap), and replay re-packed positions.
- A batch aborted at the first error, skipping later calls.
- Full-page renders make small passives hard to inspect; `image_region` is essential but costs another call.

**Hypotheses**

- The auto-router treats annotation bounds as hard obstacles and cannot add vertical stubs above/below a row, so aligned rails are impossible automatically.
- Field offsets may be applied only at creation, not on replay.

**Improvements beyond MCP**

Provide symbol UUIDs/pin geometry and page size in the fixture; clarify replay semantics; allow batch continue-on-error; extend the 10-minute deadline; and add a first-class existing-component field-style edit.