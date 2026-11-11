# librepcb-rs

Rewrite of LibrePCB (C++/Qt) into idiomatic Rust with a Slint UI and no Qt
dependency. The upstream C++ code is the behavioral reference; the goal is
equivalent behavior and byte-identical file formats, not a line-by-line
transliteration.

File-level interoperability with upstream LibrePCB is a hard requirement: we
read every file upstream writes, and upstream reads every file we write
(projects, libraries, workspace data). Internals may diverge freely.

Read before working:

- `docs/porting-guide.md`: conventions and example files for porting.
- `docs/project-model-design.md`: the project model (UUID-keyed data tree,
  `Mutation`/`apply()` with inverses, change journal). Required reading for
  anything under `project/`.
- `docs/status.md`: current state, next steps and how the work is organized.
- `docs/roadmap.md`: milestones.
- `docs/mcp-research-konnect.md`: MCP decisions.
- `COMPAT.md`: differences from upstream (Part 1: intentional, Part 2: not
  ported yet).

## Upstream reference

The upstream checkout (`https://github.com/LibrePCB/LibrePCB` with the
submodules `tests/data`, `share/librepcb/fontobene` and `i18n`) is located via
`LIBREPCB_UPSTREAM_DIR`, set in `.cargo/config.toml` (default `../LibrePCB`,
overridable by the environment). Code and tests use
`env!("LIBREPCB_UPSTREAM_DIR")` or the test helpers (`test_data_dir()`);
never hardcode paths like `../../../LibrePCB`.

The upstream command line tool `librepcb-cli` (same file format version) is
the oracle for behavior that tests can compare against: file format
migrations, library checks, exports. Tests take it from `LIBREPCB_CLI` or the
`PATH` and skip the comparison if it is missing.

## Builds for board work and timing

Use a release build of Foilfern for real-board routing, DRC runs and
performance measurements. Build the binary the workflow uses, for example:

```sh
cargo build --locked --release -p librepcb-mcp --bin librepcb-mcp
```

Run `target/release/librepcb-mcp` (under the configured target directory
if overridden), and restart the MCP process after rebuilding it. Use
ordinary dev builds for focused correctness tests and debugging. Record
the commit, binary path and Cargo profile in performance reports. Compare
code changes using the same build profile, board snapshot and request.

## Layout

- `crates/librepcb-core`: domain model (port of `libs/librepcb/core`), one
  module per upstream directory (`types`, `serialization`, `geometry`,
  `library`, `project`, `export`, ...).
- `crates/librepcb-i18n`: runtime for translating strings from Rust code.
- `crates/librepcb-network`: async networking (tokio, reqwest).
- `crates/librepcb-canvas`: 2D rendering canvas (vello_cpu, rstar, kurbo) with
  an optional Slint adapter.
- `crates/librepcb-graphics`: vector/raster document export and realistic 2D board output.
- `crates/librepcb-step`: pure-Rust CAD import, mechanical STEP output and document minification.
- `crates/librepcb-render3d`: physical board meshes and offscreen Bevy PBR rendering.
- `crates/clipper`: pure-Rust port of Clipper 1.
- `crates/librepcb-scene`: schematic/board scene builders on the canvas and
  headless PNG rendering.
- `crates/librepcb-autoroute`: built-in grid autorouter (original code, no
  upstream counterpart).
- `crates/librepcb-import`: EAGLE and KiCad library import and EAGLE project
  import (port of `libs/librepcb/{eagleimport,kicadimport}`).
- `crates/librepcb-mcp`: MCP server (rmcp; binary `librepcb-mcp`, stdio or
  streamable HTTP with the feature `http`), see `docs/mcp-design.md`.
- `tools/ts2po`: converter from Qt `.ts` to gettext `.po`.
- `lang/<lang>/LC_MESSAGES/*.po`: translation catalogs.
- `spikes/`: throwaway prototypes, excluded from the workspace.

## Rules

- Rust 2024 edition, workspace lints (`unsafe_code = "forbid"`, clippy
  warnings). Code must pass `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace`.
- Add dependencies only with `cargo add -p <crate> <dep>` so versions are
  latest. No Qt, no C++ FFI.
- Prefer existing crates over hand-written code whenever one fits (parsers,
  geometry, triangulation, formats, HTTP, zip, CSV, hashing, ...). Search
  crates.io first; choose maintained, widely used crates. Write code by hand
  only when no crate reproduces the upstream behavior that the tests or
  byte-identical file output require, and say so in the module doc.
- Each ported module starts with a doc comment naming its upstream source,
  e.g. `//! Port of libs/librepcb/core/types/length.{h,cpp}.`
