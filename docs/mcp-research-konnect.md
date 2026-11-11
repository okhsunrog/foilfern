# Lessons from Konnect (KiCad MCP server) for our MCP

Historical research for the original M1.5 milestone, from studying
[Konnect](https://github.com/mixelpixx/Konnect), an MCP server for KiCad
(Rust, ~226 tools in 21 toolsets, v0.12.1 at the time). Paths below are
relative to that repository. External-project details describe that research
snapshot, not a current audit of Konnect. Adopt/avoid lists below are the original
design input, not a claim that every proposed feature shipped.

## Current disposition (2026-11-11)

- Standalone and embedded hosts share `Arc<parking_lot::Mutex<OpenProject>>`,
  the editor undo stack and native project ownership. Both hosts are implemented.
- Schema-derived dispatch, structured errors, read-back, revision checks,
  library database search and rendered feedback are implemented; see the
  [current MCP contract](mcp-design.md).
- Reviewed routing has detached prepare/preview/apply, full model checks and
  exact retained patches. This is not a universal plan/apply API for every write.
- The board workflow is an embedded MCP resource. A client-specific skill
  installer, general observability ring buffer and persistent intent system are
  not established by these research notes.
- The server registers 79 tools at `f3c3fda`; the original under-60 aspiration
  did not hold. Avoid duplicate APIs and evaluate response size and usefulness.
- Build provenance, declarative updates and sourced persistent requirements now
  have concrete P0–P2 gates in the [roadmap](roadmap.md). Technical constraints
  should still reuse native rules where the semantics match.

## Konnect in short

- Hand-written MCP JSON-RPC over stdio and Streamable HTTP (no `rmcp`); skills,
  agents and PreToolUse hooks are installed into the Claude config by
  `konnect init`, not served as MCP prompts/resources.
- Three backends chosen per operation: text-preserving S-expression edits of
  `.kicad_sch` (KiCad has no schematic API), protobuf/NNG IPC to the PCB
  editor, and `kicad-cli` subprocesses for ERC/DRC/export/render. Most of the
  code is "write gate" logic (may the file be touched while KiCad owns it?)
  and re-deriving connectivity from geometry.

## Adopt (ranked)

1. **Toolsets with a tiny starter kit** (`router/registry.rs`,
   `router/meta_tools.rs`). Keep our surface small (< ~60 tools) and add
   dynamic loading only if needed; some clients cache `tools/list`.
2. **"Evidence, not echo" responses**: an `outcome` envelope
   (`complete|partial|failed|uncertain`, retry safety) and gate verdicts where
   a check that did not run can never read as a pass (`outcome.rs`,
   `gates.rs`, `docs/RELIABILITY_CONTRACT.md`).
3. **Structured error kinds** (`mcp/error.rs`): map `project::Error` variants
   to stable kinds plus the upstream message.
4. **Schema-enforced dispatch and drift tests**: generate input schemas from
   the serde DTOs (`schemars`); test that every advertised parameter is read
   and that skills only name existing tools
   (`crates/konnect/tests/schema_parameter_usage.rs`, `asset_references.rs`).
5. **Dry-run plan + apply with a revision token** (`tools/pcb_sync.rs`): our
   `Project::revision()` is the token; write tools take
   `expected_revision` and return the new one. Autorouter import, adding
   devices from the library and large batches become plan/apply pairs.
6. **Committed read-back**: write tools return the affected entities re-read
   from the model plus the `Change` events, not the echoed arguments.
7. **Observability tools**: recent calls ring buffer, per-tool stats, JSONL
   log, `server_info` (`observability.rs`).
8. **Visual feedback**: `render_schematic`/`render_board` returning MCP image
   content from `librepcb-canvas`; pin the rasterizer if baselines are stored.
9. **Skills and subagents**: a build agent (place → wire → annotate → save →
   ERC → render and look → fix), a review agent, "one design owner at a
   time", and an evidence hierarchy (requirements/datasheets > ERC/DRC >
   connectivity > heuristics) (`assets/skills`, `assets/agents`).
10. **Design preferences and free-text design rules** with explicit sources
    (`tools/config.rs`); map fab constraints onto LibrePCB's own
    `BoardDesignRules`/settings rather than a parallel format.
11. **Subcircuit templates as data** (`tools/templates.rs`), later.
12. **End-to-end stdio tests** that open, mutate, run ERC, export and diff
    against `librepcb-cli` (`crates/konnect/tests/e2e_kicad.rs`).
13. **FreeRouting through its own MCP/stdio mode** as an owned child process
    with plan/apply import (`freerouting_mcp.rs`).

## Avoid

- Micro-tools mirroring file syntax (`add_wire`, `add_junction`, plus
  `batch_*` duplicates); use intent-level, batch-native tools on top of
  `Mutation`.
- File-path arguments on every call and a stateless server; open once,
  address by UUID.
- Re-deriving connectivity from geometry; our netlist comes from the model.
- Duplicate entry points and aliases for the same operation.
- A backup that is not an undo (their `snapshot_project` is a PDF export);
  expose real `undo`/`redo`/`history`/`save`.
- Write-ahead transaction journals in the project directory; the in-memory
  model plus `TransactionalFileSystem` makes them unnecessary.
- Client-side hooks as the primary safety mechanism.

## Hard for them because of KiCad, and our position

- Live GUI vs saved file ambiguity: avoided if MCP runs inside the app on
  the shared `Arc<parking_lot::Mutex<OpenProject>>`; a standalone server must honor
  `DirectoryLock` (which stores user/host/PID, unlike KiCad's lock files).
- Formatting-preserving text edits: avoided, we have a canonical serializer.
- ERC/DRC via subprocess and JSON parsing: avoided, they run in-process.
- Invalid data reaching the editor: avoided, validation on every mutation.
- Library search by walking library tables: our SQLite workspace library index
  and project-library search are now implemented.

## Original decisions and implementation notes

1. **Process model**: one MCP crate, two hosts. First a standalone
   `librepcb-mcp` that owns the project and takes the upstream-compatible
   `DirectoryLock`; the same server is now embedded in the Slint app on the
   shared `Arc<parking_lot::Mutex<OpenProject>>`, so the user can watch edits.
2. **Undo**: one shared history with labeled groups; each agent tool call is
   one group ("AI: …") that the user can revert as a unit.
3. **Granularity**: intent-level, batch-native tools on top of `Mutation`,
   plus a raw `apply_mutation` escape hatch with a generated schema and full
   validation.
4. **Skills**: skill and agent files in this repository, installed by
   `librepcb-mcp init` (installer still planned). The client-independent
   board workflow is now embedded as `librepcb://guides/board-design`, a
   standard MCP resource available over stdio and HTTP without installation.
5. **Toolsets**: no dynamic loading. The original under-60 target is historical;
   current tools are listed in the MCP design and advertised schemas.
6. **Design rules**: technical constraints only in LibrePCB's own
   `BoardDesignRules`/settings (visible to upstream); free-text design intent
   in a separate project-associated representation (planned, not implemented).
   Preservation by upstream must be demonstrated before relying on that storage.
