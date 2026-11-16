**Observed behavior**
- `design_preview` pin naming is inconsistent: `connections` uses physical pads (`A12`), while `wire_groups.pins` use symbol pin names (`P0.02/AIN0`) and `connect` uses signal names (`U3.EN`, not symbol pin `CTRL`). This cost several failed calls. A per-component pin cross-reference in `component_get`/`library_element` would prevent it.
- Supply symbols rejected `connections:{"NET":"GND"}` (“Signal NET not found”), but work only by forced net name.
- Auto-wiring (`connect`, `max_wire_length`) puts vertical labels on adjacent 2.54 mm pins, producing inevitable overlaps on large QFNs. `schematic_review` reports dozens of overlaps but offers no auto-fix or label fan-out. Explicit `wire_groups` paths require calculating every rotated pin position; the docs give no pin-coordinate offsets for existing symbols.
- `schematic_review` output is verbose JSON with UUIDs, not actionable coordinates; `bounds_mm` is useful but buried.
- `erc_run` returned `outcome: partial` with 0 errors/0 warnings; unclear what “partial” means.
- My wasted calls: rebuilding pages twice because moving symbols does not remove/reflow attached labels; trying `component_remove` on 139 parts individually instead of a bulk filter. No documented way to clear a page or move a group of labels.
- Cloudflare blocked Nordic docs; `r.jina.ai` returned only the cookie page. AXP2101 OCR via `mistral_ocr` worked well.

**Hypotheses**
- Tidy/template layouts could avoid dense-pin label collisions.
- A `page_rebuild` or previewable “auto-arrange block” command would replace manual rebuilds.
- Label placement may be grid-limited rather than collision-aware.

**Beyond MCP**
- Provide verified, license-clean part libraries with datasheet pin roles, not EasyEDA passive imports.
- Require release note/handbook links that are machine-readable without Cloudflare.
- Add CI visual regression or a layout linter with readability rules (label spacing, pin fan-out).
- Document a canonical workflow for large SoCs: split multi-unit symbols per bank, or support symbol variants with fewer pins per unit.