- Port upstream unit tests from the upstream `tests/unittests/` alongside the
  code, keeping the same test vectors. Test data is never copied into this
  repository; reference it through `LIBREPCB_UPSTREAM_DIR`.
- One integration test binary per crate: `tests/<name>/main.rs` with one
  module per topic (`tests/unittests/main.rs` in core, `tests/app/main.rs`
  in the app, ...). Never add another `tests/*.rs` file or test directory:
  each test binary links the whole dependency tree again (up to 500 MB on
  disk). Tests needing process-wide state share it within the binary (e.g.
  the app's UI thread with Slint's headless platform,
  `common::with_headless()`). Binaries without unit tests set `test = false`.
- Errors: one `thiserror` enum per module/area, `Result` everywhere. No panics
  on user/file input; `unwrap`/`expect` only in tests or for true invariants
  with a comment explaining why.
- Idiomatic over literal: newtypes with validated constructors instead of
  `type_safe` wrappers, enums instead of string/int tags, traits instead of
  inheritance, `Option` instead of null, iterators instead of index loops.
  Drop Qt-isms (QString → `String`/`&str`, QList → `Vec`).
- No Qt emulation. Do not reproduce `QString`/`QChar`/UTF-16 semantics (UTF-16
  lengths or ordering, `QChar::isPrint`/`isSpace`), Qt number parsing, or
  `QLocale` behavior. Use Rust-native semantics: `char`, `str` ordering,
  `str::parse`, `char::is_whitespace`, `String::from_utf8_lossy`. Exception:
  when a Qt rule changes which realistic files are accepted, or which bytes
  are written (including check messages whose approvals are stored in
  files), express or port that rule and say why. Record every intentional
  divergence from upstream, including exotic edge cases, in `COMPAT.md`
  Part 1, and missing features or behavior still to be matched in Part 2
  (remove Part 2 entries when done). Part 1 entries are design decisions,
  never TODO items.
- Rust naming, not C++ naming:
  - file/module names describe the Rust type or concept, not the upstream
    file: snake_case with word separators, no upstream prefixes or
    abbreviations that the module path already implies
    (`project/board/device.rs`, not `project/board/items/bi_device.rs`). The
    upstream file is named in the `//! Port of ...` module doc;
  - getters without a `get_` prefix;
  - `Option` returns instead of get/tryGet pairs, with a `required_` variant
    returning `Result` only where a missing item means bad file data;
  - no in-place + copy method pairs unless both are really needed;
  - no operator overloads that make no sense dimensionally (e.g.
    `Length * Length -> Length`).
- Numeric semantics must match upstream exactly (integer nanometers, rounding
  modes, overflow checks); file output depends on them. Transcendental
  functions come from the `libm` crate for cross-platform determinism.
- User-visible strings go through `librepcb_i18n::tr!` using the upstream Qt
  translation context (the C++ class name) as context and `{0}`, `{1}`
  placeholders instead of Qt's `%1`, `%2`, so existing translations keep
  matching. `tr!` always formats (`{{`/`}}` are literal braces, `{}` takes the
  next argument); use `trn!` for plurals with `{n}`. Never use an empty
  singular in plurals (`@tr("" | "{n} x" % n)` breaks gettext); repeat the
  plural text.
- Model objects are plain data without Qt-style signals or observer
  callbacks. Project changes go through `Mutation`s and are reported by the
  project's change journal (see the project model design).
- Upstream already has some Rust in `libs/librepcb/rust-core` (zip
  read/write, Length/Angle/Point/Vertex, toolbox, math). Reuse its logic where
  it applies instead of re-deriving it from the C++.
- Keep core synchronous: no async runtime in `librepcb-core` (it is CPU/file
  bound). Long core operations run on caller-provided threads and report
  progress via callbacks or channels.
- Networking, MCP and AI integration are async (tokio) and live in their own
  crates, calling core via `spawn_blocking` where needed.
- Model types must be `Send + Sync`: no `Rc`, `RefCell` or `Cell` in core
  model objects. Assert it with `static_assertions` next to top-level
  aggregates and types with interior mutability, caches or handles.
- `Mutation`, `Change` and the types they contain derive serde
  `Serialize`/`Deserialize` unconditionally (JSON is the MCP wire format).
- Commit and push each completed logical step to the current branch's upstream
  by default. Parallel porting agents work in separate git worktrees and
  leave their changes for the coordinating agent to review.
- Prefer focused tests of the affected code. Run expensive suites when the
  scope or evidence calls for them.
