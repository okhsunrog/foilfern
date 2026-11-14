# Agent workflow improvements

Created 2026-11-14 from agent feedback. This is a scoped implementation queue
within P0–P6 of [the roadmap](roadmap.md), not a replacement for that roadmap.
Only completed, validated and pushed changes count as delivered. Real-board
measurements use release binaries and record source identity. Keep the existing
preview/apply revision checks, undo and native LibrePCB interoperability.

## Delivery queue

| Order | Scope | Acceptance | State |
| --- | --- | --- | --- |
| A1 | Safe library search | Warn about ambiguous 0201/0402/0603 even with limit 1; explain that whole is textual, not dimensional. Later rank by verified dimensions and explicit unit convention, not name guesses. | Warning implemented; focused integration test and Clippy passed; dimension-based ranking remains planned |
| A2 | Part identity and BOM | Declarative components accept manufacturer/MPN, supplier identity and attributes through editor mutations; preserve identities on reconciliation, undo and save/reopen. BOM without an explicit board selects the first board consistently; schematic-only projects still work. Test package, MPN and supplier fields end to end. | Implemented: board-default BOM and native instance part identity; stdio replay/save/reopen covered |
| A3 | Resolve devices and pins | Device alone resolves its library component. Exact MPN/name lookup returns candidates and rejects ambiguous selections. One detail response exposes signals, named pads, footprint dimensions and model availability. UUIDs remain the stable persisted identity. | Planned |
| A4 | Import a concrete purchasable part | LCSC ID or exact MPN produces a reviewed native component/device/package/symbol bundle with pin mapping, MPN, supplier ID and available 3D assets. See the import stages below. | Priority after A2/A3 |
| A5 | Partial library install and official KiCad discovery | One unresolved library does not discard successful independent requests; report per-item results and dependency failures. Discover official KiCad libraries explicitly and import a selected pinned source. | Planned |
| A6 | Board placement review | Board-layout preview returns an image plus absolute pad positions after rotation/mirroring. Preview performs no mutation and preserves stale-source refusal. Connector orientation comes from explicit metadata or geometry with confidence, never a guessed direction. | Planned |
| A7 | Legend editing | Identify text by device instance plus footprint-text UUID; report transformed bbox. Bounded text placement tests mask/courtyard constraints and reports fresh DRC delta with undo. | Planned |
| A8 | Output access and plane defaults | Create/update validated output jobs through MCP; fresh projects can export STEP and graphics. Default plane outline uses the actual board contour; test nonrectangular boards and holes. | Planned |
| A9 | Response and tool discovery cost | Measure current tools/list and representative preview/apply payloads. Add compact change summaries with detail retrieval. Investigate opt-in tool groups and client compatibility; preserve default discoverability. Budget tests catch payload regressions. | Planned; extend existing P0 |
| A10 | Rules with diagnostic provenance | Identify footprint-internal versus placement-induced DRC findings. Versioned factory profiles record source/date and require explicit selection; no silent relaxation to pass a footprint. | Planned |
| A11 | Reproducible design as code | CLI tool/batch execution reuses the same tool implementations and session. Commit a design document; CI checks idempotence, semantic diff, ERC/DRC, exports and upstream reopen. Reports have bounded output and deterministic failure status. | Local call/batch implemented; CI design-document workflow remains planned |
| A12 | Rich review and electrical intent | Expose existing offscreen 3D through render. Group schematic layout by connectivity/function. Add opt-in power-source and decoupling constraints with explicit pin roles, thresholds and evidence; report unknown intent rather than invent electrical guarantees. | Planned; extend P2/P5 |

## Concrete-part import stages (A4)

1. Trace upstream tscircuit CLI import, EasyEDA conversion and JLC search code.
   Record repository revisions, licenses, endpoints, coordinate/unit conventions,
   pin-to-pad mapping, multi-unit/duplicate pins and 3D transforms. Reuse maintained
   parsers/converters when suitable; search crates.io before new parser code.
   References: https://github.com/tscircuit/tscircuit,
   https://github.com/tscircuit/jlcsearch,
   https://docs.tscircuit.com/guides/importing-modules-and-chips/importing-from-jlcpcb.
2. Define a provider-neutral result with manufacturer/MPN, supplier IDs, datasheet,
   source identity, symbol signals, footprint pad mapping and optional models.
   Keep network fetching in network/import callers; keep core synchronous.
   Cache versioned source artifacts and support offline replay. Missing models or
   uncertain mapping must be explicit; never fabricate a pin mapping.
3. Implement exact LCSC-ID resolution first, then exact MPN search with ambiguity
   handling. Fetch source assets with limits, cancellation and per-asset errors.
4. Convert reviewed input to native library objects with deterministic identities.
   Import repeated sources idempotently; detect changed upstream assets rather
   than silently replacing edited objects. Validate pad/signal completeness,
   layers, geometry units and 3D translation/rotation.
