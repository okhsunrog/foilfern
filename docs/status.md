# Status and handoff

Reviewed 2026-11-11 against `f3c3fda`, including `work` through `33c4dfb`.
This is a capability summary, not a claim that every listed check was rerun on
that commit. Read `AGENTS.md` for repository rules and [roadmap.md](roadmap.md)
for the next delivery stages and their acceptance criteria.

## Documentation map

| Question | Owning document |
| --- | --- |
| What works, what remains, where to continue? | This status page |
| What should be built next, and what counts as done? | [Roadmap](roadmap.md) |
| How are identity, mutations, history and snapshots represented? | [Project model](project-model-design.md) |
| What can an MCP caller use today? | [MCP contract](mcp-design.md), runtime `tools/list`, [board workflow](mcp-board-workflow.md) |
| How are routing candidates prepared, checked and applied? | [Routing service](routing-service-design.md) |
| What does a reviewed board require? | [Agent PCB workflow](agent-pcb-workflow.md) |
| What has been measured on real boards? | [Routing replays](mcp-routing-reliability.md), [ESP-PD-Sink index](benchmarks/esp-pd-sink/HANDOFF.md) |
| What differs from upstream or is still missing? | [COMPAT.md](../COMPAT.md) |
| Why were the original choices made? | [Konnect research](mcp-research-konnect.md), [historical validation](history/port-validation.md) |

Keep current contracts here and in their owning documents. Historical reports
retain their original inputs, limitations and measurements; they do not define
the next task merely because their last paragraph once said "next".

## Implemented foundation

- **Native model and compatibility:** UUID-keyed circuit, schematic and board
  data, validating mutations with inverses, reverse indices, change journal,
  transactional files and migrations from v0.1 through v2. Upstream fixtures
  and the optional upstream CLI are the round-trip/check/export oracles.
- **Core checks and manufacturing:** ERC, board DRC, plane rebuilding, Gerber,
  Excellon, pick-and-place, Gerber X3, IPC-D-356A, BOM, project JSON and output
  jobs. Graphics, mechanical STEP and interactive HTML BOM use the shared
  exporter adapters from CLI, desktop and MCP. Project BOM without a board
  deliberately reports `Package=N/A`; select a board for footprint packages.
- **Shared editing:** `ProjectEditor` owns commands and undo. Desktop and MCP
  share `Arc<Mutex<OpenProject>>`; a directory lock protects native files.
  Library additions/removals and assets have editor operations alongside
  model mutations. Preview uses detached state, not live apply-and-undo.
- **Libraries and import:** SQLite workspace index, official-library installation,
  project library management, library element editors/checks, EAGLE library and
  project import, and KiCad library import. MCP also imports flat KiCad schematics
  and board placement/outline/zones/supported rules for rerouting. Full routed
  KiCad PCB and hierarchical schematic import remain outside that contract.
- **Desktop:** Slint schematic/board editors, tool FSMs, dialogs, library tabs,
  workspace/project lifecycle, shared live MCP editing, rule-check panels,
  output jobs and read-only `.lppz` opening. Detailed UI gaps remain in COMPAT.md.
- **Rendering/CAD:** incremental 2D scenes, realistic board graphics, pure-Rust
  STEP import/export/minification and a Bevy board 3D viewer. Camera, materials
  and edit/undo refresh are wired. Slint presents GPU readback images; shared
  textures and package-editor 3D are not implemented.

These subsystems were delivered under the historical M1–M5 porting labels.
Their remaining polish and compatibility gaps do not reset those milestones.

## Routing and measurements available now

The editor owns one `RoutingService` per open project; MCP is its transport.

- Detached preparation, exact paginated mutation preview, revision/context-bound
  apply, discard and one undo step. Storage holds at most eight candidates and
  refuses capacity overflow; concurrent workers are not bounded by this cap.
- Builtin order comparison, diagnostics with local blockers and failed-search
  evidence, pad escapes, short neck-downs, critical-net ordering, optional
  congestion history, authorized regional rip-up and local via-site search.
- Connected via/junction edits, path replacement, explicit copper batches and
  moves/rotations of unlocked devices with their copper in one candidate.
  A rejected candidate can be extended with a checked repair via `base_candidate`.
  Coordinates are supplied explicitly; bounded placement planning and connected
  side changes are not implemented.
- Full before/after board checks, per-net connectivity regressions/improvements,
  new warnings and `connectivity_progress` separate from `acceptable`.
  An acceptable partial or unchanged candidate is not proof of target completion.
- Plane diagnosis/stitch candidates and geometric reference-centerline coverage.
  Coverage is not an impedance or signal-integrity check.
- One reusable complete source-check baseline bound to the session, project,
  board, revision and save/path context. Derived copper is rebuilt; candidate
  checks and non-empty applied-model checks remain fresh.
- Read-only `trace_path_measure`: resolved physical pads, shortest anchored
  centerline, trace/layer/width/via identities. Plane paths, unanchored contacts
  and via-barrel lengths are excluded. An unavailable trace-only length is
  `not_measured`, not zero length or a connectivity failure.

