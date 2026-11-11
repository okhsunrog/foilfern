# Routing reliability for MCP design workflows

The STM32G474 experiment on `c1101ac` established a useful end-to-end
workflow and exposed two routing-integration failures. Correctness and
ordinary board editing take priority over design-intent storage, templates,
additional exports and 3D work.

## Reproducible regressions

The original experiment has a frozen native project with its embedded
library elements, the MCP transcript and a manifest of file hashes and
element versions. The case is `stm32g474-c1101ac`, archive SHA-256
`a589d2364be89bb8cefd42ef6f9717379c38e5d5b6569285fcd636eb5f607197`.
It captures the final saved project, including later manual edits, rather
than claiming to be the initial unrouted board. The original experiment
and the snapshot remain outside the repository.

Small regressions use the local upstream populated-library fixture instead
of network searches or copied library data. With `LIBREPCB_UPSTREAM_DIR`
configured, run:

```sh
cargo test -p librepcb-editor autoroute_test
cargo test -p librepcb-mcp --features http routing::
```

The first suite reproduces the automatic/manual via representation problem
for drill 0.3 mm and size 0.6 mm, checks other dimension combinations,
net-class inheritance and DRC limits, and applies a real layer-changing
route. Undo/redo, the actual via geometry, full DRC and optional upstream
CLI acceptance are checked.

The second suite supplies actual SES candidates for a two-net board to
both direct import and the Freerouting completion path. At 0.15 mm width
against a 0.2 mm minimum, the applied candidate has zero remaining air
wires but two new width violations. It must return `partial`, with both
violations identified. A 0.25 mm candidate passes. This test does not need
Java or execute Freerouting; it isolates the import and validation boundary.
The existing external-router suite exercises the subprocess when installed.

`trace_update` changes widths of selected UUIDs or nets as one undo group,
preserving trace identities and anchors. Its regression scenario repairs
the thin SES candidate, then deliberately widens a trace into neighboring
copper. The second change must report new clearance errors and `partial`.
Invalid selections and stale revisions leave geometry and history unchanged;
repeating the same width does not add an undo entry.

## Applied-result contract

`autoroute` (both backends) and `specctra_import` return `result.validation`:

- `before` and `after`: actual unrouted count, unapproved DRC errors,
  copper errors, warnings, whether checks ran/completed, and check failures;
- `netlist_preserved`: component signal assignments and net identities
  stayed intact (physical shorts are checked by DRC);
- `drc_clean`: the final full DRC completed without unapproved errors;
- `acceptable`: both checks completed, the netlist stayed intact, and no
  new non-connectivity DRC errors appeared;
- new/resolved error counts, including a separate new-copper-error count,
  and at most 50 detailed new errors with kind, text and approval identity.

Both snapshots run full DRC on the project, rebuilding planes and air wires.
They do not add undo steps or discard the routing result. A result with
new errors, an altered netlist, or an incomplete check is `partial` and
stays available for inspection or one-step undo. Existing errors remain
visible and produce a warning; `complete` does not imply `drc_clean`.

Missing-connection errors are counted in the DRC totals and represented
by actual air wires, but excluded from the new-error identity comparison:
the air-wire tree can change as copper is added. Built-in `failed` still
counts unsuccessful router requests; `unrouted_after` counts the actual
board connections. For a filtered routing call, the operation's completion
still refers to the requested nets, while validation describes the whole board.

Error identity uses upstream approval expressions. Reimported objects with
new UUIDs can cause an existing physical violation to be newly reported;
the detailed identities and before/after counts make that conservative
comparison explicit. Do not infer a physical regression from counts alone.

## Next work

1. Extend the grouped trace-width editing with via-size and connected
   geometry edits, using one undo step and the same before/after evidence.
2. Add revision-bound preview/apply for expensive routing, device changes
   and large moves. Review the exact candidate before committing it.
3. Fix selected-device pin resolution before PCB placement, expose effective
   routing rules and their sources, and provide local DRC inspection images.
4. Improve placement and the router against frozen examples, including
   LQFP pad access and inner-layer routing, without losing DRC correctness.

PCBCraft's [track/via editing contract](https://github.com/tangency-labs/pcbcraft/blob/9a74b2fbd69c667c4de5bd546cfb8a58edec846e/docs/mcp.md)
and [session-owned update plans](https://github.com/tangency-labs/pcbcraft/blob/9a74b2fbd69c667c4de5bd546cfb8a58edec846e/crates/engine/src/board/update/commands.rs)
are useful references for the first two steps. Retain LibrePCB's shared
model and existing intent-level tools.
