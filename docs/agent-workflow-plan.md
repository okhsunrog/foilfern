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
| A3 | Resolve devices and pins | Device alone resolves its library component. Exact MPN/name lookup returns candidates and rejects ambiguous selections. One detail response exposes signals, named pads, footprint dimensions and model availability. UUIDs remain the stable persisted identity. | Implemented: device-only creation, exact lookup and complete device/model details |
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

### Delivered source acquisition foundation (A4)

`foilfern_network::part_source::EasyEdaProvider` acquires exact LCSC IDs, validates
provider identity/required source fields and retains original response snapshots.
The cache is content-addressed by SHA-256, supports offline replay and refresh,
retains previous versions and detects tampering. Acquisition has a 30 s request
timeout, 32 MiB response limit and cancellation; it does not modify native libraries.
Provider provenance/attribution and original license metadata remain available.
Network tests: 38 passed, 3 existing live tests ignored; focused cache/identity
checks passed. The explicit live C8734 acquisition test also passed and returned
STM32F103C8T6 (source SHA-256
`a99b0a4abd493f08c20aa320d0332f6a14e50726b36d501e6268d1e1e68614c7`).
This is source acquisition only; native geometry conversion and MCP acquisition
remain pending. tscircuit/easyeda-converter source inspected at
`9d377ea2d590de8d823718c50929ef7b2441a296` (MIT), including 3D origin/bounds helpers.

### Delivered native conversion foundation (A4)

`foilfern_import::easyeda::convert` creates native symbol/component/package/device
objects using strict source parsing plus the existing KiCad-to-native converter.
Source pin numbers must match electrical package pads; malformed/unsupported
primitives fail explicitly. Coordinates are not grid-snapped; native UUIDs are
reproducible. Per-pad mask/paste expansions, native parts and original source JSON
are retained. All four elements install in one transaction; existing directories
are refused to protect manual edits. The supported initial primitive set is
explicit in the adapter; multi-unit symbols, custom/non-cardinal pads and some
mechanical primitives remain unsupported. 3D projection artwork is retained in
provenance with warnings, not fabricated as copper. STEP attachment is next.

54 importer integration tests passed (one external-source test ignored by default),
plus all-target importer Clippy. The external C8734 snapshot was converted,
installed and reopened with the native reader: 48 pins, 48 pads, 48 mappings.
Upstream CLI verification was skipped because the oracle binary was absent.
This does not yet constitute a complete MCP acquisition flow or 3D alignment check.

### STEP reference audit and representation appearance

SolidCraft was inspected at `22aa00d0072686af2faf1a1f310e9ee6a73ad2a8`,
particularly `crates/kernel/src/step_in/mod.rs` (representation traversal and
appearance) and `solidcore/crates/solidcore-step`. Its software is MIT/Apache-2.0;
the SolidCore subtree is Apache-2.0. UI/branding restrictions are outside this
work. No SolidCraft code was copied. Its CAD backend differs from ours, so this
is a reference audit rather than a proposed backend replacement.

The actual C8734 STEP exposed a missing appearance case: a style on the shape
representation itself. The STEP adapter now applies that style as a default for
direct solid items, below explicit solid/shell/face styles. Conflicting defaults
and mapped representation styles are refused instead of guessing instance colors.
All 19 STEP tests passed; the original C8734 asset also loaded successfully in
the supplier conversion experiment. Model/footprint alignment remains unverified.

### Delivered supplier STEP attachment foundation (A4)

Exact provider model UUIDs now fetch bounded, cancellable STEP assets into a
SHA-256 cache with offline replay and refresh history. Error pages cannot replace
a good cached model. The native converter extracts explicit source origin,
rotation and height, checks the asset checksum and STEP solids, and retains the
original bytes plus model provenance. It refuses missing placement origins and
multiple independently placed assets instead of inventing a shared transform.

55 importer tests and 39 network tests passed, with existing live/external tests
opt-in; all-target Clippy passed for both crates. The opt-in C8734 experiment
converted 48 pins/pads/mappings and attached its actual STEP, then reopened all
native library elements. Upstream CLI comparison and rendered alignment have not
been verified; accepting a STEP file is not evidence of correct model alignment.

### Delivered exact-ID MCP acquisition (A4)

