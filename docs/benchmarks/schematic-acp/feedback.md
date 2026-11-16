# Detailed workflow feedback

The following feedback is preserved as returned by the measured model. Some diagnoses are hypotheses or mistaken API interpretations. The results report distinguishes them from confirmed failures.

## baseline/rc-filter-1

**Observed**
- `component_add` placed all five parts exactly; pin coordinates from `library_element` matched `schematic_get`.
- `design_preview` rejected `wire_groups` with “tp1.1 is not connected to IN” even after `add_nets`. It required `pin_nets` first. This dependency is undocumented; the schema reads as if `add_nets` plus `wire_groups` suffices.
- Net labels had to lie exactly on a wire (“Place the net label on a wire at…”). The error was clear, but the constraint is absent from the schema.
- Major workflow hole: `design_apply` only accepts a preview ID from the same session, and batch cannot forward one call’s result into the next. A preview ID in a fresh process returned “Unknown design preview.” Workaround: a persistent HTTP MCP session (not one of the suggested call/batch modes). Batch reference support, or `design_preview(apply=true)`, would fix this.
- Batch emits JSONL, undocumented; `call` accepts `@file`, `batch` does not. Inconsistent.
- `schematic_get` showed `net_segments[].label` as null although labels render. So label text/position cannot be verified programmatically.
- Test-point name/value texts share identical UUIDs across instances; scoping by designator seemed necessary. I did not test `symbol_text_move` on them.
- `component_add` has no `value`, forcing separate `component_update` calls. `design_preview.NewComponent` accepts value/position/etc. and could have replaced seven writes.

**Visual review**
- Wiring, junction dot, title, R1/1k, C1/100nF and IN/OUT/GND labels read clearly; `schematic_review` reported 0 overlaps.
- TP1/TP2 rotated 90°/270° give crowded vertical name/value text. I left it; it is legible but not polished.

**Unnecessary calls:** early schema dumps, `strings` grep, an impossible cross-session `design_apply`, and two redundant renders.

**Beyond MCP**
- sandbox uv cache was read-only; I set `UV_CACHE_DIR` locally.
- The prompt permits only call/batch, but preview→apply needs same-session; this conflict should be resolved.
- An earlier killed HTTP server left a stale project lock, which the next server overrode.
- The HTTP server is still running and holding the lock.

## candidate/divider-1

**Observed**

- Guide naming is inconsistent: docs reference `foilfern://guides/schematic-design`, but the CLI only accepts `guide schematic`; the first attempt errored.
- `batch @file` failed for both relative and absolute paths (“No such file”), although the file existed; piping via `-` worked. Possible CLI sandbox/path bug (hypothesis).
- Batch output is JSONL, while `call` output is one JSON object with `structuredContent` nested under `response`. This is not documented clearly.
- `library_element` for components omits symbol pin coordinates; I had to query `kind:"symbol"` separately to derive rotations and wire endpoints.
- Explicit paths worked as described, and branch-on-path correctly created a junction.
- Repeating `design_preview` immediately before `design_apply` was necessary because each CLI process is a new session. It worked but duplicates computation.
- `schematic_get` pins had empty `name` fields, reducing diagnostic value.
- Visual review was adequate; the full render and a magnified region clearly showed wires, values, and the title. No overlaps were reported.

**Hypotheses / improvements**

- A persistent session option or reusable preview handle would avoid duplicate previews.
- Document the JSONL/response-envelope shapes and `@file` limitations explicitly.
- Include symbol pin positions in component details.
- Clarify whether test-point “value” should be the net name or a measured value.
- The design document schema is large; a compact worked example for vertical passives would help.
- Keep the useful image-region and close-up rendering workflow.

## polished/divider-1

**Observed**

- The workflow was reliable: one `design_preview` document created parts, nets, fields, notes and explicit wiring; `design_apply`, `erc_run`, `schematic_review`, `netlist`, `render` and `project_save` all succeeded. Netlist matched the requested connections exactly; ERC was clean.
- `library_element` with `kind:"symbol"` rejected the provided `symbol_variant` UUID. The workable path was: query the component, read its gate’s `symbol.uuid`, then query that. The catalog does not state this distinction clearly.
- `design_preview` paths using pin names (`"r1.1"`) worked, but the rule “must include every selected pin” is ambiguous when paths end at raw `{x,y}` coordinates instead. I avoided experimenting.
- Field `offset` behavior on a 270°-rotated resistor was unclear: expected value/name spacing did not obviously match the rendered distance. **Hypothesis:** text anchor/baseline rules differ from symbol-origin offset semantics.
- Full-page renders are too zoomed out for reliable label review. `image_region` close-up solved this. `schematic_review` reported no overlaps but cannot judge whether a value visually belongs to the right symbol.
- Revision reporting appeared session-relative: after a saved revision-39 edit, a new session reported revision 0. **Hypothesis:** each CLI load starts a fresh local revision counter, not a project edit count.

