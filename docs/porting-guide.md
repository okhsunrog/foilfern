# Porting guide

Conventions for porting upstream LibrePCB (`../LibrePCB`, C++/Qt) to
`librepcb-core` and sibling crates. `AGENTS.md` has the rules; this file shows
how they are applied in the existing code, so you don't have to rediscover
them.

## Module layout

- One module per upstream directory: `libs/librepcb/core/geometry/` →
  `crates/librepcb-core/src/geometry/`. Top-level upstream files get a
  top-level module (`sqlitedatabase.cpp` → `sqlite_database.rs`).
- One file per upstream class, snake_case with word separators, named
  after the Rust type or concept without prefixes or abbreviations the
  module path already implies: `stroketextpathbuilder.cpp` →
  `stroke_text_path_builder.rs`, `serializableobjectlist.h` →
  `serializable_object_list.rs`, `project/schematic/items/si_symbol.cpp` →
  `project/schematic/symbol.rs` (type `SchematicSymbol`),
  `project/board/items/bi_device.cpp` → `project/board/device.rs`
  (`BoardDevice`), `projectloader.cpp` → `project/loader/`. The upstream
  file is named in the `//! Port of ...` module doc, which keeps the
  traceability. Exceptions are proper names (`fontobene.rs`) and `fileio`;
  modules from earlier waves keep their names.
- `mod.rs` declares private submodules and re-exports the public types
  (`pub use circle::{Circle, CircleList, CircleListTag};`), so users write
  `librepcb_core::geometry::Circle`. Each module has one `error.rs`.
- Every file starts with `//! Port of libs/librepcb/core/<dir>/<file>.{h,cpp}.`
  followed by the differences to upstream (see `sexpression.rs`).
- Async code (network, MCP, AI) lives in its own crate; `librepcb-core`
  stays synchronous.

## Porting a class

Good examples to copy from:

- `src/geometry/circle.rs`: a plain-data model object with getters/setters
  (`property!`), UUID handling (`impl_uuid!`), a list type (`object_list!`)
  and (de)serialization.
- `src/types/element_name.rs` + `src/types/string_newtype.rs`: a validated
  newtype replacing a `type_safe::constrained_type<QString, ...>`.
- `src/types/length.rs`: a numeric value type with exact integer semantics,
  constrained wrappers (`PositiveLength`) and operators.

Checklist:

1. Read the `.h` and `.cpp` (and upstream `rust-core` if it has the logic).
2. Model objects are plain data: private fields, getters named after the
   field (no `get_`), setters `set_x()` returning whether the value changed,
   and `// upstream: emits onEdited(...)` where upstream emits a signal.
3. Constructors: `new(...)` for infallible ones, `Result`-returning ones
   for validated input, `with_uuid()` instead of copy constructors taking a
   UUID, `Clone` instead of `operator=`.
4. `type_safe` constraints become newtypes with a fallible `new()`,
   `TryFrom`, `FromStr` and `get()` for the inner value (like
   `NonZeroU32::get()`).
5. Enums instead of string/int tags; `FromStr`/`Display` for their file
   tokens.
6. Port the upstream unit tests (see "Tests").
7. Assert thread safety of top-level aggregates and of types with interior
   mutability, caches or handles, next to the type definition:
   `static_assertions::assert_impl_all!(Board: Send, Sync);` (see
   `transactional_file_system.rs`, `stroke_font.rs`, `pad.rs`). Plain-data
   leaf types don't need it (auto traits propagate).

## Serialization

`src/serialization/mod.rs` explains the two trait pairs:

- Values (one node): `ToSExpression` / `FromSExpression`, e.g. `Length`,
  `Uuid`, enums, string newtypes.
- Objects (content of a list node): `SerializeObject::serialize(&self, root:
  &mut List)` / `DeserializeObject::deserialize(node: &SExpression)`.

Writing mirrors the upstream `serialize()` call by call (the order and line
breaks define the bytes): `root.append_value(&uuid)`,
`root.append_child("layer", &layer)`, `root.ensure_line_break()`,
`point.serialize(root.append_list("position"))`. Lists of objects are
`SerializableObjectList<T, Tag>` (`object_list!`), which writes one
`(tag ...)` per element.

Reading: `node.child_value::<T>("layer/@0")?` for values,
`Point::deserialize(node.required_child("position")?)?` for objects, and
`node.child("junction")` for optional children. Older file formats will be
upgraded on the raw `SExpression` tree by the (not yet ported) file format
migrations before deserialization, so deserializers only handle the current
format.

Round-trip test pattern (`tests/unittests/geometry/mod.rs`): serialize,
deserialize, serialize again and compare the strings (`assert_roundtrip()`);
plus a test deserializing the upstream test vector string. Real files are
covered by `tests/unittests/compat/geometry_roundtrip.rs` and
`compat/sexpression_roundtrip.rs`, which re-serialize everything in the
upstream `tests/data` and require byte-identical output. Add new object types there. Library elements are
covered by `compat/library_roundtrip.rs`, which opens and saves every
current-format element (add new element types there).

## Errors and translations

- One `thiserror` enum per module (`geometry/error.rs`, `types/error.rs`),
  `#[non_exhaustive]`, one variant per upstream exception site, and a
  module `Result<T, E = Error>` alias where the module has many fallible
  functions. Use `#[from]` for nested module errors (a `types::Error` can be
  `?`-propagated into `serialization::Error`).
