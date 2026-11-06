# Intentional divergences from upstream LibrePCB

Behavior differences of the Rust port compared to the C++ reference
(`../LibrePCB`), including exotic edge cases, usually because upstream relies
on Qt semantics that are not reproduced. File formats are unaffected unless
stated otherwise. Entries are grouped by module.

## General

- **Whitespace**: `char::is_whitespace()` / `str::trim()` /
  `str::split_whitespace()` replace `QChar::isSpace()` /
  `QString::trimmed()` / `QString::simplified()`. Both use the same set
  (Unicode White_Space = categories Zs, Zl, Zp plus `\t`..`\r` and U+0085),
  so results are identical apart from Unicode version differences.
- **String ordering** is `str` ordering (code points) instead of `QString`
  ordering (UTF-16 code units). The two only differ when comparing
  characters outside the Basic Multilingual Plane with U+E000..U+FFFF.
  Affected: `SExpression`'s `Ord` impl, `PcbColor::all()` (sorted by
  translated name), and the file lists noted under fileio below.
- **Transcendental functions** (`sin`, `cos`, `tan`, `acos`, `atan`,
  `atan2`, `hypot`, the math parser functions) come from the `libm` crate in
  `librepcb-core` and `clipper`, so results are identical on all platforms.
  Upstream calls the platform C library (glibc, MSVC CRT, ...) except for
  the functions it routes through its own Rust code (`Point::rotated()`,
  arc radius/center), which already use `libm`. `libm` differs from glibc
  in the last bit for some inputs (1–18% of random inputs, depending on the
  function); none of the test vectors captured with glibc/upstream (Clipper
  golden vectors incl. round and square offsets, upstream arc flattening
  and geometry tests, round-trip files) changed after rounding to integer
  coordinates. A different result
  would require an intermediate value within one ulp of a rounding
  boundary. `sqrt` and basic arithmetic stay native (correctly rounded
  everywhere). Exception: `^` in the math parser is evaluated by `evalexpr`
  with `f64::powf()` (platform library).

## types

- **`ElementName` / `SimpleString` validity** is expressed natively: at most
  70 characters (`ElementName` only), BMP characters only, no characters of
  general category Control, Format, Surrogate, Private Use or Unassigned.
  This is exactly the set upstream accepts (`QString::length() <= 70` and
  `QChar::isPrint()` per UTF-16 code unit, which rejects surrogates).
- **Decimal numbers in files** (`Length`, `Angle`, `Ratio`, ... via
  `decimal_fixed_point_from_string()`): only ASCII digits are accepted.
  Upstream uses `QChar::isDigit()` and also accepts other Unicode decimal
  digits (e.g. Arabic-Indic `١.٥`), which it never writes.
- **`Version`** numbers are parsed with `str::parse::<u32>()` instead of
  `QString::toUInt()`: whitespace around a number (e.g. `"1. 2"`) is
  rejected.

## serialization

- **Integers** (`u32`, `i32`, `i64` values) are parsed with `str::parse()`
  instead of `QString::toUInt()`/`toInt()`/`toLongLong()`: surrounding
  whitespace (only possible in quoted values like `" 42"`) is rejected.
  Signs and leading zeros are accepted like upstream.
- **Floating point values** (only used by importers) are parsed with
  `str::parse()` and must be finite: `inf`/`nan` are rejected (upstream
  accepts them), values that underflow to zero are accepted (upstream
  rejects them), surrounding whitespace is rejected.
- **Child paths** (`SExpression::child("@3")`) parse the index with
  `str::parse::<usize>()` (no surrounding whitespace). Paths are given by
  code, not by files.

### File format migrations

- **v0.1 board outline circles** (layer `brd_outlines`, in boards or
  footprints) are located by their `position` child. Upstream reads the
  first two children of the circle node as coordinates, which fails
  ("Invalid fixed point number string"), so such files cannot be upgraded
  by upstream at all; here they are upgraded as upstream intended.
- **Several devices of one v0.1 component instance** (different devices of
  the same component in several boards) become assembly options in UUID
  order; upstream iterates a `QSet` (hash order, not deterministic across
  runs), so the order of the `device` nodes in `circuit.lp` can differ.
