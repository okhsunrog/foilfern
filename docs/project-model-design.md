# Project model design

Design of `librepcb_core::project` (port of `libs/librepcb/core/project`).
Everything else in the editor stack (schematic/board editors, undo/redo, the
command bus shared by UI, CLI and MCP) builds on it, so this document states
decisions, not options. Upstream is a densely linked object graph (raw
back-pointers, `register*()`/`addTo*()` bookkeeping, `isAddedTo*` flags,
`ScopeGuardList` rollback, Qt signals). The port replaces it with a plain
data tree keyed by the UUIDs the file format already has, one private
reverse index, and a change journal.

## 1. Storage and identity

**Tree of plain data, `BTreeMap<Uuid, T>` for every UUID-keyed collection.**

```rust
pub struct Project {
    directory: TransactionalDirectory,      // project root
    file_name: String,                      // "*.lpp"
    metadata: ProjectMetadata,              // uuid, name, author, version, created, attributes
    settings: ProjectSettings,              // locale/norm order, custom BOM attrs, lock default
    output_jobs: OutputJobList,             // project/jobs.lp (`job` module)
    stroke_fonts: StrokeFontPool,
    library: ProjectLibrary,
    circuit: Circuit,
    erc_approvals: BTreeSet<SExpression>,
    schematics: Vec<Schematic>,             // page order = index file order
    boards: Vec<Board>,                     // first = primary board
    refs: RefIndex,                         // §2, private
    journal: ChangeLog,                     // §5
    revision: u64,
}

pub struct Circuit {
    assembly_variants: AssemblyVariantList,             // insertion order (file order)
    net_classes: BTreeMap<Uuid, NetClass>,
    net_signals: BTreeMap<Uuid, NetSignal>,
    buses: BTreeMap<Uuid, Bus>,
    component_instances: BTreeMap<Uuid, ComponentInstance>,
}
pub struct ComponentInstance {
    uuid: Uuid, name: CircuitIdentifier, value: String,
    lib_component: Uuid, lib_variant: Uuid,             // resolved via ProjectLibrary
    attributes: AttributeList, assembly_options: ComponentAssemblyOptionList,
    lock_assembly: bool,
    signals: BTreeMap<Uuid, ComponentSignalInstance>,  // key: library signal UUID
}
pub struct ComponentSignalInstance { net: Option<Uuid> }  // + nothing else persistent

pub struct Schematic {
    directory: TransactionalDirectory, directory_name: String,
    uuid: Uuid, name: ElementName, grid_interval: PositiveLength, grid_unit: LengthUnit,
    symbols: BTreeMap<Uuid, SchematicSymbol>,
    bus_segments: BTreeMap<Uuid, SchematicBusSegment>,
    net_segments: BTreeMap<Uuid, SchematicNetSegment>,
    polygons: BTreeMap<Uuid, Polygon>, texts: BTreeMap<Uuid, Text>, images: BTreeMap<Uuid, Image>,
}
pub struct SchematicNetSegment {
    uuid: Uuid, net: Uuid,
    junctions: BTreeMap<Uuid, Junction>,               // geometry types reused as-is
    lines: BTreeMap<Uuid, NetLine>,                    // anchors = geometry::NetLineAnchor
    labels: BTreeMap<Uuid, NetLabel>,
}
```

Why `BTreeMap<Uuid, _>` and not slotmap/arena keys or `IndexMap`:

- Upstream keeps these in `QMap<Uuid, T*>` and serializes by iterating it,
  so **file order is UUID order** (verified in `tests/data/projects/*`:
  nets, components, symbols, segments, junctions, lines, labels, devices,
  planes, pads, vias, ... are all sorted by UUID string). `BTreeMap`
  iteration is that order for free; `Uuid: Ord` already matches the string
  order. Byte-identical save needs no sorting step.
- UUIDs are stable across load/save/undo: an undone removal re-inserts the
  same key, and MCP/CLI clients can address entities by the identifiers they
  see in files. Slotmap keys are neither stable nor meaningful outside the
  process, so they would have to be mapped to UUIDs everywhere anyway.
- Deterministic iteration everywhere (upstream iterates `QHash` in a few
  places where order does not affect files; we get the sorted order,
  recorded in COMPAT.md where observable, e.g. auto-numbering).

Insertion-ordered collections stay `Vec`/`SerializableObjectList`:
schematics, boards, assembly variants, attributes, assembly options, parts.

