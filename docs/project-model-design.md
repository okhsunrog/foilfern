# Project model design

Current architecture of `foilfern_core::project` and the editor layer, reviewed
2026-11-11. The upstream behavioral reference is `libs/librepcb/core/project`.
This document describes implemented ownership and invariants; new declarative
updates and requirements are planned in [the roadmap](roadmap.md).

The model replaces upstream's pointer graph, registration callbacks and Qt
signals with owned UUID-keyed data, private reverse indices and a change journal.
Native serialization and interoperability remain hard requirements.

## 1. Storage and identity

[`Project`](../crates/foilfern-core/src/project/project.rs) owns the transactional
directory, file name and UUID, metadata/settings, save/open time, typed output
jobs, stroke fonts, project library, circuit, ERC approvals, ordered schematics
and boards, reverse index and change journal. Its revision comes from the journal;
there is no independent persistent revision field in the project file.

- UUID-keyed collections use `BTreeMap`/`BTreeSet`. Their deterministic ordering
  matches the native UUID-sorted serialization without a second identity system.
- Page/board order, assembly variants, attributes and other ordered lists retain
  their explicit list order. The first board is the primary board.
- Typed IDs (`BoardId`, `ComponentInstanceId`, `NetSignalId`, etc.) wrap the UUIDs
  used by native files. Composite references include the owning instance/segment.
  A library pad UUID alone does not identify a placed physical pad.
- Designators and names are lookup/display labels. Renaming an entity does not
  change its UUID. Undo restores the original identities and list positions.
- References are IDs rather than pointers. `ProjectView` borrows the library and
  circuit for computations which need context; items have no parent back-pointers.
- Symbols and devices derive pin/pad views from library mappings and placement.
  Standalone board pads are persistent items. Derived views are not another
  editable set of model objects.
- Boards and schematics contain typed items and a directory name; they do not
  own separate transactional directory handles or preserve the whole native
  document as an unparsed `raw` payload.

`Project` and `OpenProject` assert `Send + Sync`. Core model ownership does not
use `Rc`, `RefCell` or UI callbacks. File-backed library elements use transactional
filesystem handles; this does not turn individual circuit objects into a locked
pointer graph.

## 2. Relationships and reverse lookups

[`RefIndex`](../crates/foilfern-core/src/project/ref_index.rs) is private to the
project. It tracks net uses, component uses and bus uses across aggregates.
Mutation implementations update it; the loader populates the model through the
validating insertion paths. `Project::is_ref_index_consistent()` compares it
with a full rebuild and also checks schematic anchor and board pad indices.

| Relationship | Representation |
| --- | --- |
| Net to component signals, schematic/board segments and planes | Project reverse index |
| Component to symbols and placed board devices | Project reverse index |
| Bus to schematic bus segments | Project reverse index |
| Schematic external anchors / board footprint pad attachments | Aggregate-local indices |
| Lines at an anchor within a segment | Computed from that segment |
| Component signal to library pins/physical pads | Computed from instance and library mappings |
| Rare dependency queries such as net-class users | Model queries rather than a parallel mutable graph |

Indices are accelerators for the authoritative forward references. Clients do
not edit them directly or infer a new circuit netlist from drawn coordinates.
Copper connectivity and plane contacts are separate derived geometric checks.

## 3. Invariants and registration semantics

An entity exists when it is in its owning collection. There is no separate
constructed/registered/added lifecycle. Validating insertion and mutation paths
check duplicate identities/names, referenced entities, library mappings,
anchor nets/layers and net-segment cohesion. Errors are typed and preserve
upstream messages where compatibility requires them.

Removal in core is strict. A referenced net/component cannot simply disappear;
editor commands compose the needed disconnections/removals. Core does not hide
cascading edits from a caller. Failed individual validating mutations do not
commit invalid model data. Batch rollback restores prior data using inverses
and restores the saved file overlay before discarding the buffered events.

## 4. Mutations, commands and undo

[`Mutation`](../crates/foilfern-core/src/project/mutation/mod.rs) is serializable
data. `Project::apply(mutation)` returns the inverse on success. Schematic and
board item operations are nested `SchematicMutation` and `BoardMutation` variants;
typed project methods are wrappers around the same implementation.
`Mutation::Batch` applies in order and returns inverses in reverse order. If a
step fails, it applies the inverses of earlier steps before returning the error.

The inverse of a successful mutation must succeed when applied immediately.
Removal inverses carry removed values so undo preserves identities. Inverses
are data, not closures. Serialization details live in the source enums rather
than a second handwritten list of variants in this document.

[`ProjectEditor`](../crates/foilfern-editor/src/editor.rs) owns the project,
library source and undo stack. Intent-level `Command`s execute through a
`Transaction`. A command starts one labeled group, or joins an already active
group. Standalone commands and MCP writes run through `execute_atomic`, which
holds exclusive editor access for the whole operation. Nested commands use
savepoints within that boundary. Empty groups are dropped. Committing
a new group discards redo; starting and aborting one does not discard redo.
Undo/redo apply stored inverse operations and keep their inverses for the next
transition. They restore data, not an earlier revision number.

