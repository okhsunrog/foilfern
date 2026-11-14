# MCP command API implementation

Approved replacement contract, developed on `mcp-command-api` from `c799b93`.
This document tracks delivery; the current public contract remains in
[mcp-design.md](mcp-design.md) until the adapters switch together.

## Delivery

1. Shared direct dispatch for every existing operation, without a synthetic
   MCP peer or background service. Preserve the public interface for parity tests.
2. Typed command registry shared by CLI and MCP: canonical names, descriptions,
   generated schemas, requirements, effects, atomic eligibility and summaries.
3. Full result capture before journal/presentation truncation; persistent cache
   outside native projects, seven-day/1 GiB retention with oldest-first eviction,
   JSON Pointer/pagination/attachment access and explicit export. Cache failure
   after a committed edit falls back to a full response, never a false failure.
4. Switch MCP to seven fixed tools: server_info, list_commands, describe_command,
   query, execute, batch, read_result. No profiles or session-dependent catalog.
   Strict input validation; query refuses commands with possible write effects.
   Discovery returns 20 entries by default, at most 100, without bulk schemas.
5. Switch CLI to info/commands/describe/query/execute/batch/result. Version 1
   batches use named {id, command, params} steps and prior-step JSON Pointer
   references into full records. Old calls receive migration instructions.
6. Optional atomic batches for synchronous Transaction edits of one project:
   preflight every step; one undo group and host notice; exact rollback of model,
   overlay, journal, undo/redo and dirty state. External effects, lifecycle,
   save/export, undo/redo and candidate/job management are excluded.
7. Update instructions, workflow examples and migration documentation. Raise
   api_capability_version once for this branch, not once per intermediate commit.

Command names replace the first underscore with a dot. Bare-name mappings:
project.undo/redo/history, circuit.connect/disconnect/netlist, routing.autoroute,
board.unrouted, output.render. Old names remain searchable migration hints only.
Domain argument meanings, native files, revision freshness and host gates stay.

## Acceptance

Cover all existing operations, strict nested validation, transport/local parity,
HTTP and stdio discovery, preview freshness, cancellation, host refusal/delegation,
cache restart/eviction/failure, batch references and complete atomic rollback.
Target tools/list <=32 KiB compact JSON and ordinary summaries <=8 KiB excluding
images; retain complete details and genuine MCP image blocks.

Run 18 real Codex trials: three scenarios, three repetitions on each interface,
with the same model/settings, isolated projects and release builds. Scenarios:
offline exact-part acquisition and BOM; small schematic/board/check/export;
legend repair with stale-preview refusal and undo/redo/reopen. Limit each run to
10 minutes and 40 tool calls. Record actual tokens including cached input,
latency, calls, failures and independent verification; retain failed trials.
No automatic merge. Commit and push each completed validated logical step.