**Improvements beyond MCP**

- Provide a prescribed zoomed review crop or automated focus on the circuit block.
- Require/expose text ink bounding boxes in `schematic_get`, so spacing and association can be checked numerically.
- Clarify symbol-UUID lookup and path endorsement rules in the guide/schema.
- The task could explicitly permit TP values as net-name evidence, avoiding ambiguity about whether separate net labels are expected.

## polished/divider-2

**Observed**

- `guide schematic` and `guide rc-lowpass` were essential; the design_preview schema (800+ lines) was unwieldy, and `$defs` for NewComponent, LocalWireGroup, InitialField and PageWaypoint had to be extracted via Python.
- A path vertex exactly on a pin coordinates was rejected: "Use the pin name at (100, 140.08) mm". Reasonable, but the guide never warns that pin-coincident vertices must be names; my first preview failed on this, costing a full call.
- `library_element` cannot resolve symbol-variant UUIDs (only components/symbols), and with no workspace open symbol geometry wasn't queryable by UUID. I read rotation semantics and test-point pin orientation from the project's `.lp` files on disk. Pin-number-to-angle mapping (resistor pin 1 = top at rotation 270, test point pin pointing down at rotation 0) was only derivable from those files, not from tool output.
- **Hypothesis:** rotation conventions and symbol pin positions could be exposed via `library_element` on variant UUIDs, or `schematic_get` after placement.
- `T` labels posiển stream was missed: `library_element` refuses variant UUIDs.
- Vertical net labels are auto-rotated and offset ~0.5 mm left of the wire. Fine, but `label` rotation/height is not configurable, and labels must sit exactly on a path (no way to nudge them away from text).
- `schematic_get` symbol entries had `designator: None`, so symbols can't be identified by designator there (component_list works instead).
- `server_info` reported `revision: 0` after saving (saved_revision was 40), which is inconsistent/confusing as a convention. Also `server_info` says `workspace: none`, yet project-library lookups worked through `--project`.
- `schematic_get` net_segments showed `labels` with empty text (`''`), making label verification depend on the render.

**Unnecessary/extra calls:** one preview killed by the pin-vertex error, one extra preview with labels (justified), and a requirements/component_list check after saving.

**Limitations:** no automatic wire routing for exact pin-to-wire geometry; explicit paths are the only route, and a preview's image is only reviewable once per session, so every adjustment needs a re-preview. Page-view zoom/region review worked well.

**Beyond the MCP:** documentation should show a pin-name reference table and rotation convention for common symbols, and a preview should be inspectable in a re-run session (or be persisted), avoiding repeated full-document previews for visual tweaks. Field/label placement tools worked; a "move label" variant would reduce render-guesswork.

## polished/rc-filter-1

**Observed issues**
- `guide rc-lowpass` shows `wire_groups` with only `net`+`pins`, but `design_preview` rejected paths with `"Explicit wire paths require at least one pin."` although the schema says pins "can infer pins from their string vertices". Needed a retry cycle.
- The schema is split: `WireVertex`, `DesignNote`, `NewComponent` were not resolvable in my grep (`null` lookups), so I read `candidate-tools.json` piecemeal. Nested `$defs` are large; a minimal example per tool would help.
- Field `offset` alignment is undocumented (centered vs left-anchored). I copied guide offsets and guessed TP/R values; results were fine but unverifiable except by rendering.
- Net-label orientation is only implied: the GND label on a vertical wire rendered rotated 90°, readable but unexpected; no rotation option exists.
- Preview and render images are visually identical, so the preview PNG is the only pre-apply evidence (acceptable).
- `history` in a fresh session shows no entries, and `project_summary` reports revision 0 after a saved rev-39 edit — confusing, since undo history is session-local.
- Tiny inefficiency: a failed preview consumed a turn; the batch could report recoverable validation errors earlier.