[`Operation`](../crates/foilfern-editor/src/undo_stack.rs) covers more than
`Mutation`: it also adds/removes file-backed project library elements and
writes/removes project asset files. Library additions/removals are `Project`
methods with journal events, not serializable `Mutation` variants. This is why
a future declarative update which installs parts must use an editor transaction,
not only a batch of circuit mutations.

Replacing the artwork of an embedded symbol is a serializable
mutation. It stages the native symbol file in the same transactional overlay
and journals the symbol UUID so schematic scenes rebuild its uses. The symbol's
pin interface stays fixed.

Derived-data updates use `ProjectEditor::update_derived_data()` outside the undo
history. They can still publish change events. Dirty state is tracked by the
editor's clean/history state and manual modifications, with pending file-format
upgrade state in `OpenProject`; revision equality alone is not a dirty-state test.

## 5. Change notification and derived data

[`ChangeLog`](../crates/foilfern-core/src/project/change.rs) stores a bounded
sequence of `Change` events (default capacity 4096). It currently uses a vector,
dropping the older half when full. `Project::revision()` is the latest sequence
number; one command/mutation may publish multiple events. A lagging consumer of
`changes_since(cursor)` receives `ChangesSince::Resync` and rebuilds its view.
`clear()` keeps the revision counter; creation/loading reset it to zero.

Atomic edits buffer events until the outer commit. A nested failure reverses
its own edits, restores its copy-on-write file overlay and drops its buffered
events. A successful rollback preserves the published journal, even when the
aborted edit produced more events than its capacity. Asset writes publish
`ProjectAsset` when bytes change. Inverse failures return an explicit error,
advance the revision and force observers to resync.

Undo/redo and cancellation of visible interactive changes publish new events.
Routing previews still use detached snapshots. Runtime revision numbers are
scoped to an open model.

Boards maintain dirty net/layer sets and derived air wires/plane fragments.
Mutations invalidate these directly; correctness does not depend on a UI consumer
reading the journal. Plane and air-wire rebuilds are synchronous core operations
or operate on extracted jobs on caller-provided workers. Consumers must rebuild
what they need before claiming connectivity or reference coverage.

ERC reads the model. DRC runs on extracted board check data and rebuilt copper.
Scenes consume the journal incrementally and handle `Resync`. MCP host notices
and a `tokio::sync::watch` channel notify the application layer; core has no
async runtime or observer callbacks.

Attribute-expanded board text depends on save/open time and source paths as
well as ordinary mutations. Cache and candidate freshness must account for these
inputs rather than keying everything on revision alone.

## 6. Concurrency model

[`SharedProject`](../crates/foilfern-editor/src/open_project.rs) is
`Arc<parking_lot::Mutex<OpenProject>>`. `OpenProject` holds the editor, directory
lock/filesystem, save/upgrade bookkeeping and session-owned `RoutingService`.
The desktop and embedded MCP share that handle and one undo history. Standalone
MCP owns an equivalent session; it is not a separate domain implementation.

MCP locks workspace before project when both are needed. Ordinary synchronous
tool work executes under session guards in `spawn_blocking`; that alone does
not make a long operation release its lock. Detached routing preparation uses
a separate capture/work/publish path, releasing live guards during search and
validation. Host callbacks occur after those guards are released. Never hold
project guards across an await or call state setters while holding those guards.

A live interactive undo group makes conflicting MCP writes refuse rather than
mix operations. GUI and MCP both mutate through the shared editor. Workers own
snapshots/extracted data; their publication checks source freshness. Concurrency
limits and cooperative cancellation for routing workers remain roadmap P3.

The original proposal used `RwLock<Project>`. The implemented ownership boundary
is `Mutex<OpenProject>` because edits include history, file state and candidate
state. An actor or copy-on-write project model is not currently used.

## 7. Loading, saving and detached snapshots

[`ProjectLoader`](../crates/foilfern-core/src/project/loader/mod.rs) verifies
native markers/versions, rejects unsupported newer files, performs migrations
in the transactional filesystem and loads typed jobs, library, circuit,
schematics, boards and settings. Old-format migration and ERC approval cleanup
are implemented. Native save writes canonical upstream-compatible documents;
round-trip fixtures check byte identity where required.

`Project::save()` stages files and updates its save time.
`TransactionalFileSystem::save()` commits to disk; `OpenProject::save()` uses
the editor save path and updates its save/upgrade bookkeeping. Removing a board
or page removes its directory from the overlay; undo can regenerate its native
files from typed data. The application/editor creation path installs stroke-font
assets; low-level `Project::create()` does not copy application resources itself.