- **Board outlines of equal length** keep their file order when sorted by
  length (stable sort; upstream `std::sort` is unstable), which decides
  which of several equally long v0.1 outlines stays the board outline.
- **Version strings of v0.1 projects** (upstream `toFileProofName()`) are
  decomposed with the `unicode-normalization` crate instead of
  `QString::normalized()`; results only differ where the Unicode versions
  differ.
- **Migration log**: the application version in the footer is passed in by
  the application (`ProjectLoader::set_application_version()`, empty by
  default); dates are formatted with `chrono` like Qt's
  `QDate::toString()`/`QTime::toString()` (English names).

## attribute

- **Numeric attribute values** (`AttributeType::is_value_valid()`): checked
  natively instead of with `QString::toFloat()`; the accepted set is
  identical to upstream (verified against Qt 6.11): `inf` with optional sign
  and `nan` without sign (any case), otherwise a finite number within the
  `f32` range that doesn't underflow to zero; surrounding whitespace allowed.
- **Translated type names** (`AttributeType::name_tr()`): looked up in the
  contexts `librepcb::AttrTypeVoltage` etc., where lupdate extracted them.
  Upstream looks them up in the (empty) context `AttributeType` at runtime,
  so it never showed them translated.
- **`AttributeType::valueFromTr()`**: not ported (system locale dependent,
  no callers).
- **Attribute substitution with a filter**: if the filter changes the length
  of a substituted value, upstream continues with stale positions and
  corrupts the text; the port restarts the search behind the filtered value.
  Results without filter, or with a length-preserving filter, are identical.
- **Whitespace** next to removed variables and around keys uses
  `char::is_whitespace()` instead of `QChar::isSpace()` (same character
  set, see "General").

## algorithm

- **Air wires**: the Delaunay triangulation comes from `spade` and the
  minimum spanning forest from `petgraph`. The total air wire length is the
  same, but among several equally long alternatives (e.g. points on a
  regular grid) a different one may be chosen; upstream's choice depends on
  the unstable `std::sort` anyway. Upstream's workarounds for its
  triangulation library (fallback chain edges, manual triangles for 3
  points) are not needed.
- **Net segment simplification**: junctions are merged in ascending anchor
  ID order; upstream iterates a `QHash` (unspecified, per-process random
  order), so the surviving line IDs may differ when several merges are
  possible. The resulting geometry is the same.

## geometry, font

- **Stroke texts are processed per `char`** (`StrokeFont::stroke_line()`),
  upstream per UTF-16 code unit. Only characters outside the Basic
  Multilingual Plane (e.g. emojis) are affected: they are rendered as one
  replacement glyph (U+FFFD) instead of two, which changes the stroke text
  geometry (and thus exports) of such texts.
- **FontoBene parsing** (`font::fontobene`) uses Rust number parsing and
  `str::trim()` instead of `QString::toDouble()`/`toUShort(16)` and
  `QString::trimmed()`: non-finite numbers (`inf`, `nan`) and `0x` prefixed
  codepoints are rejected. Irrelevant for the bundled fonts.
- **Fonts which fail to load** behave like an empty font with letter spacing
  0 and line spacing 9 (the FontoBene defaults). Upstream uses a
  default-constructed header whose spacings are uninitialized.
- Not ported (UI specific, will live in the rendering layer):
  `Path::toQPainterPathPx()`, `Via::toQPainterPathPx()`,
  `PadGeometry::to*QPainterPathPx()`, `Image::tryLoad()`,
  `Toolbox::shapeFromPath()`, `Toolbox::floatToString()`,
  `Toolbox::prettyPrintLocale()`, `Transform::mapPx()`.

## utils

- **Math parser** (user input of lengths/angles/ratios): evaluated with
  `evalexpr` instead of muparser. Same syntax for numbers (locale decimal
  point, `.` always accepted, thousands separators in groups of 3), `+ - * /
  ^`, unary signs, parentheses, muparser's functions (`sin`, `sqrt`, `min`,
  ..., arguments separated by `;`) and constants (`_pi`, `_e`). Differences:
  `^` is left-associative (`2^3^2` = 64, muparser: 512); comparison, logical
  and ternary operators and `rnd()` are not supported; non-ASCII separators
  work (upstream silently ignored them).
