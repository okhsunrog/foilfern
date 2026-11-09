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

The native CLI can open projects and libraries, run checks and export
manufacturing data. The graphical application is still planned.

| Area | State |
|---|---|
| Base types, S-expression file format | done |
| Geometry, stroke fonts, polygon clipping (Clipper 1 port) | done |
| Attributes, air wires, math expressions | done |
| File I/O (transactional file system, directory locks, ZIP), networking | done |
| Libraries: symbols, packages, components, devices, categories, organizations, and their checks | done, check output identical to upstream |
| Gerber, Excellon, IPC-D-356A, BOM, pick & place generators | done |
| File format migrations (v0.1 → v1 → v2) | done |
| Project model: circuit, schematics, boards (with undoable mutations and a change journal) | done |
| Plane fragments, DRC, ERC, output job model and manufacturing runner | done |
| SVG, PNG and multipage PDF | symbols, footprints, schematics and 2D-board output jobs |
| Mechanical STEP | pure Rust BRep export, units and surface colors; native output jobs available |
| Native CLI | core commands, graphics and STEP jobs available; open-step/minification and full upstream acceptance pending |
| MCP server, library manager, Specctra export for autorouting | planned |
| Slint viewer and editor | planned (a rendering canvas already exists) |

See [docs/roadmap.md](docs/roadmap.md) for the milestones and
[docs/status.md](docs/status.md) for the current state and next steps.

## Repository layout

| Path | Contents |
|---|---|
| `crates/librepcb-cli` | Native command-line interface, without a display server |
| `crates/librepcb-core` | The domain model: types, serialization, geometry, libraries, project model, exports |
| `crates/librepcb-i18n` | Translations for strings from Rust code, using the upstream catalogs |
| `crates/librepcb-network` | Async networking (downloads, LibrePCB API) |
| `crates/librepcb-canvas` | 2D rendering canvas (vello_cpu) with hit testing, and a Slint adapter |
| `crates/librepcb-graphics` | Headless symbol, footprint, schematic and board graphics exports |
| `crates/librepcb-step` | Pure Rust CAD geometry, mechanical STEP and OBJ/MTL export |
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

To use the native CLI:

```sh
cargo run -p librepcb-cli -- open-project --erc --drc path/to/project.lpp
cargo run -p librepcb-cli -- open-project --run-job "Gerber/Excellon" path/to/project.lpp
cargo run -p librepcb-cli -- open-library --all --check path/to/library.lplib
```

Opening and checking do not save changes. Pass `--save` to persist edits or
file format upgrades. Both `.lpp` and `.lppz` projects are supported.
Run `scripts/check-cli-core.sh -q` for the unmodified upstream tests of the
implemented commands. Set `LIBREPCB_CLI` to the **official** executable when
running the Rust tests against the upstream oracle, since both binaries
are named `librepcb-cli`.

The complete upstream CLI suite is not passing yet: standalone STEP tools
and minification remain deferred. Help, version and parser diagnostics follow
clap instead of Qt. See `COMPAT.md` and `docs/status.md` for the exact boundary.

To see the canvas demo:

```sh
cargo run --release -p librepcb-canvas --features slint --example canvas_demo
```

Slint is pinned to upstream master commit
[`aebe3dd580ce0594de272bc5797b7444a2aa18fd`](https://github.com/slint-ui/slint/commit/aebe3dd580ce0594de272bc5797b7444a2aa18fd)
(2026-11-08, development package version 1.20.0). All Slint dependencies,
including the standalone prototypes under `spikes/`, use this revision.
The UI selects Slint edition 1.20; this snapshot requires Rust 1.95 or newer.
Check the optional integration explicitly:

```sh
cargo test --workspace --features librepcb-canvas/slint
cargo clippy --workspace --all-targets --features librepcb-canvas/slint -- -D warnings
```

See [docs/slint-tooling.md](docs/slint-tooling.md) for the runtime MCP tools
and a repeatable headless interaction/screenshot check of the Rust canvas.

## Documentation

- [docs/roadmap.md](docs/roadmap.md) — milestones
- [docs/porting-guide.md](docs/porting-guide.md) — conventions for porting upstream code
- [docs/project-model-design.md](docs/project-model-design.md) — design of the project model, mutations, undo and change tracking
- [docs/mcp-research-konnect.md](docs/mcp-research-konnect.md) — lessons from an existing KiCad MCP server, and our MCP decisions
- [COMPAT.md](COMPAT.md) — intentional differences from upstream

## License

GNU General Public License v3.0 or later, like upstream LibrePCB. See
[LICENSE.txt](LICENSE.txt).