**References are UUIDs, never pointers.** Newtype IDs give type safety
without changing the representation: `NetSignalId(Uuid)`, `NetClassId`,
`BusId`, `ComponentInstanceId`, `SchematicId`, `BoardId`, `NetSegmentId`,
`SymbolId`, `PlaneId`, ... (one `id_type!` macro; `Display`/`FromStr`/
`ToSExpression` delegate to `Uuid`). Nested items are addressed by path
structs: `ComponentSignalRef { component, signal }`, `SymbolPinRef { symbol,
pin }`, `NetPointRef { segment, junction }`. Net line / trace anchors are the
existing `geometry::NetLineAnchor` and `geometry::TraceAnchor` enums, which
already carry exactly the file-format references (junction, pin{symbol,pin},
bus junction, via, pad, footprint pad{device,pad}); no second "resolved"
anchor type exists. Resolution is a lookup with context:
`schematic.anchor_position(&anchor, ctx) -> Option<Point>`.

**No back-pointers.** Children do not know their parents. Functions that
need context take it explicitly, bundled in a read view:

```rust
pub struct ProjectView<'a> { pub library: &'a ProjectLibrary, pub circuit: &'a Circuit, ... }
impl SchematicSymbol {
    pub fn pins<'a>(&'a self, ctx: ProjectView<'a>) -> Result<impl Iterator<Item = SymbolPinView<'a>>>;
}
```

**Instances derived from library elements are not stored.** Upstream
allocates `SI_SymbolPin` per symbol pin and `BI_Pad` per footprint pad, each
caching position, name, numbers and geometry. Here they are computed views
(`SymbolPinView`, `FootprintPadView`) built from the placed item, the library
element and the component's pin-signal / pad-signal maps. This removes the
largest source of registration code; the only state a pin/pad has that is not
derivable is "which net lines touch it", which is an index (§2). Standalone
board pads (`BoardPadData` in a net segment) are stored, as upstream.

`Send + Sync`: everything is owned plain data. `TransactionalDirectory`
wraps `Arc<TransactionalFileSystem>` with a `Mutex`, already asserted
`Send + Sync`. `static_assertions::assert_impl_all!(Project: Send, Sync)`
sits next to the type.

## 2. Relationships and reverse lookups

Decision per relation (S = stored index maintained by the mutation API,
D = computed on demand):

| Relation (upstream registration) | Choice | Why |
|---|---|---|
| NetSignal → schematic segments, board segments, planes, component signals (`register*NetSegment`, `registerComponentSignal`, `registerBoardPlane`) | S (`RefIndex::net_uses`) | queried by every removal, every rename, ERC for every net, air wire scheduling; on demand would scan all segments of all schematics and boards per query |
| ComponentInstance → symbols per gate (`registerSymbol`), devices per board (`registerDevice`) | S (`RefIndex::component_uses`) | removal check, "gate already placed", "all symbols in one schematic", primary device, BOM/`getUsedDeviceUuids` |
| Bus → bus segments | S (`RefIndex::bus_uses`) | same pattern as nets |
| External anchor → net segment (pin → `SI_NetSegment`, footprint pad → `BI_NetSegment`, bus junction → net segment) | S (per schematic/board `anchor_segments: BTreeMap<ExternalAnchor, NetSegmentId>`) | encodes the invariant "one segment per pin/pad" directly; ERC and scene builders ask it per pin; a scan would be O(lines) per pin |
| Anchor → lines within its segment (`mRegisteredNetLines`, `isOpen`, junction dots) | D (`segment.lines_at(anchor)`) | segments are small; a line count is a linear scan of tens of lines |
| ComponentSignal → pins/pads (`registerSymbolPin`, `registerFootprintPad`) | D | derivable from `component_uses` + library maps; needed only by ERC and pad-name display |
| NetClass → nets, library element → users, device ↔ component | D | rare operations (remove net class, prune library), linear in the circuit |

```rust
pub(crate) struct RefIndex {
    net_uses: BTreeMap<NetSignalId, BTreeSet<NetUse>>,
    bus_uses: BTreeMap<BusId, BTreeSet<(SchematicId, BusSegmentId)>>,
    component_uses: BTreeMap<ComponentInstanceId, ComponentUses>,
}
pub enum NetUse {
    ComponentSignal(ComponentSignalRef),
    SchematicSegment(SchematicId, NetSegmentId),
    BoardSegment(BoardId, NetSegmentId),
    Plane(BoardId, PlaneId),
}
pub struct ComponentUses {
    symbols: BTreeMap<Uuid /* gate item */, (SchematicId, SymbolId)>,
    devices: BTreeSet<BoardId>,      // a board holds at most one device per component
}
```