5. Expose one acquisition flow: resolve -> preview bundle -> install/use. Carry
   selected part identity into the design document and manufacturing BOM.
6. Acceptance corpus: CH340, CP2102, STM32F103C8T6, USB-C connector, 0603 LED,
   resistor and capacitor. Pin supplier IDs before testing. Include missing 3D,
   ambiguous MPN, duplicate pin names, network failure and repeated import.
   Inspect symbol/footprint/model previews and reopen native output in upstream;
   tests and a plausible image alone do not prove datasheet correctness.

## Evidence and continuation

The review is a report of a particular tool build. Recheck each complaint against
current source before implementing: some compact responses, bounded placement,
3D output and declarative reconciliation already exist. A2 starts with a trace
from NewComponent to circuit/device part data and bom_generator; A4 research
runs before choosing its provider or intermediate format.

### Initial source trace

Inspected tscircuit/cli at `f6b1fa43ba2f5682867dcfa1d656a08e576e1c08`
(MIT; preserve notices if code is reused). Entry points:
`cli/import/import-jlcpcb-part.ts` and
`lib/import/import-component-from-jlcpcb.ts`. The latter delegates fetching and
conversion to the `easyeda` package, validates the raw schema, takes the MPN from
`dataStr.head.c_para["Manufacturer Part"]`, optionally fetches datasheet metadata,
and converts to circuit JSON and TSX. Exact footprint retention is an option;
the default attempts conversion to a footprinter. Model transforms/URLs come
from the converted CAD component, with optional local asset download. Missing
3D is explicitly reported by the CLI. This is a source trace, not a successful
live import or validation of pin correctness. Inspect `easyeda` source and its
license next; copying the CLI wrapper alone would not implement conversion.

### Delivered BOM substep (A2)

`export_bom` selects the first board by default, falling back to generic BOM for
boardless projects. `schematic_only` explicitly requests the generic export and
is mutually exclusive with `board`. The response identifies the selected board.
Integration coverage compares implicit and explicit board selection and checks
schematic mode and argument conflicts. All 85 MCP integration tests and focused
MCP Clippy passed. This does not yet implement declarative MPN/supplier selection.

### Delivered part identity substep (A2)

New declarative components accept `part: {mpn, manufacturer, attributes}` and
component `attributes`. The selected part is stored in the instance's native
assembly option, not in a shared library device. Replay preserves identity and
manual placement and rejects conflicting part/attribute selections. `component_get`
returns assembly options. `export_bom.additional_attributes` adds native attribute
columns such as `LCSC[]`. The stdio creation/replay test covers invalid input,
preview isolation, conflict, undo/redo, board BOM and native save/reopen.
Focused integration and all-target MCP Clippy passed.

Priority update: direct CLI call/batch execution (A11) moves before the remaining
A3/A4 work, as requested on 2026-11-14.

### Delivered local execution substep (A11, prioritized)

`foilfern-mcp call TOOL JSON` and `batch FILE` invoke the shared tool registry
without an MCP client or protocol handshake. File/stdin input, JSON-lines output,
prior-result references, stop/continue-on-error, explicit save and nonzero failure
status are implemented. Unsaved edits are diagnosed and never silently treated
as persisted. Batch operations retain their individual transaction boundaries;
this is not batch-level rollback. Four process-level tests cover file/stdin,
validation, references, session reuse, persisted reopen and failure/save behavior.
All 89 MCP integration tests and all-target MCP Clippy passed. The CLI entry point
is complete; the broader design-document CI workflow in A11 is still pending.

The MCP server Rust type is now `FoilfernMcp`; the old type name was removed.

### Delivered device-only creation substep (A3)

A declarative new component can specify only its device UUID. The referenced
library component is inferred and imported through the same transaction; an
explicit incompatible library_component is rejected. The existing stdio creation
acceptance now exercises device-only creation, rollback, undo/redo, persisted
reopen and no-op replay, and rejects conflicting explicit component references.

### Delivered exact resolution and model metadata (A3)

`library_search exact:true` returns full details for a unique exact name/MPN/UUID,
rejects ambiguity even at limit 1, and includes matching parts. Device details now
report model file existence and footprint model transforms. A fixture-based test
covers MPN resolution, no fuzzy fallback, duplicate names and UUID disambiguation.
All 90 MCP integration tests and all-target MCP Clippy passed.

### Import feasibility evidence (A4)

The EasyEDA product endpoint returned C8734 successfully on 2026-11-14, including
STM32F103C8T6, supplier/manufacturer fields, symbol and package source data. This
is fetched-source evidence only, not a converted native library or pin audit.
Crates evaluated: easyeda2kicad-rs 1.1.2 (MIT, library API), npnp 3.1.0
(non-commercial restriction; not selected), pcb-jlcpcb 0.1.3 (MIT, binary-only).
The MIT converter silently defaults several malformed numeric fields and drops
some primitives, so its output needs strict source/mapping/geometry validation
before any import path is accepted. No converter dependency has been added yet.