Current limitations: no declarative reconciliation API, persisted engineering
requirements, automatic diagnosis-to-repair planner, MCP progress/cancellation
workflow or desktop candidate overlay. The router's cancellation callback is
not yet wired through the editor/MCP preparation path. `server_info` now reports
build-time commit, tracked-source state, Cargo profile label and API capability
version. `device_place` supports compact responses with `include_pads: false`.
The timing protocol remains open in P0.

## Evidence and continuation points

| Case | Recorded result | Limit / next use |
| --- | --- | --- |
| [STM32 local via](mcp-routing-reliability.md#local-via-search) | Automatic bounded via-site search closes the preserved U1.21 gap; undo/reopen/upstream checks recorded | Ordinary builtin routing still misses this case; use it for strategy selection |
| [Two-layer ESP32-C3](mcp-routing-reliability.md#two-layer-esp32-c3-dev-board-completion) | Five checked patches close all six gaps; zero ERC/DRC errors and 13 inherited silkscreen warnings; RF/crystal geometry preserved | External scratch search helped choose coordinates; 79 vias and 1096.493 mm versus 71 and 852.307 mm initially; not an automatic planner result |
| [ESP-PD-Sink selected layout](benchmarks/esp-pd-sink/review/gate/REPORT.md) | OLED, U5, U2, Kelvin, ADC and gate-clamp review batches; zero open connections, three inherited errors and nine warnings | Continue PCB review from the gate-clamp archive; mechanical/silkscreen and remaining engineering findings are not closed |
| [Release baseline reuse](benchmarks/esp-pd-sink/review/performance/release/REPORT.md) | Repeated OLED prepare: 25.598 → 13.915 s; full validation and normalized geometry agree | Small samples; first preparation and apply are not accelerated |
| [Competing builtin trial](benchmarks/esp-pd-sink/review/performance/release/BUILTIN.md) | Repeated prepare: 26.553 → 14.667 s; both versions reject a new plane-fed GND break | DRC dominates this local workload; no A* speedup established |
| [Path API](benchmarks/esp-pd-sink/review/path-api/REPORT.md) | All 18 saved pairs replayed; 17 match prototype lengths, one is explicitly unmeasured | Requirements/groups remain external prototypes, not enforced project objects |

Use the original ESP-PD-Sink `inputs.tar.gz` for performance comparisons and the
selected gate-clamp project for further layout work. They are different snapshots.
Do not substitute one for the other in a before/after benchmark.

The immediate product sequence is P0 observability, P1 declarative updates,
P2 requirements, P3 bounded workers, P4 local planning and P5 visual review.
P6 defines end-to-end evidence, including a held-out board. See the roadmap for
exact dependencies and acceptance rather than adding another task list here.

## Validation scope

The last local merge checks on `33c4dfb` passed six routing-service unit tests and
32 routing-candidate MCP integration tests with the configured upstream checkout,
plus formatting. The Nix addition at `f3c3fda` was evaluated for x86_64-linux and
aarch64-linux; shell/resource/library checks and `cargo check -p librepcb-i18n`
ran on x86_64-linux. This was not a full workspace or GUI-in-Nix acceptance run.

Earlier full-workspace, upstream CLI, GPU and desktop evidence is in
[historical validation](history/port-validation.md). Do not carry its test counts
forward as current results. Real-board timing and output evidence belong to
the linked reports and their exact commits, not to this documentation refresh.

## Environment and development

On Linux with Nix, run `nix develop`. The lock pins Rust, native development
libraries and upstream source/test resources. `LIBREPCB_UPSTREAM_DIR` defaults
to the prepared Nix store source; an existing value selects a custom checkout.
The upstream CLI oracle and Freerouting remain optional external tools. See
[README](../README.md#build-from-source) and [the porting guide](porting-guide.md).

On a fresh Debian/Ubuntu environment, `scripts/cloud-setup.sh` prepares the
pinned upstream checkout, CLI and Freerouting. Optional oracle comparisons may
skip when these tools are unavailable; report that rather than claiming they
passed. `LIBREPCB_TEST_LIBRARIES_DIR` selects installed official library clones
for tests that require them. Network availability is environment-specific; the
library installer can fall back from ZIP download to Git.

Use release binaries for real boards, routing, DRC and timing; restart the MCP
process after rebuilding. Match profile, snapshot and request across comparisons.
Focused correctness tests use the normal dev profile. Keep the configured Cargo
cache/output paths; temporary copies and diagnostics follow AGENTS.md.

The dev profile uses line tables for workspace crates and no dependency debug
info. Each crate has one integration-test binary to limit linking/storage. For
full debugging, use `CARGO_PROFILE_DEV_DEBUG=full` and, if needed,
`--config 'profile.dev.package."*".debug="full"'`.

## Maintenance rules

- Update the owning API/design document with a behavioral change and link its
  acceptance evidence. Move completed roadmap items only when their gates pass.
- Keep intentional upstream divergences in COMPAT.md Part 1 and real remaining
  gaps in Part 2; remove resolved gaps instead of appending contradictory notes.
- Domain behavior stays in core/editor. MCP and GUI share operations and history.
  Core remains synchronous and its model types remain `Send + Sync`.
- New Rust user-visible strings use the existing translation contexts. Custom
  `.slint` `@tr()` additions are not covered by the Rust `context_check` test.
- Use the current branch's commit/push policy and dedicated temporary root from
  AGENTS.md. Historical porting waves do not prescribe how every task is staffed.