`RefIndex` is private to the `project` module, rebuilt from scratch after
load (`RefIndex::build(&Project)`), updated incrementally by every mutation
that adds/removes/re-targets a referencing item, and checked against a full
rebuild in `debug_assert!` inside tests. Because forward references live in
different aggregates than the referenced entity (a board segment references a
circuit net), the index lives on `Project`, not on `Circuit`.

## 3. Invariants and registration semantics

Upstream has two error classes: `LogicError` for misuse of the object
lifecycle (not added, added twice, wrong parent) and `RuntimeError` for
data-level conflicts. The lifecycle class disappears: **an entity exists iff
it is in its map**. Constructors produce values (`NetSignal::new(uuid,
net_class, name, auto)`), insertion validates, removal returns the value.
There is no "constructed but not added" state, so `isAddedToCircuit`,
`addToSchematic()`/`removeFromBoard()` and the recursive register/unregister
cascades have no Rust counterpart.

Data-level rules become variants of `project::Error` with the upstream
messages (`tr!` with the upstream class name as context):

| Upstream check | Rust error (checked before mutating) |
|---|---|
| duplicate UUID / name / directory name (schematic, board, net class, net, bus, component, assembly variant) | `DuplicateUuid`, `DuplicateName`, `DuplicateDirectoryName` |
| remove net/component/bus/net class still referenced | `NetSignalInUse`, `ComponentInUse`, `BusInUse`, `NetClassInUse` (via `RefIndex`/scan) |
| remove schematic that is not empty; remove last assembly variant | `SchematicNotEmpty`, `LastAssemblyVariant` |
| symbol: gate not in variant / gate already placed / other schematic than sibling symbols / symbol not in library / pin count mismatch | `InvalidSymbolItem`, `SymbolItemAlreadyPlaced`, `SymbolsInDifferentSchematics`, `MissingLibrarySymbol`, `PinCountMismatch` |
| device: no such device/package/footprint in project library, device ↔ component mismatch, unknown signal in pad-signal map | `MissingLibraryDevice`, ..., `DeviceComponentMismatch` |
| line/trace to pin/pad of another net; trace layer not on pad; lines of several segments on one pin/pad; both endpoints equal | `AnchorNetMismatch`, `TraceLayerMismatch`, `AnchorInMultipleSegments`, `DegenerateLine` |
| net segment not cohesive (after add/remove of elements) | `NetSegmentNotCohesive` (connected components via `petgraph::unionfind::UnionFind` over the candidate element set) |
| change net of component signal while its pins/pads are wired; change net of a used segment | `ComponentSignalInUse`, `NetSegmentInUse` |
| loader: inexistent net class / net / component / signal, signal defined twice, signal count mismatch, unknown anchor | one variant each, messages verbatim |

**Check-then-commit replaces `ScopeGuardList`.** Every mutation validates
against the current model plus the candidate change and only then mutates,
so a failed mutation leaves the model untouched. Composite operations
(loading a schematic, adding a net segment with elements) validate the
composed candidate (e.g. cohesion of `existing ∪ new` elements) before
inserting anything.

**Strict removal, no cascade, exactly as upstream.** Removing a net that
still has segments fails; the editor removes the segments first (its own
commands, each individually undoable). This keeps every mutation's inverse a
single mutation (§4).

## 4. Mutation API

**Commands are data.** There is one `Mutation` enum; `Project::apply()` is
the only way to change a loaded project, and it returns the inverse:

```rust
#[non_exhaustive]
pub enum Mutation {
    // circuit
    AddNetSignal(NetSignal),
    RemoveNetSignal(NetSignalId),
    SetNetSignalName { id: NetSignalId, name: CircuitIdentifier, auto: bool },
    SetComponentSignalNet { signal: ComponentSignalRef, net: Option<NetSignalId> },
    AddComponentInstance(ComponentInstance),
    // schematic
    AddSchematic { schematic: Schematic, index: usize },
    AddSymbol { schematic: SchematicId, symbol: SchematicSymbol },
    SetSymbolPlacement { symbol: SymbolRef, position: Point, rotation: Angle, mirrored: bool },
    AddNetSegmentElements { segment: NetSegmentRef, junctions: Vec<Junction>, lines: Vec<NetLine> },
    RemoveNetSegmentElements { segment: NetSegmentRef, junctions: Vec<Uuid>, lines: Vec<Uuid> },
    SetNetSegmentNet { segment: NetSegmentRef, net: NetSignalId },
    // board
    AddDevice { board: BoardId, device: BoardDevice },
    AddBoardNetSegmentElements { .. }, SetPlaneOutline { .. }, ...
    // project
    SetProjectMetadata(ProjectMetadata), SetProjectSettings(ProjectSettings), SetErcApproval { .. },
    Batch(Vec<Mutation>),                 // applied in order, inverted in reverse
}

impl Project {
    /// Applies `m`; on success returns the mutation that undoes it.
    /// On error nothing changed (check-then-commit; `Batch` rolls back
    /// already applied steps with their inverses).
    pub fn apply(&mut self, m: Mutation) -> Result<Mutation>;
    pub fn revision(&self) -> u64;
}
```

Guaranteed invariant: *the inverse of a successful mutation, applied
immediately afterwards, succeeds*. Removal inverses carry the removed value
(`RemoveNetSignal(id)` inverts to `AddNetSignal(value)`, `RemoveSchematic`
inverts to `AddSchematic { schematic, index }`), so undo restores UUIDs and
list positions and the saved file is identical to before.

Granularity mirrors upstream's `Cmd*` classes (which are the atomic model
operations upstream already needed for undo): entity add/remove, property
set, segment element add/remove, net re-targeting. Typed methods
(`project.add_net_signal(ns) -> Result<NetSignalId>`,
`project.circuit()`, `project.schematic(id)`, ...) are the readable API;
`apply` dispatches to `pub(crate)` implementations per area so the enum stays
the single source of truth. The later command bus is then: `UndoStack {
groups: Vec<(text, Vec<Mutation /* inverses */>)> }`, and MCP/CLI submit
`Mutation`s (a serde DTO layer lives in the MCP crate; `Mutation`, `Change`
and their contained types derive serde unconditionally for the JSON wire format).

**Derived data** is kept out of the persistent tree and invalidated by
dirty sets that the mutations maintain (the same events upstream turns into
`scheduleAirWiresRebuild()` / `invalidatePlanes()`):

```rust
pub struct Board {
    ...persistent fields...,
    derived: BoardDerived,   // skipped by PartialEq and by save()
}
pub struct BoardDerived {
    air_wires: BTreeMap<Option<NetSignalId>, Vec<AirWire>>,
    dirty_air_wire_nets: BTreeSet<Option<NetSignalId>>,
    plane_fragments: BTreeMap<PlaneId, Vec<Path>>,
    dirty_plane_layers: BTreeSet<Layer>,
}
impl Project {
    pub fn rebuild_air_wires(&mut self, board: BoardId);            // sync, BoardAirWiresBuilder
    pub fn plane_job(&self, board: BoardId, layers: Option<&BTreeSet<Layer>>) -> PlaneJob; // snapshot
    pub fn apply_plane_fragments(&mut self, board: BoardId, result: PlaneFragments); // not undoable
}
```

- Air wires: dirty nets are collected per mutation; the editor calls
  `rebuild_air_wires` after each command batch (upstream `trigger`), the CLI
  before export. `AirWire { net, p1: TraceAnchor, p2: TraceAnchor }` plus
  positions.
- Plane fragments: `PlaneJob` is the ported `JobData` snapshot; the builder
  runs on a caller thread (rayon per layer, as upstream uses QtConcurrent)
  and the result is applied through `apply_plane_fragments`, journaled as
  `Change::PlaneFragments` but never part of undo.
- Pad geometry, pin positions, symbol/device texts, attribute lookup
  (`ProjectAttributeLookup`) are pure functions of the model and the
  library; consumers memoize by `(revision)` if they need to.
- ERC runs directly on `&Project`. DRC works on an extracted
  `BoardDrcData` (upstream already does this), so it can run off-thread.
- Net connectivity for MCP ("netlist") is a query over
  `circuit.component_instances[*].signals` and `RefIndex::net_uses`; no
  extra state.

## 5. Change notification

No callbacks in core. `Project` keeps a **change journal**:

```rust
pub struct ChangeLog { first_seq: u64, entries: VecDeque<(u64, Change)>, capacity: usize }
#[non_exhaustive]
pub enum Change {
    ProjectMetadata, ProjectSettings,
    NetSignalAdded(NetSignalId), NetSignalRemoved(NetSignalId), NetSignalRenamed(NetSignalId),
    ComponentInstance { id: ComponentInstanceId, what: ComponentChange },
    SchematicAdded(SchematicId), SchematicRemoved(SchematicId),
    Symbol { schematic: SchematicId, id: SymbolId, what: ItemChange },   // Added, Removed, Placement, Texts
    NetSegment { schematic: SchematicId, id: NetSegmentId, what: SegmentChange }, // Added, Removed, Net, Elements{..}, Labels
    Device { board: BoardId, component: ComponentInstanceId, what: ItemChange },
    Plane { board: BoardId, id: PlaneId, what: PlaneChange },
    AirWires(BoardId), PlaneFragments(BoardId, Vec<PlaneId>),
    Resync,   // journal overflowed: rebuild everything
    ...
}
impl Project {
    pub fn changes_since(&self, seq: u64) -> ChangesSince<'_>; // Ok(iterator) or Resync if `seq` fell off the ring
}
```

Every `apply` appends the entity-level events that upstream would have
emitted (`Schematic::symbolAdded` → `Change::Symbol{Added}`,
`SI_Symbol::onEdited(PositionChanged)` → `Change::Symbol{Placement}`, list
signals → `Elements{added, removed}`), always after the mutation succeeded,
and bumps `revision`. Consumers (scene builders, ERC panel, MCP
subscriptions) pull with their own cursor. The UI loop calls `changes_since`
after each command batch; async code learns the new revision through a
`tokio::sync::watch<u64>` the application layer updates after every apply
(core stays synchronous). The journal is a bounded ring (default 4096
entries); a lagging cursor gets `Resync`, which every consumer must handle
anyway (initial build).

Dirty sets for derived data (§4) are computed by the mutations themselves,
not by re-reading the journal, so the journal is purely an outbound
interface.

## 6. Concurrency model

Recommendation: **one owner, `Arc<parking_lot::RwLock<Project>>` at the
application layer, short critical sections, long work on extracted
snapshots.**

- The editor thread applies commands under the write lock (microseconds to
  a few ms). Nothing else ever mutates.
- MCP handlers (`spawn_blocking`) and CLI take the read lock only to
  *extract* what they need (JSON netlist, `BoardDrcData`, `PlaneJob`, a
  `Project::clone()` for an export run) and release it before computing or
  awaiting. DRC, plane fragments, exports and ERC-on-request all follow
  upstream's existing `JobData` pattern, so this costs nothing extra.
- Locks are never nested and never held across `.await`; the rule is
  documented on the type that owns the lock (editor crate), not in core.
- Change propagation to async code: the `watch<u64>` revision channel.

Rejected: actor/message passing to the owner thread (adds latency and an
API per query; can be layered on later without changing core),
`ArcSwap<Project>` copy-on-write (clones the whole project on every edit
while any reader holds a snapshot), persistent (`im`) collections (a second
collection API for little gain; `BTreeMap` clones are cheap enough for the
occasional full snapshot).

## 7. Loading and saving

**Loader** (`project::ProjectLoader`, order identical to upstream
`ProjectLoader::open`): verify `<name>.lpp` and `.librepcb-project` exist;
read the format version; reject newer; run file format migrations on the
`TransactionalDirectory` (the migration agent's code, one call site here,
producing the same `MigrationLog` HTML); then `metadata.lp`, `settings.lp`,
`jobs.lp` (typed `job::OutputJobList`), library
(`library/{sym,pkg,cmp,dev}/<uuid>/` via `TransactionalFileSystem::dirs()`;
invalid element directories are skipped with a warning, loaded with
`LibraryBaseElement::open`), `circuit/circuit.lp` (variants, net classes,
nets, buses, components with signals; each with the upstream error), ERC
approvals, schematics in `schematics.lp` order, boards in `boards.lp` order,
user settings (errors → defaults, logged). After a migration: run ERC, keep
only approvals still produced, `save()`.

Loading goes through the same validating insertion functions as editing
(builds `RefIndex` incrementally, same error messages as upstream, which
also validates while loading); the journal is cleared and `revision` reset
when `open` returns. Each schematic/board is deserialized into a complete
value first and inserted once (upstream `addToProject()` validates the
whole page in one go).