- Messages are the upstream strings. Strings upstream translates use
  `librepcb_i18n::tr!` with the upstream context (C++ class name, or the
  context lupdate recorded):
  `#[error("{}", tr!("Length", "Value must be > 0!"))]`. Replace `%1`, `%2`
  by `{0}`, `{1}`; `{{`/`}}` are literal braces. Plurals: `trn!` with `{n}`.
  The context is the `msgctxt` of `lang/librepcb.pot`, including the
  namespace for classes outside the `librepcb` root namespace
  (`librepcb::editor::CmdBoardEdit`, `librepcb::eagleimport::...`). The
  test `context_check` in `librepcb-i18n` fails for contexts or source
  strings missing from the catalog; genuinely new strings go into its
  allow-list.
- Untranslated upstream strings (logic errors, debug messages) stay plain
  `#[error("...")]`.
- No panics on file or user input. `expect()` only for real invariants, with
  the reason as message.

## Naming

- Getters are nouns without `get_`: `uuid()`, `layer()`, `vertices()`.
  Predicates `is_x()`/`has_x()`, setters `set_x()`.
- Conversions: `to_x()` (cheap, by value/ref), `into_x()` (consuming),
  `as_x()` (borrowed view), `from_x()` constructors, `FromStr` for parsing.
- One variant per operation. For `Copy` values, only the value-returning
  form (`rotated()`, `mirrored()`, `mapped_to_grid()`); callers write
  `p = p.rotated(...)`. In-place methods only for large objects where
  copying matters.
- Operators only where dimensionally meaningful: `Length ± Length`,
  `Length * i64`, `Length / i64`, `Length % Length`, `Point ± Point`,
  `Angle ± Angle`, `Ratio ± Ratio`. No `Length * Length`, no comparisons
  with bare integers (`len > Length::ZERO`, not `len > 0`).
- Upstream renames: `tryGetChild()` → `child()`, `getChild()` →
  `required_child()`, list `find(uuid)` → `by_uuid()`, list `get(uuid)` →
  `required_by_uuid()`, `Layer::get(id)` → `Layer::from_id()` / `parse()`.

## Lookups: Option vs Result

- A lookup returns `Option` and is named after what it returns:
  `SExpression::child(path)`, `child_mut(path)`, `list.by_uuid(uuid)`,
  `list.by_name(name, case_sensitive)`, `map.get(key)`, `Layer::from_id(id)`.
- Only where a missing element is a *data error* that callers would
  otherwise have to build by hand (deserialization, references in files),
  add one `Result` variant with the `required_` prefix:
  `required_child(path)`, `required_by_uuid(uuid)`, `required_by_name(name)`.
  No `get_`/`try_get_`/`find_` pairs.
- `child_value::<T>(path)` deserializes a required child. For an optional
  one: `node.child(path).map(T::from_sexpression).transpose()?`.
- Parsing a string into a type is `FromStr` (returns the module error);
  fallible numeric constructors return `Result` (`Length::from_mm()`,
  `Angle::from_rad()`), callers use `.ok()` if they need an `Option`.

## Qt semantics and COMPAT.md

- No Qt emulation: `char`/`str` instead of UTF-16, `str` ordering,
  `char::is_whitespace()`/`str::trim()` (same set as `QChar::isSpace()`),
  `str::parse()` instead of `QString::toInt()` & co.
- Where a Qt rule decides which files are valid, reproduce the accepted set
  natively and say so in a comment (e.g. `ElementName`: "max 70 chars, BMP
  only" instead of "70 UTF-16 code units").
- Floating point: basic arithmetic and `sqrt` native, every transcendental
  function (`sin`, `atan2`, `hypot`, ...) from `libm`, for identical results
  on all platforms. Integer nanometer rounding must match upstream exactly.
- `COMPAT.md` (workspace root) lists every divergence from upstream,
  grouped by module ("General" for crate-wide rules), including exotic
  edge cases. Part 1 holds intentional differences (design decisions, not
  work items), Part 2 features not ported yet (work items, removed when
  done). Add an entry whenever behavior differs, even if no realistic file
  is affected, and say whether file output can change.

## Tests

- Ported gtests go to
  `crates/librepcb-core/tests/unittests/<module>/<name>_test.rs` (one
  integration-test binary per crate, declared in `tests/unittests/main.rs`;
  other crates follow the same pattern, e.g.
  `librepcb-network/tests/network/main.rs`), keeping the upstream test
  vectors and names in snake_case
  (`testSerializeAndDeserialize` → `test_serialize_and_deserialize`).
  Parametrized suites become loops over data tables.
- Small tests of private helpers go into `#[cfg(test)] mod tests` in the
  source file.
- Test data is never copied: reference it through the upstream checkout
  location `LIBREPCB_UPSTREAM_DIR`, which `.cargo/config.toml` sets to
  `../LibrePCB` (relative to the workspace root) unless the environment
  variable is already set:
  `Path::new(env!("LIBREPCB_UPSTREAM_DIR")).join("tests/data")`.
  Don't hardcode paths relative to `CARGO_MANIFEST_DIR`.
- Shared helpers: `tests/unittests/helpers.rs` (temp dirs, dummy process),
  `tests/unittests/geometry/mod.rs` (`parse()`, `assert_roundtrip()`).
- Before finishing: `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.