**Hypotheses**
- The pins requirement is probably intentional (path vertices alone can't distinguish two pins on the same point) but contradicts its own description; fix the text or infer pins.
- Value text detachment on test points is likely default alignment; a documented `alignment`/`justify` field would let agents place fields precisely.

**Beyond MCP**
Agent-side: pre-flight one tiny preview before the full document. UI-side: selection overlay of pin/field bounding boxes would make visual review — and stale-revision messaging — much clearer.

## polished/rc-filter-2

**Observed issues**

- **Explicit path rule unclear.** Defining a waypoint exactly at a pin (`{"x":58.42,"y":50.8}` for TP2.1) was rejected: “Use the pin name at …”. The schema only says paths must include every pin; it does not say coincident pin waypoints are forbidden. I removed the waypoint and terminated with the pin name. Same for C1.1.
- **Label limits.** `wire_groups.label` accepts only a position. Orientation automatically follows the wire, so GND became vertical. There is no per-label rotation, height, justification or offset control. I accepted it as legible, but cannot make all labels horizontal.
- **Revision inconsistency.** `design_apply` returned revision 39; a later CLI session reported revision 0/saved 0 with `unsaved_changes:false`. *Hypothesis:* revisions are session-local or reset when reloading the saved file, which risks confusing clients using `expected_revision`.
- **Duplication ambiguity.** TP values “IN/OUT/GND” plus net labels of the same names appear twice. The guide does not advise whether test-point values should repeat net names. It is readable but visually redundant.
- **Visual review.** Full-page render was clear and useful. There is no automated check for duplicated net-label/value text; `schematic_review` only reports overlaps. The image tooling itself worked.

**Unnecessary calls/workarounds**

No genuinely unnecessary calls; three preview attempts were needed to discover path rules. Better schema examples would avoid those.

**Beyond MCP**

Clarify path/pin coincidence and label orientation in the guide; document revision semantics; add label rotation or text controls; let visual checks flag redundant annotations; state test-point value expectations in the task fixture.

## polished/dual-rc-1

**Observed behaviors / workarounds**
- Pin geometry is unavailable before placing parts: `library_element` with symbol UUIDs failed (`not_found`, "only the project library was searched", even though the components reference those variants). I had to place symbols first, then read pins via `schematic_get`. The docs imply symbol inspection before layout.
- `wire_groups.paths` reject vertices that coincide with pins ("Use the pin name at (17.78, 71.12)"). Undocumented in the guide; the error message rescued me but cost a turn.
- `label` must sit exactly on a wire ("Place the net label on a wire at …"). Not stated in the schema. Labels on vertical wires render rotated 90°, which forced me to invent horizontal ground stubs.
- No way to delete or re-route an existing wire except `disconnect` (which also drops the net connection) or re-supplying a `wire_group`, which silently replaces matching segments (`replaced_segments` in the diff). Re-layout required removing and re-adding all nine components.
- Field text alignment for negative offsets (right-align) was inferred from renders; schema only says "offset", not anchoring.
- `server_info` reports project revision 0/unsaved=false while `project_summary` reports the true saved revision 2 — inconsistent.

**Hypothesis (unverified)**
- Duplicate text UUIDs shared between C1/C2 (same field UUIDs) likely make `text_positions` affect both symbols; I avoided it and used `symbol_text_move` instead.

**Unnecessary calls**
- Initial placement-only apply (two-step: place, read pins, wire).
- A failed preview for labels plus a failed preview for paths.
- Re-rendering Channel B after a one-segment fix.

**Beyond MCP**
- The guide should document label/path validity rules and pin-relative vertices (`RelativeCoordinate` exists but is unexplained).
- Ship a way to edit/remove individual wires.
- Library search of project symbols should work by UUID.
- Provide a "rest of sheet empty" hint or auto-fit/crop in render; my zoomed crops clipped the "Channel A/B" titles because region bounds ignore text height.

## polished/dual-rc-2

**Observed**

- Batch `$ref` semantics were unclear. Using `{"$ref":"design.json"}` for the first call failed with “`$ref` must be INDEX#JSON_POINTER”. Workaround: inline the whole document with Python. The guide documents refs only to prior results.
- `library_element` with a symbol-variant UUID returned `not_found`; I had to query the component, extract its embedded symbol UUID, then query the symbol. This two-step lookup is undocumented.
- Preview IDs are session-local, so inspecting the preview image and then applying required two previews: one preview-only session, then preview+apply in a fresh session. This duplicated work.
- A single-pin GND wire group with an explicit path plus `label` worked and produced a correct labeled stub; ERC showed 0 errors and the netlist merged C1.2, C2.2 and TP5.1.
- Automatic wiring added vertical 90° net labels (`OUT_A`, `GND`) on vertical wires. They were readable, but the schema does not explain when labels are auto-added or how to suppress them.
- `render` produced 1600×1200 images fitted to content, not the full A4 page; this was not documented. Full-page and `image_region` close-ups were both useful for checking text/wire clearance.

**Hypotheses**

- A design-document file argument for `design_preview`/`design_apply`, or persistable preview IDs, would remove the duplicate-preview workflow.
- `library_element` should resolve symbol-variant UUIDs directly.
- Guides should state batch-ref rules, label behavior, and render fit/crop semantics.

No unresolved design issues.
