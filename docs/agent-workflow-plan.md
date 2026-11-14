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
| A2 | Part identity and BOM | Declarative components accept manufacturer/MPN, supplier identity and attributes through editor mutations; preserve identities on reconciliation, undo and save/reopen. BOM without an explicit board selects the first board consistently; schematic-only projects still work. Test package, MPN and supplier fields end to end. | Planned |
| A3 | Resolve devices and pins | Device alone resolves its library component. Exact MPN/name lookup returns candidates and rejects ambiguous selections. One detail response exposes signals, named pads, footprint dimensions and model availability. UUIDs remain the stable persisted identity. | Planned |
| A4 | Import a concrete purchasable part | LCSC ID or exact MPN produces a reviewed native component/device/package/symbol bundle with pin mapping, MPN, supplier ID and available 3D assets. See the import stages below. | Priority after A2/A3 |
| A5 | Partial library install and official KiCad discovery | One unresolved library does not discard successful independent requests; report per-item results and dependency failures. Discover official KiCad libraries explicitly and import a selected pinned source. | Planned |
| A6 | Board placement review | Board-layout preview returns an image plus absolute pad positions after rotation/mirroring. Preview performs no mutation and preserves stale-source refusal. Connector orientation comes from explicit metadata or geometry with confidence, never a guessed direction. | Planned |
| A7 | Legend editing | Identify text by device instance plus footprint-text UUID; report transformed bbox. Bounded text placement tests mask/courtyard constraints and reports fresh DRC delta with undo. | Planned |
| A8 | Output access and plane defaults | Create/update validated output jobs through MCP; fresh projects can export STEP and graphics. Default plane outline uses the actual board contour; test nonrectangular boards and holes. | Planned |
| A9 | Response and tool discovery cost | Measure current tools/list and representative preview/apply payloads. Add compact change summaries with detail retrieval. Investigate opt-in tool groups and client compatibility; preserve default discoverability. Budget tests catch payload regressions. | Planned; extend existing P0 |
| A10 | Rules with diagnostic provenance | Identify footprint-internal versus placement-induced DRC findings. Versioned factory profiles record source/date and require explicit selection; no silent relaxation to pass a footprint. | Planned |
| A11 | Reproducible design as code | CLI tool/batch execution reuses the same tool implementations and session. Commit a design document; CI checks idempotence, semantic diff, ERC/DRC, exports and upstream reopen. Reports have bounded output and deterministic failure status. | Planned; extend P1/P6 |
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