`part_import {lcsc:"C8734"}` acquires source and optional STEP, creates the native
library/bundle in one transaction, rescans and returns complete device/component
details plus a ready declarative device/part entry. It does not modify a project.
`cache:"offline"` makes no HTTP requests; `require_model:true` refuses missing or
invalid models. Missing optional models produce an explicit partial result.
Repeated imports preserve native edits; changed source/model snapshots conflict
instead of overwriting them. HTTP and CAD conversion run outside the session lock;
installation refuses a switched workspace. Sources/model snapshots retain hashes
and attribution. The CLI `call`/`batch` registry exposes the same tool.

All 91 MCP integration tests passed plus all-target MCP/importer Clippy. A local
HTTP test covers native library creation, offline replay, retained manual part
edits, changed-source conflict and mandatory-model refusal. The opt-in live
registry acceptance imported C8734 with 48 mappings and its actual STEP, then
replayed offline with the same native UUID. Exact MPN acquisition, the remaining
part corpus, rendered alignment and upstream CLI reopening remain pending.

### Delivered exact MPN acquisition (A4)

`part_import` accepts exactly one of `lcsc` and `mpn`. MPN selection uses the
tscircuit JLC index with a retained SHA-256 snapshot and offline replay. Only a
unique exact case-insensitive MPN can select an LCSC ID; fuzzy, ambiguous and
possibly truncated responses fail. CAD acquisition independently rechecks the MPN
so a stale index cannot substitute a different part. Ambiguity reports supplier
IDs for an explicit choice; prices and stock do not choose a device.

Focused network/MCP tests and all-target Clippy passed. Tests cover fuzzy suffixes,
duplicate IDs, ambiguous exact MPNs, truncated lists, index/source disagreement,
offline replay, conflicting selectors and native installation by MPN. The live
registry acceptance also passed with `mpn:"STM32F103C8T6"`, acquired C8734 with its
STEP and replayed offline by ID. Some other live index queries returned HTTP 500
during corpus research; exact-ID acquisition remains independent of that index.

### Corpus-driven symbol compatibility (A4)

Actual supplier sources exposed reversed straight pin paths and filled SVG `PT`
artwork. Pin direction now follows the declared electrical connection point at
either endpoint, retaining length/rotation and rejecting paths with neither
endpoint at that anchor. Simple filled SVG paths preserve LED arrows; compound
filled paths remain explicitly unsupported instead of incorrectly filling holes.
56 importer tests and all-target Clippy passed.

The opt-in source conversion/install/native-reopen experiment now passes for
CH340G C14267 (16 mappings), CP2102-GMR C6568 (29, including exposed pad),
USB4105-GF-A-060 C3025063 (16), KT-0603R LED C2286 (2), 0603WAF1002T5E resistor
C25804 (2) and CC0603KRX7R9BB104 capacitor C14663 (2). Combined with STM32 C8734,
all seven initial part classes convert. These are source/native-reader checks:
datasheet pin audits, preview inspection, model alignment and upstream CLI reopen
remain separate acceptance gates. Raw experimental snapshots/logs are retained
under `foilfern-parts/corpus` in the task artifact directory, not copied as fixtures.

### Generated supplier field corrections (A4)

Visual inspection of the new STM32 preview exposed overlapping native symbol
name/value fields and a literal `REF**` footprint text. Generated symbol fields
are now separated above the imported geometry, and the footprint uses the native
name placeholder through a KiCad reference property. Electrical/graphic source
coordinates are retained. Focused importer tests cover separated fields and
native placeholder substitution; importer all-target Clippy passed.

### Reviewed supplier bundle acquisition (A4)

`part_import dry_run:true` renders native symbol/footprint PNGs and returns the
pad/signal map without installing a library. Its `install` request replays offline
snapshots with expected source and (when present) STEP hashes; changed bytes are
refused before installation. CAD conversion and preview rendering remain outside
the session lock. Installed details explicitly use workspace objects, avoiding
an older project-library copy with the same UUID. Capability version is 44.

All 91 ordinary MCP tests passed; the live registry test performed STM32 MPN
preview, hash-pinned installation and offline replay with STEP. The two generated
images were inspected, including the field corrections above. Native pin-number
labels in this generic library-symbol view are placeholders; the returned mapping
contains the exact physical pad numbers. All-target MCP Clippy passed.

