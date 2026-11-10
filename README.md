# librepcb-rs

A work-in-progress rewrite of [LibrePCB](https://librepcb.org) — the free EDA
suite for schematics and PCBs — in Rust, with a [Slint](https://slint.dev) UI
and no Qt.

The goals:

- **File-compatible with upstream LibrePCB.** Everything upstream writes can be
  read, and everything this project writes can be read by upstream. Files are
  written byte-identically to upstream's canonical format.
- **Idiomatic Rust**, not a line-by-line transliteration: plain data models,
  no Qt emulation, existing crates wherever they fit.
- **Designing boards with AI agents.** An MCP server built on the same model
  as the editor, so agents can query and edit projects, run ERC/DRC and
  export manufacturing data, with every change validated and undoable.

This is an unofficial project and is not affiliated with the LibrePCB
project. All credit for the design, the file format and the original code
goes to the [LibrePCB developers](https://github.com/LibrePCB/LibrePCB).

## Built with AI agents

Most of the code in this repository was written by AI coding agents (Claude
Code) under human direction, porting the upstream C++ code module by module.
Treat it accordingly. What keeps it honest is a strict verification setup:

- the upstream unit tests are ported with their original test vectors;
- round-trip tests load every file in upstream's test data and require
  byte-identical output;
- where possible, results are compared directly against the upstream
  `librepcb-cli` (library checks, file format migrations, export generators).

Every intentional behavior difference from upstream is listed in
[COMPAT.md](COMPAT.md).

## Status

The headless core, a command line tool and an MCP server work; the Slint
application is next.

| Area | State |
|---|---|
| Base types, S-expression file format, geometry, stroke fonts, Clipper 1 port | done |
| Libraries (symbols, packages, components, devices, ...) and their checks | done, check output identical to upstream |
| File format migrations (v0.1 → v1 → v2) | done, byte-identical to upstream |
| Project model: circuit, schematics, boards (undoable mutations, change journal) | done |
| ERC, DRC | done, messages identical to upstream on all test projects |
| Plane fragments, Gerber/Excellon, pick & place, IPC-D-356A, BOM, JSON exports | done, byte-identical to upstream |
| Graphics export (PDF/SVG/PNG), output jobs | done (graphics not byte-identical to Qt) |
| `librepcb-cli` | native CLI, including standalone STEP validation/tessellation, minification and mechanical output jobs |
| Workspace, library index, library download | done |
| Editing commands with undo, autorouting (built-in router, Freerouting via Specctra DSN/SES) | done |
| MCP server | working: agents design schematics and boards end to end |
| Slint viewer and editor | implemented with embedded MCP; desktop platform acceptance remains separate |

See [docs/roadmap.md](docs/roadmap.md) for the milestones and
[docs/status.md](docs/status.md) for the current state and next steps.

## MCP server

`librepcb-mcp` lets an AI agent design a board through the
[Model Context Protocol](https://modelcontextprotocol.io): search parts in
the LibrePCB libraries, draw the schematic, place and route the board, run
ERC/DRC and export manufacturing data. The projects open in upstream
LibrePCB.

```sh
cargo install --path crates/librepcb-mcp
claude mcp add librepcb -- librepcb-mcp --workspace ~/LibrePCB-Workspace
```

Then ask for a design, e.g. "design a two-transistor LED blinker powered
from a 2-pin header, route it and export Gerbers". Libraries are installed
with the `library_install` tool (or copied to
`<workspace>/data/libraries/local/`). Autorouting uses
[Freerouting](https://github.com/freerouting/freerouting) when a
`freerouting` launcher or `LIBREPCB_FREEROUTING_JAR` (Java 25) is
available, else the built-in router. See `crates/librepcb-mcp/src/lib.rs`
and [docs/mcp-design.md](docs/mcp-design.md).

## Repository layout

| Path | Contents |
|---|---|
| `crates/librepcb-core` | The domain model: types, serialization, geometry, libraries, project model, exports |
| `crates/librepcb-i18n` | Translations for strings from Rust code, using the upstream catalogs |
| `crates/librepcb-network` | Async networking (downloads, LibrePCB API) |
| `crates/librepcb-canvas` | 2D rendering canvas (vello_cpu) with hit testing, and a Slint adapter |
| `crates/librepcb-render3d` | Physical board meshes and interactive offscreen Bevy 3D rendering |
| `crates/librepcb-scene` | Schematic/board/symbol/footprint scenes, PNG rendering and graphics export (PDF/SVG) |
| `crates/librepcb-editor` | UI-independent editing commands with undo, used by MCP and the future UI |
| `crates/librepcb-autoroute` | Built-in grid autorouter |
| `crates/librepcb-cli` | Port of the upstream `librepcb-cli` |
| `crates/librepcb-mcp` | MCP server for AI agents |
| `crates/clipper` | Pure-Rust port of the Clipper 1 polygon clipping library |
| `tools/ts2po` | Converter from upstream Qt `.ts` translations to gettext `.po` |
| `lang/` | Translation catalogs (25 languages) |
| `spikes/` | Throwaway prototypes (canvas rendering, Slint translations) |
| `docs/` | Roadmap, porting guide, project model design, MCP research |

## Building and testing

You need a recent stable Rust toolchain (edition 2024).

Many tests use upstream LibrePCB's test data, and some compare against the
upstream command line tool. `scripts/cloud-setup.sh` sets both up on a fresh
machine (Debian/Ubuntu packages, Rust if missing, the pinned upstream checkout
next to this repository, and the official `librepcb-cli` release run through
Xvfb):

```sh
scripts/cloud-setup.sh
```

Or check out upstream manually:

```sh
git clone https://github.com/LibrePCB/LibrePCB.git ../LibrePCB
git -C ../LibrePCB submodule update --init tests/data share/librepcb/fontobene i18n libs/fontobene-qt
```

The location is set by `LIBREPCB_UPSTREAM_DIR` in `.cargo/config.toml`
(default `../LibrePCB`) and can be overridden from the environment:

```sh
LIBREPCB_UPSTREAM_DIR=/path/to/LibrePCB cargo test
```

Then:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The file format migration test compares against the upstream command line
tool (same file format version), taken from `LIBREPCB_CLI` or `librepcb-cli`
on the `PATH`; without it, that comparison is skipped.

To see the canvas demo:

```sh
cargo run --release -p librepcb-canvas --features slint --example canvas_demo
```

To open a board's interactive 3D view:

```sh
cargo run -p librepcb-app -- --project /path/to/project.lppz --tab board3d
```

On Linux, 3D rendering needs a Vulkan loader and a compatible driver
(`libvulkan1` and `mesa-vulkan-drivers` on Debian/Ubuntu, installed by the setup
script). It also works without a display server: add `--screenshot view.png`.
On NixOS, the development shell must include `vulkan-loader` in
`LD_LIBRARY_PATH`. Slint can still use its software renderer. See
[the renderer notes](crates/librepcb-render3d/README.md) for controls and GPU tests.

## Documentation

- [docs/roadmap.md](docs/roadmap.md) — milestones
- [docs/porting-guide.md](docs/porting-guide.md) — conventions for porting upstream code
- [docs/project-model-design.md](docs/project-model-design.md) — design of the project model, mutations, undo and change tracking
- [docs/mcp-research-konnect.md](docs/mcp-research-konnect.md) — lessons from an existing KiCad MCP server, and our MCP decisions
- [docs/mcp-design.md](docs/mcp-design.md) — design of the MCP server and its tools
- [COMPAT.md](COMPAT.md) — intentional differences from upstream

## License

GNU General Public License v3.0 or later, like upstream LibrePCB. See
[LICENSE.txt](LICENSE.txt).