**Saving** (`Project::save(&mut self) -> fileio::Result<()>`) writes the
same files in the same order as `Project::save()` upstream into the
transactional file system: `.librepcb-project`, `<name>.lpp`,
`project/metadata.lp`, `settings.lp`, `settings.user.lp`, `jobs.lp`,
`circuit/circuit.lp`, `circuit/erc.lp`, `schematics/schematics.lp` +
`schematics/<dir>/schematic.lp` + `settings.user.lp`, `boards/boards.lp` +
`boards/<dir>/board.lp` + `settings.user.lp`; then updates `date_time`.
Committing to disk stays `TransactionalFileSystem::save()`.

Element order in files (what must be preserved):

| Sorted by UUID (upstream `QMap`) → `BTreeMap` | Insertion order (upstream lists) → `Vec` |
|---|---|
| net classes, nets, buses, components, signals per component | assembly variants, attributes, assembly options, parts |
| symbols, symbol texts, bus/net segments, junctions, lines, labels, polygons, texts, images | schematics, boards (index files and page order) |
| devices (keyed by component UUID), device stroke texts, pads, vias, traces, planes, zones, polygons, stroke texts, holes | silkscreen layer lists, preferred footprint tags |
| ERC/DRC approvals (`BTreeSet<SExpression>`, upstream `sortedQSet`) | layer visibility (`BTreeMap<String,bool>`, upstream `QMap<QString,bool>`) |

**Project library** (`ProjectLibrary { directory, symbols: BTreeMap<Uuid,
Symbol>, packages, components, devices }`): elements are the existing
`library` types with their own `TransactionalDirectory` under
`library/<sym|pkg|cmp|dev>/<uuid>/`. `add_*` copies a foreign element into
the project's file system immediately (`save_into_parent_directory`, as
upstream; nothing reaches disk before `save()`), `remove_*` moves its files
to a temporary directory and returns the element value, so the undo inverse
(`AddLibraryElement(value)`) restores files and entry. Model objects
reference library elements by UUID only (`lib_component`, `lib_device`,
`lib_footprint`, gate → symbol UUID); `MissingLibrary*` errors at insertion
replace upstream's constructor exceptions.

## 8. Module layout and parallel porting

File names describe the Rust type or concept, without the prefixes the
module path already implies (AGENTS.md); the upstream file is named in each
`//! Port of ...` module doc. Types drop the upstream abbreviations
(`SchematicSymbol`, `SchematicNetSegment`, `BoardDevice`, `BoardNetSegment`,
`BoardPlane`).

```
crates/librepcb-core/src/project/
  mod.rs               doc, re-exports; declares all submodules up front
  id.rs                typed identifiers (id_type!) and path structs (ComponentSignalRef, ...)
  error.rs             project::Error, EntityKind (one enum, upstream messages)
  project.rs           Project, ProjectMetadata, ProjectSettings, save(), reverse index queries
  library.rs           ProjectLibrary, LibraryElementKind (project-local library elements)
  ref_index.rs         RefIndex, Reference, NetUse, ComponentUses (private index)
  change.rs            Change, ChangeLog, ChangesSince
  mutation/            mod.rs (Mutation, apply, Batch, typed wrappers, library add/remove)
                       circuit.rs, schematic.rs (SchematicMutation), board.rs (BoardMutation)
  loader/              mod.rs (ProjectLoader: open, migrations hook, metadata/settings/jobs/library/erc)
                       circuit.rs, schematic.rs, board.rs
  circuit/             circuit.rs, net_class.rs, net_signal.rs, bus.rs, assembly_variant.rs,
                       component_instance.rs, component_signal_instance.rs, component_assembly_option.rs
  schematic/           schematic.rs (Schematic, SchematicProperties), symbol.rs (SchematicSymbol + pin views),
                       net_segment.rs, bus_segment.rs, change.rs (SchematicChange)
                       later: polygon/text/image items, net_segment_splitter.rs
  board/               board.rs (Board, BoardDerived, BoardProperties), device.rs (BoardDevice + pad views),
                       net_segment.rs, plane.rs, air_wire.rs, change.rs (BoardChange)
                       later: design_rules.rs, fabrication_output_settings.rs, *_data.rs, zone/polygon/
                       stroke_text/hole items, air_wires_builder.rs, plane_fragments_builder.rs,
                       net_segment_splitter.rs, drc/
  erc/                 later: electrical_rule_check.rs, electrical_rule_check_messages.rs
```