- **Overline markup**: `extract_overlines()` returns byte ranges of the
  output string instead of UTF-16 `(start, length)` pairs.
- **`TangentPathJoiner`**: the `timed_out` flag is set whenever the search
  is aborted; upstream misses the flag if the timeout happens in the last
  top-level iteration.
- **`Toolbox::incrementNumberInString()`** wraps on `i32` overflow (like the
  upstream release build of the Rust code, which panics in debug builds).
- **Numeric sorting** (`toolbox::compare_numeric()`, upstream
  `Toolbox::sortNumeric()`): ICU4X's collator for the root locale (numeric
  ordering, secondary strength = case-insensitive, punctuation not ignored)
  instead of `QCollator` with the system ICU and the system locale. Same
  order for typical designators (`R1 < R2 < R10`, `D1 < D1:1 < D2`, case
  ignored); may differ for locales with tailored collation rules (e.g.
  Lithuanian or Estonian letter order) and between ICU/Unicode versions.
  Callers use a stable sort; upstream uses `std::sort`, whose order of
  elements comparing equal (e.g. `r1` vs. `R1`) is unspecified. Affects the
  row order of exported BOM and pick&place files in these edge cases.

## export

- **Gerber attribute values** are truncated to 65535 characters instead of
  65535 UTF-16 code units. Only differs for (object attribute) values longer
  than 65535 code units that contain characters outside the BMP; upstream
  could even cut a surrogate pair in half there.
- **Generation software version and dates** are passed in by the caller
  (`GerberFileInfo`, `Timestamp`) instead of read from
  `Application::getVersion()` and `QDateTime::currentDateTime()`; the
  written format is `QDateTime::toString(Qt::ISODate)` (local time without
  offset, UTC with `Z`, otherwise `+hh:mm`).
- **NFKD normalization** of Gerber attributes and IPC-D-356A strings uses
  `unicode-normalization` instead of Qt; results only differ between
  Unicode versions (for characters new in one of them).
- Invalid paths and failed arc computations are logged with `log` instead of
  `qWarning()`/`qCritical()`; the output is the same.

## sqlite_database

- Uses `rusqlite` (bundled SQLite) instead of the Qt SQL driver; same pragmas
  (`foreign_keys = ON`, WAL journal) and busy timeout (5 s). Queries use
  rusqlite's parameter binding directly.

## fileio

### FilePath

- Paths are stored as UTF-8 `String`; paths which are not valid Unicode are
  rejected (`FilePath::new()` returns `None`). Upstream decodes them lossily
  into a `QString`, i.e. they would not work anyway.
- Ordering is `str` ordering (code points) instead of `QString` ordering
  (UTF-16 code units). Only differs between non-BMP characters and
  U+E000..U+FFFF; nothing on disk depends on it.
- Cleaning uses `std::path` component semantics (via `path-clean`) instead
  of `QDir::cleanPath()`: `..` above the root is dropped (`/../a` → `/a`),
  Windows prefixes (drives, UNC, verbatim) are parsed by `std`.
- `is_located_in_dir()` works for the root directory (upstream appends `/`
  unconditionally, so nothing is ever located in `/`).