`Project::snapshot()` copies the current transactional assets/library and
serializes the current model, including unsaved edits, into an independent
in-memory transactional filesystem, then loads a new `Project`. It does not
write the source, acquire a second source lock, or alter live journal/history.
The snapshot starts at revision zero; callers retain the live base revision
separately. Derived copper is rebuilt by its consumer. Logical source paths and
save time are preserved for text attributes without sharing filesystem writes.

Routing candidates freeze mutations generated on this detached editor. They bind
to the service instance, project/board, live revision and save/path context.
Apply replays the frozen patch rather than rerunning search. Baseline-check reuse
is session-local and uses those freshness inputs; candidate and non-empty
applied-model checks remain fresh. The complete lifecycle, rejection rules and
post-commit check semantics are in [routing service design](routing-service-design.md).
Candidates and baseline caches are not native project objects.

## 8. Module ownership

| Area | Owning source |
| --- | --- |
| Project aggregate, snapshots and serialization | `foilfern-core/src/project/project.rs` |
| IDs, changes, reverse index and library ownership | `foilfern-core/src/project/{id,change,ref_index,library}.rs` |
| Typed circuit, schematic and board data | `foilfern-core/src/project/{circuit,schematic,board}/` |
| Validating changes and native loading | `foilfern-core/src/project/{mutation,loader}/` |
| Electrical and board checks | `foilfern-core/src/project/erc/`, `board/drc/` |
| Commands, file/library operations and history | `foilfern-editor/src/{editor,undo_stack,open_project}.rs`, `commands/` |
| Detached routing work and validation | `foilfern-editor/src/routing_service/`, `routing_service.rs`, `routing_validation.rs` |
| Tool DTOs, dispatch and session orchestration | `foilfern-mcp/src/` |
| UI event translation and scene presentation | `foilfern-app`, `foilfern-scene`, `foilfern-canvas` |

The initial skeleton/parallel porting assignments are historical. These modules
contain implementations; new work follows the ownership boundaries rather than
reintroducing placeholders or a second model for an external algorithm.

## 9. Representation and implementation choices

| Need | Choice |
| --- | --- |
| Stable file/object identity and ordering | Typed UUIDs, `BTreeMap`/`BTreeSet`, explicit ordered lists |
| Segment cohesion and graph algorithms | Existing `petgraph` dependency |
| Parallel plane work | `rayon` on extracted data |
| Typed errors and thread-safety assertions | `thiserror`, `static_assertions` |
| Shared editor ownership | `parking_lot::Mutex<OpenProject>` |
| Async host notifications | `tokio::sync::watch` outside core |
| Wire representation | Unconditional serde on `Mutation`, `Change` and contained model values |

Serde uses UUID strings (including map keys), integer nanometres for lengths,
integer microdegrees for angles, and `{x, y}` integer coordinates for points.
Validated string newtypes serialize as strings; ordered object lists as arrays;
enums such as mutations are externally tagged. Dates use RFC 3339. Typed MCP
DTOs convert human-facing mm/degrees at the boundary; raw mutations use core units.
Use generated schemas for the exact current shape.

This serde wire form is not the native file format. Native S-expression writers
preserve ordering, numeric rounding and escaping independently. Project library
objects own directories and are handled by editor operations instead of pretending
the entire runtime project can be serialized through the mutation wire schema.

## 10. Design boundaries and future extensions

Stable UUIDs avoid maintaining a second arena-key identity across files, undo
and MCP. Pull-based changes avoid callbacks/re-entrancy inside core. Explicit
editor composition keeps destructive cascades visible. Native snapshots preserve
unsaved state and assets while isolating long experiments from the live editor.

Declarative schematic reconciliation resolves human labels to stable identities,
then captures model, library and file operations from a detached snapshot.
Apply replays those operations inside one transaction against the same project
revision and save context.

Schematic requirements use the versioned `project/foilfern-schematic-requirements.json`
asset with native UUID references and integer nanometres. The editor stages its
changes through undoable file operations, which publish a project asset change
and invalidate older schematic previews. Preview measures the proposed model.
Apply checks the requirements document and re-evaluates hard rules before
committing the patch. Tests cover save/reopen, undo, native archive export and
preservation through upstream LibrePCB open/save. Board requirements remain a
separate roadmap milestone.

## Verification entry points

- Core `tests/unittests/project/` covers model operations, indices, journal,
  detached snapshots and their isolation; `tests/unittests/compat/` covers native
  round trips, checks and exports against upstream fixtures/oracles.
- Editor `tests/editor/` covers commands, undo, library/assets and editor FSMs.
- MCP `tests/mcp/routing_candidates.rs` covers exact preview/apply, stale/session
  refusal, connected edits, rebuilt-plane regressions and measurements.
- Application `tests/app/` covers shared editing and headless UI behavior;
  rendering/desktop acceptance has separate platform requirements.

Run focused tests for the affected boundary and broaden for cross-cutting changes
as required by AGENTS.md. A skipped optional oracle is not a passing comparison.
