# ESP32-C3 schematic acceptance

This replay creates an empty native project, reads the published design document
over stdio MCP, reviews and applies it, and saves and reopens the result. It
compares all named net memberships, symbol and wire segment counts, notes and
explicit no-connect marks with [expected.json](expected.json). It also checks
undo/redo and replays the same document after a manual C3 annotation move.

Run from the repository root. Use a fresh output path and a cache containing
the UUID-named `.lplib` directories for LibrePCB Base, LibrePCB Connectors,
Espressif Systems and C&K. The replay links those libraries into a temporary
workspace and rescans it. `--upstream-cli` adds an independent ERC run on
the saved project.

```sh
cargo build --locked -p librepcb-mcp --bin librepcb-mcp
uv run docs/benchmarks/esp32-c3-schematic/replay.py \
  --binary target/debug/librepcb-mcp \
  --libraries "$LIBREPCB_TEST_LIBRARIES_DIR" \
  --output "$FOILFERN_REPLAY_DIR" \
  --upstream-cli "$LIBREPCB_CLI"
```

The output contains the actual JSON resource, a preview PNG, the native project,
and `report.json` with build identity, binary/document/library hashes, the
semantic result and per-call elapsed time. Library fingerprints hash sorted
relative file paths and each file's SHA-256, excluding `.git`. The report uses
designators, signal names and counts across fresh projects. Within a single
project it compares native UUIDs and full read-back across undo/redo and reopen.
The PNG is available for visual review.

The 2026-11-13 replay at `0d3838b` used API 21 and a debug build. It applied
83 changes in the initial preview. The result contains 23 symbols, 27 wire
segments, 13 notes, 14 nets and 5 no-connect markers. Foilfern and LibrePCB
2.1.1 reported zero ERC messages. After moving the C3 designator, the same
document applied zero operations and preserved the complete saved page and
netlist. This case covers schematic creation and editing. Board layout and
physical validation have separate acceptance cases.