- `to_unique()` uses `dunce::canonicalize()` (no `\\?\` prefix on Windows).

### file_utils (upstream FileUtils)

- `write_file()` is atomic like `QSaveFile` (temporary file in the same
  directory, then rename), but the temporary file is named `.XXXXXX.tmp`
  instead of `<name>.XXXXXX`. Permissions of existing files are kept, new
  files get `0666 & ~umask` like upstream.
- `files_in_directory()` name filters use `glob` syntax (`*`, `?`, `[...]`,
  negation with `[!...]`), case-insensitive; Qt wildcards negate with
  `[^...]` as well.
- Listing errors (unreadable subdirectories) are returned as errors instead
  of being silently skipped.
- `temp_dir()` (upstream `Application::getTempDir()`): if
  `$XDG_RUNTIME_DIR` is unset on Unix, falls back to the system temporary
  directory instead of Qt's `/tmp/runtime-$USER`.

### TransactionalFileSystem

- `clean_path()` converts `\` to `/` *before* resolving `..`. Upstream (on
  Unix) resolves `..` first, so `a\..\..\x` passed the sandbox check and was
  resolved outside the root later by `FilePath` — a sandbox breakout. A
  leading `/` is ignored, so `/../x` is rejected (Qt may resolve it to the
  root). Trimming uses `str::trim()` (Unicode White_Space).
- `dirs()` / `files()` return sorted lists (upstream: `QSet` order).
- Entries in the backup/autosave index files are sorted by `str` ordering
  (upstream: UTF-16 order); only differs for non-BMP file names and does not
  affect reading.
- `load_from_zip*()` reads all entries before writing any, so a corrupt
  archive does not leave a partially loaded state.

### DirectoryLock / system_info

The lock file format and status logic are identical. Values which can
differ between platforms/implementations:

- Line 1 (full user name) is informational only: taken from
  `whoami::realname()`. On macOS upstream runs `finger`, on Windows it falls
  back to parsing `net user` output.
- Line 5 (process name) and the stale-lock check: identical on Linux
  (`/proc/<pid>/exe`). On macOS/Windows/BSD the name comes from `sysinfo`
  (executable file name, on Windows without extension) instead of
  `proc_name()` / `QueryFullProcessImageNameW()`; long macOS process names
  may therefore differ from what a C++ instance wrote, which makes a running
  C++ instance look like a stale lock to a Rust instance (and vice versa).
  Solaris/OpenBSD are not supported by `sysinfo` (processes always look
  stopped).
- PIDs `<= 0` or `> i32::MAX` in a lock file are treated as "not running"
  (upstream passes them to `kill()`, where `<= 0` means process groups).

### ZIP

- `ZipArchive::extract_to()` uses `zip` 8.x extraction (rejects unsafe paths,
  handles symlinks safely) instead of `zip` 0.6.6.
- Entries > 4 GiB are written as ZIP64.

### CsvFile

- A record consisting of a single empty value is written as `""` (by the
  `csv` crate) instead of an empty line.
- Trailing whitespace of comment lines is trimmed with
  `char::is_whitespace()` instead of `QChar::isSpace()`.

### OutputDirectoryWriter

- Index lines are sorted by `str` ordering (upstream: UTF-16 order).
- `remove_unknown_files()` removes empty parent directories only inside the
  output directory (upstream `QDir::rmpath()` may also remove empty
  directories above it).

## library, rule_check

- **URLs** (library URL, resource URLs, organization and PCB design rules
  URLs) are stored verbatim as strings. Upstream parses them with
  `QUrl(…, QUrl::StrictMode)` and writes `toString(QUrl::PrettyDecoded)`
  (library, organization: an empty string if the URL is invalid). Files
  written by upstream are always normalized, so they round-trip
  identically; a hand-edited, non-normalized or invalid URL is written back
  unchanged instead of being normalized or cleared. **File output can
  differ** in that case.
- **Old file formats**: elements in a file format older than the current
  one are rejected (`Error::MigrationRequired`) since the file format
  migrations are not ported yet. Upstream upgrades them when opening.
- **Title case check** (`is_title_case()`, `title_case_fixed_name()`):
  "lowercase letter" is general category Ll (upstream `QChar::isLetter() &&
  isLower()` on UTF-16 code units, identical for BMP names). The fixed name
  converts a letter only if it has a single-character uppercase mapping
  (Rust `char::to_uppercase()`), like `QChar::toUpper()`; results can differ
  only where the Unicode versions of Rust and Qt differ.
- **Pin names in "Overlapping pins: …"** are sorted by natural order
  (`alphanumeric-sort`, case-insensitive), upstream by `QCollator` in numeric
  mode (ICU). Both order digit sequences numerically ("A2" < "A10"); they
  can differ in the relative order of punctuation characters (pin names are
  circuit identifiers, i.e. ASCII letters, digits and `-._+/!?&@#$()`).
- **Message order**: the overlapping pins messages are emitted in order of
  the first pin of each position (upstream: `QHash` iteration order, i.e.
  unspecified). `librepcb-cli` sorts the messages, so its output is not
  affected.
- **Image check** (port of `Image::tryLoad()`): PNG and JPEG files are
  decoded with the `image` crate (only the format given by the file
  extension is tried, like `QImage::loadFromData()` with a format), SVG
  files are parsed with `usvg` and must have a default size of at least 1×1
  pixels (rounded) like with `QSvgRenderer`. Files which one decoder accepts
  and the other rejects (e.g. truncated or exotic PNG/JPEG variants, SVGs
  that `usvg` cannot parse) give different results; the error details in
  the message description are not the Qt texts. Like upstream, empty image
  files are reported as read error ("Failed to read image file").
- **DRC settings sources** (`BoardDesignRuleCheckSettings`, currently in
  `library::org`) keep their insertion order; upstream stores them in a
  `std::unordered_set` and writes multiple sources in unspecified order.
  Sources are always empty in organizations.
- **`Organization::duplicate_from()`** copies the output jobs with new UUIDs;
  upstream clears the job lists before iterating over them and thus drops
  all jobs of the duplicate. Output jobs use the same typed model as projects.
- **`Component::duplicate_from()`**: a pin-signal-map entry referring to a
  non-existent signal becomes unconnected (upstream: undefined behavior).
- **Package check geometry** (pad clearances, legend clearance, annular
  rings, pad origin): upstream decides with `QPainterPath::intersects()` /
  `contains()`, whose curve approximations (e.g. only 3 samples per quarter
  circle of a pad corner in `QPathClipper`) change the results of real
  libraries by tens of µm. These predicates, `QPainterPathStroker` (square
  caps, bevel joins) and `Transform::mapPx()` are ported in
  `utils::painter_path` and give the same results as upstream on all test
  libraries, the official LibrePCB libraries and 12,000 random cases near
  the thresholds. Remaining differences: the rotation of pads by other
  angles than multiples of 90° uses `libm` instead of the platform's
  `sin`/`cos`, and the union of multi-segment slot outlines
  (`QPainterPath::united()`) is approximated by the outlines with winding
  fill. Both can only matter if a distance is within about 1 nm of a check
  threshold.
- Not ported (UI specific, will live in the rendering layer):
  `FootprintPainter`.

## network (crate librepcb-network)

- Async (tokio/reqwest); progress is a `watch` channel, so intermediate
  progress states may be coalesced (upstream emits every signal).
- The HTTP disk cache is not ported (`setCacheLoadControl()`,
  `setMinimumCacheTime()`, `AlwaysCache`); `ApiEndpoint`'s `forceNoCache`
  parameter is therefore dropped.
- User agent: `LibrePCB/<version> (<os>; <arch>; <locale>)` (upstream:
  `LibrePCB/<version> (<pretty OS name>; <arch>; <locale>) Qt/<version>`).
- Error messages: HTTP errors read
  `Error transferring <url> - server replied: <reason> (<HTTP status>)`;
  upstream appends the `QNetworkReply::NetworkError` number instead of the
  HTTP status. `file://` errors read `Error opening <path>: <OS error>` with
  the Rust I/O error text.
- `NetworkRequest` aborts as soon as the 100 MB limit is exceeded instead of
  downloading everything first.
- `Content-Length` of POST requests is always set automatically.
- `FileDownload`: if moving the extracted directory into place fails, the
  backup is restored and an error is returned (upstream restores the backup
  but reports success). A ZIP discovery callback returning a path outside
  the temporary directory is an error (upstream: assertion).
- `ApiEndpoint`: a library entry with a field of the wrong JSON type (e.g.
  `"recommended": "yes"`) is skipped as a whole; missing and `null` fields
  default to empty values like upstream.
- `OrderPcbApiRequest`: the JSON upload body is compact instead of indented;
  the "project is too large" message formats the size like
  `formatFileSize()` (`"1.50 MB"`) instead of
  `QLocale::formattedDataSize()` (`"2 MiB"`).

## project/json_export

- Bounding-box extents use widened integer subtraction before conversion to
  millimeters. Opposite corners more than `i64::MAX` nanometers apart are
  exported without integer overflow; upstream subtracts in its length type.
  Realistic boards and all upstream fixture exports are byte-identical.

## project/erc

- Findings and representative highlighting locations are selected in
  deterministic model order (UUID-keyed collections and library pin order)
  where upstream traverses unordered sets/hashes or registration lists.
  The selected schematic/segment/pin may differ
  when several locations represent the same finding. Approval contents
  are unchanged; the two bus-net findings can appear in a different order.
- A wire width of `i64::MAX` cannot be expanded by 1 nm for highlighting:
  ERC returns a range error instead of overflowing. Realistic widths and
  their highlighting match upstream.

## project/bom_generator

- Value cleanup uses Rust Unicode whitespace (`regex` and
  `str::split_whitespace()`) instead of Qt regular expressions and
  `QString::simplified()`. Non-ASCII whitespace around redundant part names
  can therefore change the cleaned BOM value. Oversized escaped part-name
  patterns return a project error if they exceed the regex engine's limits.

## project/attribute_lookup

- Lookups borrow a project and explicit context instead of holding Qt weak
  pointers and following parent pointers. For detached objects with missing
  library/component references, `value()` and `substitute()` return project
  errors; upstream assumes these references were resolved during construction.
  Values and precedence for valid project objects are unchanged. Dates and
  times use the local time zone, including creation timestamps stored in UTC.

## project/schematic

- Loading validates each schematic page as a whole after deserializing it
  (upstream adds the empty page and then item by item); for a file with
  several errors, the reported error can differ. Messages are upstream's.
- Removing a symbol with connected pins, a junction with lines, a bus
  junction or bus segment with attached net lines, or changing the net/bus
  of a non-empty segment fails with an `ItemInUse` error message (upstream:
  `LogicError` without message).
- `DuplicateUuid` messages use "a" for every item kind ("There is already a
  image ..."; upstream: "an image").
- Pin numbers text (`SymbolPinView::numbers_text()`): the 8 character
  truncation counts the text built so far; upstream accidentally counts the
  previously cached text, so its result depends on the history of pad name
  updates. Display only.
- The inverse of adding a non-empty schematic page is a batch removing its
  items first (a page can only be removed when empty, like upstream).

## project/board

- DRC runs synchronously on an owned snapshot,
  without Qt worker lifecycle callbacks or model changes. Findings are
  deterministic in snapshot order; callers must not depend on the ordering
  of upstream's concurrently scheduled checks. Approval tokens, UUID fields,
  formatting and severities are unchanged, except for the already documented
  equal-length airwire choices. Extraction rebuilds planes and air wires in a
  private copy instead of applying derived geometry to the live board.
  Progress/cancellation is reported between rule groups, rather than Qt
  worker jobs. Errors or cancellation return no partially applicable result;
  approval cleanup therefore only consumes successful results.
  The source rules which ignore footprint circle centers during cutout
  checking and leave `%2` in invalid-pad-connection titles are retained.

- Gerber/Excellon board exports return expanded output names and contents
  instead of writing files or removing obsolete outputs. Relative names are
  relative to the project directory; the caller owns filesystem resolution,
  write callbacks and cleanup. Version/time are explicit, as in the low-level
  generators. Rebuild planes before fabrication export, as upstream does.
  Successful fabrication, glue and component files match upstream; X3 string
  simplification follows Rust's whitespace rule. The upstream component
  outline rule which omits circle centers is deliberately retained.

- Plane calculation is synchronous and reports progress/cancellation between
  planes instead of exposing Qt's asynchronous builder lifecycle. Errors or
  cancellation retain the previous fragments and dirty state atomically;
  upstream can return partial results with per-plane errors. Successful
  copper geometry uses the same 5um arc tolerance and algorithms.

- Pick&place values use `str::split_whitespace()` to simplify whitespace
  after substitution instead of `QString::simplified()` (the crate-wide
  Rust whitespace rule). Typical values and all fixture CSV bodies match
  upstream; exotic Unicode whitespace may differ between Unicode versions.

- Derived via copper/mask diameters use widened addition and return a range
  error for dimensions outside `i64`, instead of overflowing. Ordinary
  dimensions, rule rounding and layer spans match upstream.

- Loading rejects duplicate item UUIDs first and then validates each board
  as a whole (upstream adds the empty board and then item by item); for a
  file with several errors, the reported error can differ. Messages are
  upstream's.
- The `lock` flag of planes is loaded. Upstream's
  `ProjectLoader::loadBoardPlane()` does not read it, so upstream saves a
  locked plane as unlocked after opening the project; we keep the flag.
  File output differs only for locked planes.
- Removing, mirroring or replacing the library device/footprint of a device
  with connected traces, changing the net of a segment with vias, junctions
  or traces, and adding a device of a schematic-only component fail with an
  error message (`ItemInUse`, `SchematicOnlyComponent`; upstream:
  `LogicError` without message).
- Updating net segment elements in place (`UpdateNetSegmentElements`) may
  change the layer of a connected trace, the layers of a via or the side of
  a standalone pad as long as the result is valid; upstream only allows
  `BI_NetLine::setLayer()` on traces not added to the board (the editor
  removes and re-adds them). Resulting files are the same.
- Air wires are rebuilt on request (`Project::rebuild_air_wires()`, called by
  the loader for each board and by the editor after each command) from the
  nets marked dirty by the mutations, a superset of upstream's
  `scheduleAirWiresRebuild()` calls (e.g. also when only traces change).
  The anchors are added in a deterministic order, and ties between equally
  long air wires can be resolved differently (see algorithm). Display and
  DRC "missing connection" approvals only.
- DRC approval cleanup goes through `Project::update_drc_approvals()` and the
  existing approval mutation/journal, with a nonpersistent session cache in
  `BoardDerived`. Quick runs and unknown current-format approvals are
  preserved; a full run after a format upgrade removes all obsolete entries,
  as upstream does.
- `BoardNetSegmentSplitter`: new junction UUIDs come from a caller provided
  generator, and the elements of a resulting segment may be listed in
  another order. Files are unaffected (elements are saved sorted by UUID).

## job

- Jobs use owned enum variants instead of a class hierarchy. Graphics,
  interactive BOM and 3D settings can be read and written independently of
  the deferred renderers. Unknown jobs retain the original complete node,
  including when common fields are edited, matching `UnknownOutputJob`.
- Cloning a known job preserves arbitrary option nodes. Upstream's base
  copy constructor omits them, accidentally losing forward-compatible
  options. This difference can affect files after cloning such jobs.
- Output job execution is synchronous, with caller-provided file and job
  observers replacing Qt signals/event processing. Manufacturing jobs
  rebuild copper planes before exporting, including callers' detached
  geometry without dirty flags. Application version/time are explicit.
  Rendering/3D/interactive BOM jobs fail explicitly until their milestone.
  ZIP timestamps/compression follow the existing Rust ZIP implementation;
  archive contents and output filenames are compatible.

## cli

- The native CLI uses clap instead of Qt's argument parser. Help layout,
  parser diagnostics and parser-error exit codes follow clap. Version output
  reports this Rust package's version, without fictitious Qt/OpenCascade
  versions. Application messages still use the upstream translation context.
- Implemented commands are `open-project`, `open-library`, `open-symbol` and
  `open-package`; symbol/package commands currently support checks only.
  Graphics exports, STEP/minification and rendering-dependent jobs remain
  deferred and are reported as unsupported. The 115 upstream tests of the
  implemented command paths pass unchanged; the full 178-test suite still
  has 63 failures in these deferred features and parser/help/version text.
- Saving `.lppz` builds the updated archive in memory, then writes it
  atomically through the existing file utility. A failed archive export
  therefore retains the original file. The project root is isolated from
  sibling files before loading ZIP contents, like upstream.