The skeleton (implemented): ids, `error.rs`, `Project` with
metadata/settings/save, `ProjectLibrary`, the complete `circuit/`,
`RefIndex`, `change.rs`, `mutation/mod.rs` + `mutation/circuit.rs`,
`loader/mod.rs` + `loader/circuit.rs`, and the `Schematic`/`Board`
containers with their item maps typed against placeholder item structs in
their final files. Until the items are ported, the loaded `schematic.lp`,
`board.lp` and the board's `settings.user.lp` are kept verbatim
(`raw` fields, `#[serde(skip)]`) and written back unchanged, so the
round-trip test already covers whole projects. Three agents then work
without touching the same files:

- **schematic items**: `schematic/**` (item structs, `Schematic::references()`,
  `Schematic::is_component_signal_wired()`, `Schematic::save()` instead of
  `raw`), `mutation/schematic.rs` (`SchematicMutation` variants and their
  validation, reverse index updates via `p.refs.insert()/remove()`),
  `loader/schematic.rs`, `schematic/change.rs` (`SchematicChange`
  variants), serde derives on the geometry types its items embed.
- **board items**: `board/**` (item structs, settings, `BoardDerived` with
  air wires, plane fragments and dirty sets, `Board::save()`),
  `mutation/board.rs`, `loader/board.rs`, `board/change.rs`, the air wires
  and plane fragments builders.
- **file format migrations**: `serialization/file_format_migration*.rs`
  (raw `SExpression`/file operations) plus the single call site marked
  `TODO(wave3b/migrations)` in `loader/mod.rs::open` (and the `library`
  open hook); no dependency on the item types.

Shared files (`mod.rs` re-exports, the top-level `Mutation`/`Change` enums)
already delegate to the per-area enums, so they need no edits. The
round-trip test (`tests/unittests/compat/project_roundtrip.rs`: open every project in
upstream `tests/data/projects`, save, every written file byte-identical)
stays green as the `raw` passthroughs are replaced by real serialization.

## 9. Crates

| Need | Choice |
|---|---|
| UUID-keyed, file-ordered collections | `std::collections::BTreeMap`/`BTreeSet` (no crate; `indexmap` not needed because file order is UUID order, not insertion order) |
| net segment cohesion, air wires | `petgraph` (already a dependency: `UnionFind` for connected components; `min_spanning_tree` already used by `AirWiresBuilder`) |
| plane fragments per layer in parallel | `rayon` (already in the lock file via dependencies; add to `librepcb-core` when porting the builder) |
| errors, thread-safety asserts | `thiserror`, `static_assertions` (existing) |
| application-level lock | `parking_lot::RwLock` in the editor/MCP crates, not in core |
| async notification | `tokio::sync::watch` in the app/MCP crates |
| `Mutation`/`Change` over the wire | `serde` derive, unconditional (core functionality, see "Serde representation"); `serde_json` as dev-dependency for the tests |
| Not used | `slotmap`/`generational-arena` (unstable keys, §1), `im`/`rpds` (§6), `arc-swap` (§6) |

## Serde representation

`Mutation`, `Change`, the identifiers and every model/value type they
contain derive `Serialize`/`Deserialize` unconditionally (MCP and CLI
clients speak this representation). JSON-friendly choices:

| Type | Wire form |
|---|---|
| `Uuid`, all `*Id` newtypes | string, e.g. `"d2c30518-5cd1-4ce9-a569-44f783a3f66a"`; map keys of `BTreeMap<*Id, _>` become JSON object keys |
| `Length`, `UnsignedLength`, `PositiveLength` | integer nanometers (`2540000`); constraints checked when deserializing |
| `Angle` | integer microdegrees (`90000000`) |
| `Point` | `{"x": <nm>, "y": <nm>}` |
| validated string newtypes (`ElementName`, `CircuitIdentifier`, `BusName`, `FileProofName`, `SimpleString`, `AttributeKey`), `Version`, `LengthUnit`, `Layer`, `AttributeType` | their string / identifier form (`"1.2"`, `"millimeters"`, `"top_cu"`, `"voltage"`), validated when deserializing (`utils::serde_string!`) |
| `Attribute` | `{"key", "type", "unit" (name or `null`), "value"}`, validated |
| `SerializableObjectList<T>` | plain array |
| `DateTime<Utc>` | RFC 3339 (chrono) |
| `SExpression` (check message approvals) | externally tagged tree (`{"List": {"name": .., "children": [..]}}`) |
| enums (`Mutation`, `Change`, `NetUse`, ...) | serde's default externally tagged form |