The local CLI batch also acquired all seven pinned corpus parts with mandatory
STEP models, using the dev binary for correctness (not a board timing run).
LibrePCB CLI 2.1.1, file format 2, became available from the pinned Nix package;
all seven converter outputs passed `open-library --all --strict`, including the
STM32 package with the actual STEP. STM32's 48 source pin numbers/names also
matched an independent STM32 chip-data physical pinout after explicitly removing
oscillator/RTC/WKUP aliases and supply instance suffixes. Other datasheet audits
and rendered model/footprint alignment remain pending; model acceptance stays
explicitly `alignment_verified:false`. Experimental logs and images remain under
the task's `foilfern-parts/corpus` and `foilfern-parts/previews` directories.

### Delivered partial library installation (A5)

`library_install` resolves named requests independently and retains every settled
download result. Unknown names do not block valid requests; failed downloads do
not hide successful installations. Results include ordered per-query outcomes,
failures and missing/failed dependencies, with an explicit partial outcome.
Successful native files are retained and rescanned even when another request
fails. A library downloaded without a successfully updated dependency is reported
as installed but its request remains partial; no complete dependency state is
claimed. A switched workspace is refused at report publication. The old network
installer API retains its behavior for other callers; the MCP uses the new
per-library report API. Capability version is 45.

92 ordinary MCP tests and 40 network tests passed, plus all-target Clippy for
both crates. A local HTTP regression covers a valid name beside a missing name,
and a successful dependent-library download beside a failed dependency update,
including fresh index visibility. Official KiCad source discovery remains next.

### Official KiCad source acquisition foundation (A5)

`OfficialKiCad` resolves an explicit tag/commit in the official symbol, footprint
or model repository and fetches individual files only at the resolved full commit.
It shares bounded/cancellable HTTP and SHA-256 snapshot storage with supplier
acquisition. Offline replay verifies bytes; failed/mismatched revisions never
fall back to another branch. Local HTTP tests cover pinned URLs, path validation,
offline replay, cancellation and cache tampering; all-target network Clippy passed.

The opt-in live check acquired CH340G, SOIC-16 and its STEP from release 10.0.8:
symbol commit `45efbd93be87e365a21d743daf9b8a07e6414df6`, footprint commit
`a337146e090931ad1176e20ed0b7100990fac5fa`, model commit
`8975715167d1a76f9b44be155ac02326f704cdbb`. Version 10 stores symbols separately
under `Interface_USB.kicad_symdir`; the 9.0.9.1 aggregate symbol file also remains
available. Official licensing/attribution must accompany fetched/converted library
collections; references: https://www.kicad.org/libraries/download/ and
https://www.kicad.org/libraries/license/. MCP automatic assembly/import is next;
fetching valid bytes alone is not native conversion evidence.

### Delivered automatic official KiCad imports (A5)

`library_import source:"official:Library:Symbol", format:"kicad"` now assembles
pinned official source dependencies and enters the existing native importer.
The default tested release is 10.0.8; an exact footprint override and offline
source replay are supported. Split/aggregate symbol files and one inheritance
level work; effective inherited footprint properties are materialized and parents
precede derived symbols for the native import order. Deeper/cyclic inheritance
is refused. Available STEP assets are validated before use. Licenses and source
hash/commit provenance accompany the bundle and native collection. Actual imports
default to a local KiCad Imports library, while dry runs create no native library.
Installed device details and a ready device entry reduce follow-up lookup calls.
Capability version is 46.

The local HTTP acceptance covers inherited symbol selection, STEP attachment,
retained licenses, dry run, offline replay and duplicate prevention. All-target
MCP Clippy passed. A live CH340G registry test performed a dry run and offline
native import with the SOIC-16 STEP; exact device lookup and upstream CLI strict
reopen passed. The current importer deliberately skips already imported elements;
source refresh does not update native data. A richer source-change conflict and
update workflow for shared KiCad dependencies remains future work.

Continuation after this checkpoint: finish A4 datasheet/model-alignment evidence,
then A6 board preview/absolute pads, A7 legend placement, A8 jobs/planes, A9 response
cost, A10 rule provenance, A11 design-document CI and A12 3D/electrical intent.
Use the delivered CLI and acquisition tools for those checks; do not replace the
remaining product work with board experiments or mark the entire plan complete.