Integers instead of the file format's decimal strings because they are
exact, need no parsing rules on the wire and are what the model stores; a
client converting to millimeters divides by 10<sup>6</sup>. Non-persistent
state (`Board::derived`, the verbatim `raw` files) is skipped.

## Skeleton deviations from the sections above

- Project library elements are file backed (`TransactionalDirectory`) and
  not serializable, so adding/removing them is not a `Mutation` but the
  methods `Project::add_library_*()` / `remove_library_*()` (journaled as
  `Change::LibraryElement*`); an undo stack keeps the removed element
  itself.
- `Schematic` and `Board` own no `TransactionalDirectory`: the project
  writes `schematics/<dir>/` and `boards/<dir>/` on save and removes the
  directory from the transactional file system when a page/board is
  removed (upstream moves it to a temporary file system; the files are
  regenerated from the value on undo + save, byte-identical).
- `project/jobs.lp` is the typed `job::OutputJobList` (replaced as a whole
  by `Mutation::SetOutputJobs`, journaled as `Change::OutputJobs`);
  `Project::create()` does not copy the application's fontobene fonts yet
  (no resources directory port).
- Projects in an older file format are upgraded by the file format
  migrations (`serialization::file_format_migrations()`), followed by the
  ERC approval cleanup (`project::erc::run_erc()`), like upstream.
- `Project::create()`/`open()` reset the journal, so a fresh project has
  revision 0; `created` is truncated to seconds (the file format's
  precision).

## 10. Alternatives considered

- **Arena + generational keys (slotmap)**: fast lookups, but keys are not
  stable across undo/reload and would sit beside the UUIDs the files and
  MCP clients use; two identities for every entity. Rejected.
- **`Rc<RefCell>`/`Arc<Mutex>` object graph mirroring upstream**: violates
  the `Send + Sync`/no-interior-mutability rule and reproduces the
  registration bookkeeping the rewrite is meant to shed. Rejected.
- **`IndexMap` (insertion order) + sort on save**: would preserve an order
  nobody observes, at the cost of a sort per save and non-canonical
  iteration in algorithms. Rejected.
- **Mutations returning inverse closures** instead of data: not
  serializable, not inspectable, not `Send`; the command bus could not
  batch, coalesce or forward them to MCP. Rejected.
- **Observer callbacks (`Box<dyn Fn(&Change)>`)** instead of a journal:
  re-entrancy hazards during `apply`, needs `Send + Sync` closures in the
  model, and async consumers would need a channel anyway. A pull journal
  serves UI and MCP with one mechanism.
- **Cascading removal in the model** ("remove net = remove its segments"):
  simpler for callers, but makes inverses composite, diverges from upstream
  error behavior, and hides destructive edits from users. Composite
  operations belong to the command layer as `Batch`.
- **Storing pins/pads as objects** (upstream): most of the registration
  code and cache invalidation exists only to keep them in sync with the
  library; views computed from the library are simpler and always
  consistent.

## Open questions (with recommendation)

1. **Type names**: `SchematicSymbol`/`BoardDevice` (recommended) vs
   upstream-like `SiSymbol`/`BiDevice`. Files stay `si_*.rs`/`bi_*.rs`.
2. **Concurrency**: `RwLock` + extracted snapshots (recommended, §6) vs an
   owner-thread actor from the start. Both keep core unchanged; the actor
   can be layered later.
3. **Journal retention**: bounded ring of 4096 entries with `Resync`
   (recommended) vs unbounded until every registered cursor advanced
   (needs cursor registration API).
4. **Derived data placement**: inside `Board` as a non-persistent
   `derived` field (recommended, because DRC and Gerber export read air wires
   and plane fragments from the board like upstream) vs a separate
   `DerivedState` map keyed by `BoardId` on `Project`.
5. **`output_jobs` as raw `SExpression`** until `core/job` is ported
   (recommended: guarantees byte-identical round trip now) vs porting `job`
   first. (Done: `job` is ported, `output_jobs` is typed.)
6. **`serde` on `Mutation`/`Change`**: feature-gated derive in core
   (cheap, recommended once the MCP crate exists) vs hand-written DTOs in the
   MCP crate only.
