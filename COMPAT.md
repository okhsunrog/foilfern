# Differences from upstream LibrePCB

Behavior differences of the Rust port compared to the C++ reference
(`../LibrePCB`), including exotic edge cases. File formats are unaffected
unless stated otherwise. The file has two parts, each grouped by module in
the same order:

- **Part 1: Intentional differences** are permanent design decisions:
  Rust-native semantics instead of Qt emulation (see "No Qt emulation" in
  `AGENTS.md`), a different internal architecture, deliberate fixes of
  upstream bugs, differences that do not affect files, and things that are
  different by design (e.g. PDF/SVG output not byte-identical to Qt). They
  are **not work items**: do not "fix" them to match upstream.
- **Part 2: Not ported yet / known gaps** lists missing features and
  behavior we intend to match eventually (3D/STEP, printing, missing
  dialogs, missing application wiring, ...). These **are work items**;
  remove an entry (or move the remaining difference to Part 1) once it is
  done.

Rule for new entries: a divergence that is meant to stay goes into Part 1;
anything that is missing or differs only because it is not done yet goes
into Part 2. An entry that mixes both is split.

# Part 1: Intentional differences

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

- **Air wires**: ported literally, including upstream's Delaunay
  triangulation library (with its single precision circumcircle test),
  the fallback edges and libstdc++'s `std::sort()` (`clipper::std_sort`),
  so the same air wires are chosen among equally long alternatives as by
  upstream built with GCC/libstdc++ (the DRC "missing connection"
  approvals depend on it). Upstream built with another standard library
  (MSVC, libc++) may choose differently.
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
- **`GraphicsExportSettings`** is plain data: the page size is its
  `QPageSize` key string, and the painting helpers (`getFillColor()`,
  `convertImageColors()`) are not ported. The adjustment of board colors
  for white backgrounds deliberately ports Qt 6's `QColor` HSV conversion
  (single precision, 16 bit channels, `qt_div_257()` rounding), because the
  resulting colors are written to `jobs.lp` by the default graphics jobs.
  (Embedded default colors: see Part 2, export.)
- **Graphics export** (port of `GraphicsExport`, `GraphicsPainter` and the
  painters, in the scene crate as `librepcb_scene::export`): no Qt, so the
  written bytes differ completely from upstream's `QPdfWriter`,
  `QSvgGenerator` and `QImageWriter` output. The page layout follows
  upstream (fixed or content-derived page size, orientation incl. the
  automatic one, margins, rotation, mirroring, fixed scale or fit,
  background, minimum line width, black/white, one PDF with all pages,
  numbered SVG/image files for several pages, output directory creation,
  error messages). Differences:
  - PDF: vector paths through `pdf-writer` at the page size in points
    (upstream: 1200 DPI device units); creator and producer are
    `LibrePCB <version>` (upstream producer: Qt). No invisible texts for
    selection/search (upstream draws transparent TrueType texts on boards
    and footprints), no embedded fonts (all texts are stroke paths, see
    scene).
  - SVG: hand-written, one `<path>` per paint, `<desc>Generated with
    LibrePCB</desc>`; size in millimeters rounded to 0.1 µm.
  - Images: PNG, JPEG and BMP only (upstream: every Qt image format), via
    vello_cpu and the `image` crate; anti-aliasing differs. JPEG/BMP drop
    the alpha channel like Qt (transparent areas become black).
  - The source rectangle (upstream `QPicture::boundingRect()`) is the
    bounding box of the paths plus half the stroke widths; Qt's is
    slightly larger (about 0.3 mm on a schematic page), so derived page
    sizes can differ by a few points.
  - Page sizes are Qt 6's `QPageSize` table (`PageSize`); the page size in
    whole points is used for all output devices.
  - Pages with nothing to paint get the device's real-size scale instead
    of Qt's infinite fit scale.
  - Unknown extensions fail before painting (upstream paints the page
    first, then fails to save it); the message is the same.
- **Interactive HTML BOM** (`InteractiveHtmlBom`): generated by the
  `interactive-html-bom` crate in the same version as upstream (0.2.0),
  with the same conversions as upstream's C++/FFI wrapper (`f32`
  millimeters, Y axis negated as floating point value, so zero becomes
  `-0`), so the HTML is byte-identical (verified against `librepcb-cli`
  on all test projects, `tests/unittests/compat/interactive_html_bom.rs`). `add_footprint()`
  returns an error if a pad outline cannot be calculated (upstream's
  `noexcept` method would terminate the application).

## job

- **Class hierarchy → enum:** `OutputJob` holds UUID, name and options,
  and `OutputJobKind` the type specific settings (one struct per upstream
  subclass, plain data with public fields). Signals and icons are not
  ported. Output is identical.
- **Copies keep the forward compatibility options:** upstream's
  `OutputJob` copy constructor (used by `cloneShared()`, e.g. when copying
  a job in the output jobs dialog) drops the `option` nodes of the base
  job; `Clone` keeps them. Only differs for copied jobs of a future minor
  version with options.
- **Ordering of options and layer colors** (upstream `QMap<QString, ...>`)
  is `str` ordering, see "String ordering" under General.
- **Colors** (`layer`/`background` of graphics jobs) are parsed with
  `Color` (hex forms only, see types); `QColor` also accepts SVG color
  names, which LibrePCB never writes.
- **Default job set:** upstream creates it in the editor's output jobs
  dialog; here it is `job::default_output_jobs()` in core, taking the
  Gerber settings and custom BOM attributes as parameters.

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
  all jobs of the duplicate.
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
- `FootprintPainter` is not part of core; it is ported in the scene crate
  as `FootprintScene` (see scene).

## network (crate librepcb-network)

- Async (tokio/reqwest); progress is a `watch` channel, so intermediate
  progress states may be coalesced (upstream emits every signal).
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
- `LibraryDownload` (upstream in the editor) is part of this crate; it
  returns the library directory or an error instead of emitting
  `finished(bool, QString)`. The library installer (`library_installer`,
  selecting libraries by name/UUID with their dependencies) has no upstream
  counterpart; upstream does this in the library manager UI.

## project/schematic

- KiCad project import stores source-specific pin legends, global label
  outlines and DNP/NC markers in `schematics/<sheet>/import-appearance.json`.
  Foilfern uses it for rendering. Upstream LibrePCB opens the native schematic
  with its own label and pin rendering. The sidecar does not change the
  circuit or the native schematic S-expression format.

- The declarative editor can replace the artwork of a symbol in the
  project library while preserving its pin interface. This changes the native
  `symbol.lp` file. The file format and pin references follow upstream.
- The declarative editor can draw groups of explicit orthogonal wire paths
  in one preview. It refuses to replace segments attached to pins outside
  the selected group or to buses, and checks the new paths for collisions.
  It can place a selected block and redraw its explicit wire groups together,
  resolving relative coordinates from the resulting pin positions. The wires,
  junctions, labels and notes use the native format.
- A declarative preview can create named nets and library-backed components,
  then wire them on an existing sheet. Replay binds added instances through
  a native component attribute and keeps their existing identities.

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
  The anchors are added in the order of upstream's registration lists after
  loading a project (component, component signal and pad UUID; net
  segments by UUID). After editing, upstream's lists are in modification
  order, so ties between equally long air wires can be resolved
  differently (display and DRC "missing connection" approvals only), and
  the same holds for hand-written files whose elements are not sorted by
  UUID.
- `BoardNetSegmentSplitter`: new junction UUIDs come from a caller provided
  generator, and the elements of a resulting segment may be listed in
  another order. Files are unaffected (elements are saved sorted by UUID).

## scene (crate librepcb-scene)

Rendering only; no file is affected.

- Schematic texts, pin names and numbers and net/bus labels are drawn with
  the project's stroke font (`newstroke.bene`) instead of the Noto Sans /
  Noto Sans Mono TrueType fonts upstream draws through Qt. The stroke text
  is placed in the box the TrueType text would occupy (Noto Sans descent
  and cap height), with upstream's alignment, auto-rotation, multi-line
  and overline rules; glyph shapes and widths differ.
- Pad and via drills are cut out of the copper (filled subpaths nested in
  others become holes, like Qt's odd-even rule); all drills are also
  filled with the background color on a separate layer (the editor look,
  hidden in graphics exports).
- Planes are drawn with the fragments stored in the board's derived data
  (if computed) plus their outline as a hairline.
- Board colors are the dark scheme's primary colors (the editor look); the
  graphics export's color adjustment for white backgrounds is not applied.
  Graphics exports build the scenes with the colors of the export
  settings (`ColorScheme::custom()`).
- Symbols and footprints (`SymbolScene`, `FootprintScene`, upstream
  `SymbolPainter`/`FootprintPainter`) draw their texts raw (e.g.
  `{{NAME}}`) like upstream; footprint texts use the default stroke font
  found at runtime (`LIBREPCB_SHARE`, `../share/librepcb` next to the
  executable, or the upstream checkout the crate was built against); without
  it, no footprint texts are drawn.
- Board graphics export (`BoardPainter`): THT pads and vias are drawn once
  in the pads/vias color if no copper layer is enabled (upstream
  deduplicates identical paths; here only the top copper instance, see
  Part 2 for the vias this misses). Plane outlines are drawn as minimum
  width lines (upstream: fragments only). Zones and air wires are not
  drawn (like upstream).
- Project graphics jobs and schematic exports use `librepcb-graphics`;
  interactive canvas scenes and library previews use `librepcb-scene`.
  Realistic project output uses the shared `BoardPhysicalGeometry`, retaining
  drill spans, face-dependent blind-hole projection, mask and legend clipping
  for future 3D consumers. Rasterization uses vello_cpu; vector output retains
  even-odd contours. Antialiasing and font shaping differ from Qt.

## render3d (crate librepcb-render3d)

- The interactive board viewer uses Bevy PBR rather than upstream's Qt/OpenGL
  renderer. Lighting, tone mapping, camera controls, triangulation and pixels
  differ; this does not alter project or manufacturing file output. The viewer
  reuses core's physical geometry and the STEP backend's placed component meshes.
- Missing or unsupported STEP component bodies are reported while retaining
  the PCB and successfully loaded components. Surface colors/opacity are kept;
  material roughness and metallic response are viewer defaults because STEP
  presentation styles do not supply the PBR parameters used by Bevy.

## project (ERC, attribute lookup, BOM, JSON export)

- ERC (`project::erc`): where upstream iterates a `QHash`/`QSet` (nets
  attached to a bus) or a registration list (net segments of a net: here
  schematic page order, then segment UUID order; bus segments of a bus:
  schematic UUID order, then segment UUID order; symbol pins of a
  component signal: gate order), items are visited in a deterministic
  order. This only affects the order of the messages and which of several
  equivalent items a message location (highlighting) points to; the
  messages and approvals (`circuit/erc.lp`) are identical. Symbols whose
  pins cannot be computed from the project library are skipped (the loader
  rejects such projects, upstream cannot represent them at all).
- `ProjectAttributeLookup` borrows the project and the objects (upstream:
  `QPointer`s which silently yield empty values after deletion). Built-in
  keys whose library element is missing from the project library
  (`COMPONENT`, `DEVICE`, `PACKAGE`, `FOOTPRINT`) resolve to an empty
  string (upstream cannot represent that state). `PROJECT_DIRPATH` and
  `PROJECT_FILEPATH` are empty for projects not on disk. `CREATED_DATE`,
  `CREATED_TIME`, `DATE` and `TIME` are formatted with chrono in local
  time (`%Y-%m-%d`, `%H:%M:%S`), the same text as Qt's `ISODate`.
- `BomGenerator`: the value cleanup uses the `regex` crate with ASCII `\s`,
  the same set as upstream's `QRegularExpression` without Unicode
  properties; `QString::simplified()` is `split_whitespace()` (see
  General).
- `json_export`: the output is written by a port of `QJsonDocument`'s
  indented format and of Qt 6's `QByteArray::number(d, 'g',
  FloatingPointShortest)` (shortest round-trip digits, exponent form like
  `1e-05` only where shorter). Older Qt versions may format numbers in
  exponent form differently; lengths realistically exported (multiples of
  1 nm between 0.001 mm and 1 m) are formatted identically. Keys are sorted
  like `QJsonObject`.

## workspace

- **Library database** (`libraries/cache_v8.sqlite`): same file name,
  schema, database version and row contents as upstream, so both
  implementations can use the same workspace (not at the same time, the
  data directory is locked). Values are bound like the Qt SQLite driver:
  empty icons/logos and empty `generated_by` are `NULL`, strings upstream
  passes through `nonNull()` are empty strings. URLs (organizations,
  resources, PCB design rules) are stored verbatim instead of normalized by
  `QUrl::toString()`; they are only read for display.
- **Scanner** runs synchronously on a caller provided thread instead of
  an owned `QThread`; progress is a callback. Libraries are scanned in the
  order of their directory names compared case insensitively with
  `str::to_lowercase()` (upstream: `QDir` default sorting), which only
  changes the database row IDs and thus which of two elements with the
  same UUID *and* version is returned as "latest".
- `WorkspaceLibraryDb::Part` is `library::dev::Part`; parts are sorted
  like upstream, with attribute values compared by the natural order of
  `natural_cmp_case_insensitive()` instead of `QCollator` (see library). If
  two attribute values compare equal there, the next attribute decides
  (upstream: the parts compare equal).
- **Workspace settings**: color schemes (`schematic_color_schemes`,
  `board_color_schemes`, `3d_color_schemes`) are kept as raw S-expressions
  and written back unchanged (upstream re-serializes them canonically when
  they are written, i.e. after an edit or a file format upgrade). Keyboard
  shortcuts keep their key sequences as strings without
  `QKeySequence` normalization. The migration of the legacy `themes` entry
  writes the user color schemes like upstream (as raw S-expressions).
  API endpoint URLs are stored verbatim (upstream: `QUrl`), and an endpoint
  counts as valid if its URL is non-empty (upstream: `QUrl::isValid()`).
- The "workspace requires LibrePCB %2 or later" message fills in both
  placeholders (path and version); upstream passes only the version, so its
  message shows the version as path and a literal `%2`.
- The most recently used workspace path (`QSettings`) is not ported (it
  belongs to the application). `Workspace::open_or_create()` performs the
  steps of the upstream workspace initialization wizard (create, choose and
  copy the data directory) without UI and without downloading the example
  projects.

### Plane fragments and board exports

- **Plane fragments builder**: creating a `PlaneJob` does not modify the
  board; the processed layers are unscheduled when the result is applied,
  and only if the project was not modified since the job was created
  (upstream takes the scheduled layers when creating the job). Planes may
  therefore be rebuilt more often, the fragments are the same.
- The fragments of a plane are sorted by their first point with a stable
  sort (upstream `std::sort`); the order can only differ for two fragments
  starting at the same point, which Clipper does not produce.
- Cancellation is an `AtomicBool` checked between planes of a layer
  (upstream also checks between the steps of one plane).
- **Missing stroke font**: upstream fails to load a board whose default
  stroke font does not exist in the project (`StrokeFontPool::getFont()`
  throws in the `BI_StrokeText` constructor); here the board loads, and the
  plane rebuild and the exports fail with the same message.
- **Export metadata**: the application version and creation date written
  into Gerber, Excellon, pick&place and IPC-D-356A files are passed in by
  the caller (`ExportInfo`), see "export".
- **Output paths** are resolved relative to the project directory like
  upstream; a project without a directory on disk (in-memory file system)
  cannot export to relative paths (error instead of a path relative to an
  empty directory).
- **Interactive HTML BOM generator** (`BoardInteractiveHtmlBomGenerator`):
  the generation date is a `Timestamp` (the date of the `ExportInfo`,
  written as `yyyy-MM-dd hh:mm:ss` in the time zone of the value). The
  designator prefix for the component order strips trailing
  `char::is_numeric` characters (upstream: `QChar::isDigit()`, i.e.
  only decimal digits; differs for other numeric characters like `²`).
  BOM rows are grouped by their field values in Rust `String` order
  (upstream: `QStringList` in UTF-16 order) and sorted with a stable sort
  (upstream: `std::sort`); the resulting order only differs for rows whose
  first designators compare equal (e.g. `r1` and `R1`).
  `Board::calculateBoundingRect()` is ported as a private helper of the
  generator.

### Specctra DSN export

- The DSN output is byte-identical to upstream (upstream unit test
  expectation), including upstream's quirks that define the bytes: blind
  and buried via padstack IDs contain a literal `%2` (nested
  `QString::arg()`), vertical oblong pads are exported as a zero-length
  path.
- The host CAD name and version in the `parser` node are parameters
  (default "LibrePCB" and the crate version; upstream: the application
  name and version).
- The pins of a net are ordered by (component UUID, signal UUID, pad UUID)
  and (net segment UUID, pad UUID); upstream iterates its registration
  lists, which is the same order for a freshly opened project but can
  differ after editing (only the order of the `pins` list changes).

## project/board/drc

- **Exact DRC copper geometry:** DRC uses one rational boolean implementation
  (`dashu-ratio`) for copper/stopmask aggregation and the checks consuming those
  regions: copper clearances, hole clearances, annular coverage and cutout
  boundaries. Intermediate vertices are not rounded to integer nanometers.
  Shape construction, arc flattening and primitive offsets still follow the
  existing upstream-derived geometry; project files and manufacturing exporters
  retain their integer format and geometry pipeline. At sub-nanometer boundaries,
  exact positive-area overlaps or gaps can produce violations which upstream's
  sequential rounding suppresses. Open boundary intervals coincident with copper
  count as covered; isolated point touches do not create positive-length intervals.
  Integer rounding is applied only to the resulting display locations and cannot
  suppress a violation. There is no backend flag or alternate DRC runner.

- **Snapshot safety and failed checks:** preparation rejects intervening model
  edits before applying planes and extracts data in the same exclusive operation.
  Concurrent air-wire/plane-fragment updates are allowed because they do not
  change calculation inputs; an expired change-journal cursor fails closed. Failed/incomplete plane rebuilds abort preparation instead of
  silently checking partial copper. Approval cleanup requires a successful,
  identified, current result; failed or outdated checks preserve approvals and
  their version. GUI results from older revisions remain marked outdated.
  Background plane rebuilds also discard failed, aborted or stale results
  instead of replacing newer fragments or clearing dirty layers.
  These reliability checks intentionally strengthen upstream behavior.

- **Iteration order**: the input data keeps net segments, devices, pads,
  vias and junctions in `BTreeMap`s (UUID order) and layers in `BTreeSet`s;
  upstream iterates `QHash`es/`QSet`s. Approvals are identical (upstream
  sorts the objects of an approval canonically), but the order of the two
  objects in the text of a copper clearance message (`'GND' pad ↔ 'VCC'
  pad`) and the order of the messages may differ. Merging of copper
  clearance violations per object pair is order independent.
- **Keepout zones**: upstream tests the intersection with
  `QPainterPath::intersects()` against the union (`|=`, `QPathClipper`) of
  the object areas; here each area is tested with the ported Qt predicate
  (`PainterPathPx::intersects()`) and the object intersects if any area
  does. Via areas are circles drawn with arcs (same Bézier quarter curves)
  instead of `QPainterPath::addEllipse()`. Results only differ for objects
  touching a zone within Qt's curve sampling tolerance.
- **Device names** in "Device in courtyard"/"Device overlap" messages are
  ordered by `str` ordering instead of `QString` (UTF-16) ordering; only
  names with characters outside the BMP can be affected.
- **Plane fragments**: like upstream, a full check rebuilds all planes;
  if the plane job cannot be created (e.g. missing stroke font, see
  "Plane fragments and board exports"), `Project::run_drc()` fails with
  that error.
- **Errors**: checks that fail (e.g. a non-positive calculated diameter)
  report the error in `DrcResult::errors` like upstream, with the Rust error
  message. A panic of a check thread is reported as error as well.
- **Approval cleanup** (`Board::updateDrcMessageApprovals()`):
  `Project::drc_approvals_update()` returns a `SetDrcApprovals` mutation
  instead of modifying the board; the set of approvals seen during the
  session is derived data of the board. `ProjectEditor::update_drc_approvals()`
  applies it without undo step and marks the project as manually modified,
  like upstream's board editor. `librepcb-cli` does not clean up approvals
  (like upstream).
- `DrcMsgInvalidPadConnection` keeps upstream's untranslated `'%2'` in its
  message (upstream substitutes only the first placeholder).

## editor (crate librepcb-editor)

- **Undo stack**: every change is recorded in a group (upstream `execCmd()`
  of a single command is a group with one operation). The redo history is
  discarded when a group is *committed*; upstream discards it when a group
  is started, so an aborted group also loses the redo history there. If
  undoing or redoing fails (a violated model invariant), the applied steps
  are rolled back and the history is cleared; upstream rethrows and leaves
  the stack as it is. `state_id()` replaces `getUniqueStateId()`/the
  `stateModified` signal.
- **Atomic edit publication**: the Rust journal publishes changes from a
  synchronous transaction after commit. A successful rollback restores the
  file overlay and preserves the previous revision. Undo, redo and cancellation
  of an interactive preview publish new changes.
- **Combining net segments** (`CmdCombineSchematicNetSegments`,
  `CmdCombineBoardNetSegments`, used when a wire or trace connects two
  segments) moves the elements with their UUIDs; upstream re-creates
  junctions, pads, vias and labels with new random UUIDs. Likewise the
  segments re-added after removing items keep the UUIDs of their elements
  (upstream: new junction and label UUIDs). Files differ only in these
  (random) UUIDs.
- **Forced net names** of component signals (e.g. `{{VALUE}}` of supply
  symbols) are applied like upstream by the wiring commands and to the
  segments re-added after removals, but only if the substituted name is a
  valid net name (upstream collects the raw strings, and the wiring tool
  shows a message box for invalid ones; we log a warning). An explicit
  `net` parameter of a wiring command takes precedence over a forced name.
- **`AddBoard`** with `copy_from` (upstream `Board::copyFrom()`) does not
  copy the calculated plane fragments; the planes of the copy are rebuilt
  like any other planes.
- **Adding a via** connects it to the traces and junctions of its net at
  its position like upstream's add-via tool (hit test: the trace width
  resp. the widest trace at a junction instead of the graphics items'
  shapes), but builds the combined segment in one step: split traces are
  replaced by two traces to the via (upstream first adds a split junction,
  then replaces it), the element UUIDs of the combined segments are kept.
- **Reviewed builtin routing** (`routing_service`): original session-local
  candidate service with detached native snapshots, exact mutation previews,
  revision-bound single-undo apply and full applied-model validation. Candidate
  state is never serialized; project/library file formats remain unchanged.
- **Autorouter** (`Autoroute`): no upstream counterpart (upstream only
  exports Specctra DSN); see the `commands::autoroute` module docs.
- **`create_project()`** copies the stroke fonts from a given directory
  (upstream: the application resources directory), because the core's
  `Project::create()` does not know the application resources yet.
- Interactive steps are replaced by parameters: no snapping to the grid or
  to items, gates of a multi-gate component are placed with a fixed offset
  unless placed individually, and the default device of `AddDevice` is the
  first device of the component's assembly options.

### Schematic editor FSM (`fsm::schematic`)

- **Undo steps** are upstream's: a drag is one group, followed by a
  separate group simplifying the modified net and bus segments (also
  after removing items); a paste and the one-shot operations (rotate,
  mirror, move by keys, snap) do not simplify, like upstream. Drawing a
  wire or bus is one group per clicked segment plus a simplification group
  when finishing.
- **Live previews** are mutations in the open undo group, replaced on
  every pointer move; upstream modifies the objects without undo and
  records the command at the end. While a tool keeps a group open, other
  commands (e.g. from the embedded MCP server) join that group.
- **Clipboard:** the format is upstream's (`schematic.lp` in a ZIP under
  `application/x-librepcb-clipboard.schematic; version=<app version>`).
  When pasting, net lines at bus junctions of a bus segment which was
  split while copying are attached to the right part (upstream keeps only
  the junctions of the last part). Symbol clipboard data (from the symbol
  editor) pastes its polygons, texts and images like upstream.
- **Hit testing:** net and bus junctions are tested on the model with
  upstream's 1.2 mm grab square; items of equal priority keep the item
  order (upstream: `QMultiMap` insertion order).
- **Images:** the file name of an added image is derived from the file
  name without asking the user (upstream: an input dialog), the image
  chooser is requested from the application.
- **Find:** component candidates are the symbol names (with gate suffix)
  resolved through the symbols of the page, so symbols of multi-gate
  components are found too (upstream looks them up as component names).
- **Cross-probing** is an output of the FSM (`cross_probe()`); the
  application highlights the objects in the other editors.

### Board editor FSM (`fsm::board`)

- **Live previews** are mutations in the open undo group, replaced on
  every change (dragging, trace positioning, placing vias, pads, devices,
  texts and holes, drawing polygons, planes and zones); upstream modifies
  the objects without undo and records the edit commands at the end. The
  resulting undo steps are upstream's ("Drag Board Elements", one "Draw
  Board Trace" group per clicked segment, one "Draw board polygon/plane/
  zone" group per vertex, ...). Air wires are rebuilt during previews.
- **Draw trace:** net segments are combined keeping the element UUIDs
  (upstream creates new segments); a pad without traces is attached to the
  current segment directly (upstream first creates a segment for it).
- **Add pad:** changing the net re-creates the preview segment with the
  new net (upstream removes, edits and re-adds it).
- **Hit testing:** grab areas are the scene items of `librepcb-scene`
  (device grab area: its footprint graphics); junctions are tested on the
  model with a circle of the widest trace at them (upstream
  `BGI_NetPoint`). Items of equal priority are ordered by item reference
  (upstream: `QMultiMap` insertion order).
- **Selection:** held by the FSM; selecting a device selects its texts
  (like upstream). The rubber band selects junctions whose position is in
  the rectangle (upstream: whose circle touches it).
- **Clipboard:** upstream's format (`board.lp` plus `dev/`, `pkg/` in a
  ZIP under `application/x-librepcb-clipboard.board; version=<app
  version>`). Footprint clipboard data (from the package editor) pastes
  its polygons, stroke texts and holes like upstream. Like upstream, a
  pasted net whose name does not exist becomes a new net with an automatic
  name.
- **Change device:** the devices offered in the context menu come from the
  editor's library element source (upstream: the workspace library
  database).
- **Cross-probing** is an output of the FSM (`cross_probe()`,
  `highlighted_nets()`); the application highlights the objects in the
  other editors.
- Plane visibility (context menu "Visible") and aborting blocking tools
  of other editors are handled by the application (the FSM reports the
  context menu action).

### DXF reader (`import::dxf_reader`)

- Based on the `dxf` crate instead of dxflib: an empty file is read as a
  drawing without entities like upstream, but other files the crate
  rejects are reported as errors even where dxflib silently reads
  nothing.

### Library element editors (`library_editor`, `fsm::library`)

- **Undo stack:** a group stores snapshots of the element content before
  and after it (upstream: undo command objects). The grid interval and
  message approvals are not part of the snapshots; changing them marks the
  element as modified without undo, like upstream. A group without changes
  is dropped.
- **Auxiliary files:** image files of removed symbol images and STEP files
  of removed 3D models are deleted when saving (upstream: immediately, by
  `CmdImageRemove` / `CmdPackageModelRemove`, restored on undo). Writing a
  STEP file (`AddPackageModel`, `EditPackageModel`) is not undone; files
  written since the last save which are unreferenced at the next save are
  deleted.
- **Live previews** modify the element inside the open undo group (upstream:
  edit commands with `immediate = true`); the undo steps are upstream's.
- **Hit testing:** the FSMs query a view (`LibraryView`); the model based
  `ModelHitTester` approximates texts by rectangles (0.7 × height per
  character) and pins by their line; `SymbolScene`/`FootprintScene` map
  their items to objects (zones are not drawn by `FootprintScene`, like the
  upstream graphics export).
- **Dialogs** (pin/pad/polygon properties, import pins, courtyard excess,
  fix parameters) are requests to the application; check fixes take their
  answers as `FixParams`.
- **Images** (symbol editor): the file name of an added image is derived
  from the base name passed by the application (upstream asks for it in
  an input dialog); an existing file with the same content is reused
  like upstream. The clipboard data has no pixmap.

## project (output job runner)

- **Graphics jobs** need a `GraphicsExporter` set by the caller
  (`set_graphics_exporter()`, the CLI uses the scene crate's
  `ProjectGraphicsExporter`) since the painters live in the scene crate;
  without one they fail like unsupported job types. `build_pages()` ports
  `buildPages()`; for board contents with "no board" (a project without
  boards and the default board set), no page is created (upstream would
  dereference a null board).
- Unknown job types fail with the upstream message.
- The Qt signals (`jobStarted`, `aboutToWriteFile`, `aboutToRemoveFile`,
  `warning`) are one observer callback (`OutputJobEvent`).
- The application version and creation date written into the files are
  passed in (`ExportInfo`), one date for the whole run (upstream takes the
  current time for each file).
- Archive jobs collect their input files in an in-memory transactional
  file system; upstream opens a writable one in a random temporary
  directory, which is left behind. The archive content is the same (of
  several input files with the same name, the first written one wins like
  upstream's `QMultiHash` iteration), but the ZIP entries are sorted (see
  TransactionalFileSystem). The same applies to `*.lppz` output jobs.
- A project without a directory on disk cannot run output jobs (the
  output directory is relative to it).

## import (crate librepcb-import)

- EAGLE XML (parseagle): numbers are parsed with `str::parse` after
  trimming instead of `QString::toDouble()`/`toInt()`, and XML is read
  with `roxmltree` (DTDs allowed) instead of `QDomDocument`; realistic
  files are read the same, exotic number spellings Qt accepts may be
  rejected.
- HTML descriptions (EAGLE) are converted to plain text by a small
  converter (`html::html_to_plain_text`: block elements become line
  breaks, entities are decoded) instead of `QTextDocument::toPlainText()`;
  whitespace can differ in unusual markup.
- Grab areas of EAGLE symbols: the union of the filled outlines is
  computed with Clipper (only overlapping shapes are united) instead of
  `QPainterPath::united()`, and "is the text inside the grab area" uses the
  same union; the imported files of the upstream test data are identical.
- KiCad footprints: lines are grouped into polygons per (layer, width) in
  a deterministic order (layer order, then width) instead of upstream's
  `QMap` order keyed by `Layer` pointers, so the order of polygons in
  `package.lp` can differ from upstream (upstream's own order is not
  stable either).
- Directory scans list entries sorted by Rust string ordering instead of
  `QDir::Name` ordering (UTF-16 based; differs only for exotic names).
- KiCad datasheet URLs are taken as written instead of being normalized
  by `QUrl` (and not checked with `QUrl::isValid()`).
- `MessageLogger` child loggers prefix messages with `"[group] "` and
  forward them synchronously (no Qt signals); scan, parse and import run
  on the caller's thread and report through `Progress` (percent, status,
  cancellation) instead of `QThread`/`QFuture` and signals.
- The KiCad import's lookup of already imported elements
  (`ImportedElementLookup`, implemented for `LibraryDb`) does not wait
  for a running workspace library scan like upstream; the caller
  rescans first if needed.
- EAGLE project import: `Project::create()` of the core does not copy the
  stroke fonts; callers (MCP `project_create`, tests) write
  `resources/fontobene` themselves before creating the project.
- `EagleLibraryImport` enables the "EAGLE Import" categories by default
  (upstream's class has none, the wizard enables them);
  `KiCadLibraryImport` keeps upstream's class default (disabled). The
  MCP tool `library_import` uses the wizards' defaults (name prefix off,
  categories on) for both.

## CLI (crate librepcb-cli)

The console output (messages, help texts, exit codes) is identical to
upstream `librepcb-cli` 2.1.1 except for:

- **Graphics exports** (`open-project --export-schematics`, `open-symbol
  --export`, `open-package --export`, graphics output jobs) print the same
  lines as upstream, but the files differ (see export). The help texts
  list `pdf, svg, bmp, jpeg, jpg, png` as supported extensions (upstream:
  PDF, SVG and all image formats of the Qt installation).
- **`--version`** prints `Implementation librepcb-rs (Rust, no Qt)`
  instead of the Qt version line, `OpenCascade N/A`, and the Git revision
  `unknown`; the application version is the crate version (also written
  into exported files as generating software).
- **`--verbose`** log messages use the `env_logger` format and the
  messages of the Rust port (upstream: Qt's message handler format).
- **Parser errors**: the command line is parsed with `clap`; the error
  texts of `QCommandLineParser` are reproduced for unknown options
  (all of them, like `Unknown options: a, b, c.`), missing and unexpected
  values. The parser strings of Qt (`Usage: {0}`, `Options:`,
  `Arguments:`, the error texts) are not in LibrePCB's translation
  catalogs, so they are always English (upstream: Qt's catalogs).
- **Specctra session import** (`ImportSpecctraSession`, upstream
  `CmdBoardSpecctraImport`): messages are returned (and logged with the
  `log` crate) instead of a `MessageLogger`; net segments without net are
  matched by their exported dummy net name (`~anonymous~<uuid>`), so each
  keeps its own pads (upstream adds the pads of all segments without net to
  every such net); only footprint pads of the imported board are anchors
  (upstream also considers pads of the same component on other boards);
  old traces are matched regardless of their direction; zero-length wire
  segments are skipped. Via padstack names mangled by FreeRouting 2.x
  (fractional digits dropped, e.g. `via-0:auto-0:auto-tht`) are mapped back
  to the exported names, so drill, size and exposure are kept (upstream
  falls back to automatic values). Additionally, a strict mode validates
  the session against the export (unchanged project revision and
  placement, known nets and padstacks) before modifying the board.
- **FreeRouting** (`FreeroutingRouter`, no upstream counterpart: upstream
  only exports/imports files): the DSN passed to FreeRouting uses the
  resolution 1/100000 mm instead of the export's 1/1000000 mm, because
  FreeRouting 2.4.1 reports bogus clearance violations and leaves
  connections unrouted with the finer resolution.

## app (crate librepcb-app)

- **Number formatting in the UI** (`Backend.format-length` etc.) uses `.`
  as decimal separator and no group separators instead of `QLocale`; input
  parsing uses the math parser with `.` (upstream accepts the locale's
  decimal separator too).
- **Workspace selection (M3c):** the workspace comes from
  `LIBREPCB_WORKSPACE` or upstream's client settings
  (`workspaces/most_recently_used` in `~/.config/LibrePCB/LibrePCB.conf`,
  Linux; the chosen workspace is written back there like upstream, other
  entries of the file are kept); the initialize workspace wizard asks if
  none is set, it does not exist, or it needs an upgrade or initialization.
  `--workspace` (and the headless `--screenshot` mode) skip the wizard and
  create the workspace without asking. Differences to upstream's wizard:
  the data directory is copied synchronously (no progress bar), the
  example projects are not downloaded when a workspace is created, the
  language list shows the locale codes (upstream: native language names),
  and the dialogs are overlays of the main window. "Switch Workspace"
  shows the same wizard and stores the choice for the next start.
- **Opening projects (M3c):** a project locked by another application
  shows upstream's directory lock dialog ("Open anyway" after "I accept
  the risk." for locks of other users or unknown applications); in
  addition to upstream, "Open Read-Only" opens it without lock. Other
  errors while locking the directory (e.g. no write permission) open the
  project read-only with a notification. An autosave backup (after a
  crash) is restored after upstream's question (Yes/No/Cancel). Opening
  works in steps (the file system reports the lock or the backup, the
  prompt is shown, opening continues with the answer) instead of blocking
  callbacks. `*.lppz` archives are extracted into a new temporary
  directory and opened read-only, like upstream.
- **File format upgrade notification:** like upstream; the migration log
  is written to `logs/migration_preview.html` of the project and opened
  with the default application (not the configured external web
  browser). Libraries opened in an older file format show no message
  (upstream neither).
- **Autosave (M3c):** one timer for all open projects with the interval of
  the workspace settings (upstream: a timer per project); a project which
  is busy (locked by a worker or the MCP server, or an undo group is
  active) is skipped until the next interval (upstream retries after 10 s).
- **New project wizard (M3c):** the pages are shown in a form dialog with
  "Back"/"Next"/"Finish" (upstream `QWizard`); the EAGLE files are parsed
  synchronously when their paths change; the location is not remembered
  per workspace (upstream client setting `new_project_wizard/location`)
  and the EAGLE file paths are not remembered; the EAGLE import messages
  are shown in a notification (upstream: a separate log window). "New
  project" in a folder of the home tab's folder tree creates the project
  in that folder (upstream `setLocationOverride()`).
- **Project library tab and updater (M3c):** the updater does not close a
  project with unsaved changes (upstream asks whether to save it); the
  update is aborted with a message instead. The updater dialog is closed
  and shown again with the log after the update (upstream keeps the
  window open). Opening an element of the project library tab opens the
  workspace library element (like upstream).
- **Workspace settings dialog (M3c):** all settings are applied with
  "OK"/"Apply" (upstream applies the theme, language, grid styles and
  color schemes immediately and reverts them on "Cancel"); the language
  change applies to the running application. "Restore Defaults" asks by
  showing the question as an error message and restores on the second
  click (upstream: a message box). Keyboard shortcuts are edited as text
  (Qt portable key sequences, one per line; only the first chord of
  multi-chord sequences is used) instead of recording key presses, the
  commands are listed without upstream's categories (the `.slint` command
  set has none) and overridden shortcuts are marked with a color swatch
  (upstream: bold). The API endpoint table is a list with check boxes for
  the selected endpoint; the library locale/norm order lists take free
  text (upstream: combo boxes of all locales/norms). The desktop
  integration and the 3D view color schemes are not available; the
  rendering method setting is stored but has no effect.
- **Color schemes:** the schematic and board color schemes of the
  workspace settings (user defined schemes, active scheme) are read and
  written like upstream (`(active <uuid> "<name>")`, `(scheme ...)` with
  the loaded nodes kept); the default (no user scheme, first base scheme
  active) removes the settings entry. The color scheme editor is upstream's
  `colorschemedialog.slint` shown as overlay. The schematic and board tabs
  use the primary colors of the active schemes (applied after "OK"); the
  secondary (highlight) colors and the grid color of the schemes are not
  used by the scenes yet, and the library element editors (symbol,
  package) and the previews always use the default schemes.
- **Grid interval and unit** changed in a tab modify the schematic or the
  board settings like upstream (so the editor tools snap to the shown
  grid), but as an undoable step ("Change Grid Properties"); upstream
  changes them without undo command.
- **Scene editing (schematic and board tabs):** the editor FSMs of
  `librepcb-editor` get the Slint pointer/key events; double clicks are
  detected like upstream (500 ms) but allow 2 px of movement between the
  presses (upstream: same scene position). Overlays (selection rectangle,
  ruler, scene cursor, gray-out) are canvas items. Cross-probing highlights
  the selected symbols/devices and the nets of selected wires, traces and
  vias (and the nets of the trace being drawn) in the other tabs of the
  project with the selection color (upstream: a separate highlight
  state, also driven by hovering). A tool which keeps an undo group open
  is aborted when another tab of the project is used, becomes current or
  is closed, and before undo, redo and save.
- **Context menus** of the scenes are a Slint popup built from the FSM's
  entries (upstream: `QMenu`).
- **Dialogs (M3b):** upstream's Qt Widgets dialogs are Slint overlays of
  the main window (one at a time; the application keeps running). Most
  are generic *form dialogs* (`ui/dialogs/formdialog.slint`,
  `src/dialogs/`) built from upstream's widgets with the labels of the
  upstream `.ui` files; the layout differs (one column of labeled fields,
  pages as tabs, radio buttons drawn as check boxes). Each dialog applies
  its changes as one undo group ("OK"/"Apply"); "Cancel" discards them.
  Details per dialog are in the module docs of `src/dialogs/*.rs`; the
  main differences by design (missing parts: see Part 2, app):
  - *Symbol properties:* the name swap question is shown in the dialog
    ("click OK again to swap"); library names are plain text.
  - *Device properties:* the device and footprint are chosen from combo
    boxes (upstream: context menu only).
  - *Path vertices* (polygons, zones, ...) are edited in place.
  - *Board setup:* the inner layer count is a combo box.
  - *Project setup:* net classes are renamed below the list; assembly
    variants are applied with the other settings (upstream: immediately).
  - *Add component:* choosing a part selects its device. "Add more"
    reopens the dialog whenever the placement ends with the select tool
    (upstream: after Escape).
  - *Graphics export:* edits a graphics output job (like the output jobs
    dialog) and writes into the output directory (`output/<version>/`,
    path with `{{PROJECT}}` etc.) instead of asking with a file dialog;
    format by file extension. Colors are named by their color role where
    no layer exists.
  - *Output jobs:* jobs are added from a combo box; running reports in
    notifications instead of a log panel.
  - *BOM review:* the custom attributes are stored (undoable) when
    closing with "OK" (upstream: while typing, without undo).
  - *Pick&place generator:* generates through an output job in a worker
    thread with paths relative to `output/<version>/` (upstream:
    `./output/{{VERSION}}/...` relative to the project), closes the
    dialog.
  - *Move/align:* the new positions are applied when accepting (upstream:
    live while editing); used by the package editor.
  - The workspace settings, directory lock, autosave restore, new
    project wizard, project library updater, initialize workspace and
    library import wizards (M3c/M4c) are described in their own entries.
- **Clipboard:** copy/paste uses the system clipboard with upstream's MIME
  types through `clipboard-rs` (X11, Wayland, macOS, Windows). Exchange
  with upstream LibrePCB (same version) works on X11/Wayland; on Windows
  and macOS Qt stores custom MIME types under its own format names, which
  are not reproduced. Without a system clipboard (no display, or
  `LIBREPCB_NO_SYSTEM_CLIPBOARD` set), an in-app clipboard is used.
- **M3d tools in the tabs:** buses (the tool button's menu of existing
  buses starts the tool with the chosen bus), images, the standalone pad
  tools and DXF import work like upstream, with these differences (gaps:
  see Part 2, app):
  - *Bus member menu:* the "Add New Bus Member" / nets / "Cancel" menu is
    the scene's Slint popup; closing it by clicking into the scene counts
    as "Cancel" (upstream's `QMenu::exec()` is modal).
  - *Images* (schematic and symbol editor): PNG, JPEG and SVG files only
    (upstream converts other formats supported by Qt's image readers to
    PNG); the image file name is not asked for (the file's base name is
    used). In the symbol editor, images are hit tested by their rectangle
    in the tab (the scene draws only their borders).
  - *DXF import dialog* (board, symbol and package editors): a form
    dialog; the scale factor is a text field.
  - *Find:* the zoom rectangle is the bounding box of the highlighted
    scene items (upstream: graphics items of the found objects), with
    upstream's margin.
  - *Plane visibility* (plane context menu "Visible") is a model setting
    applied without undo step, like upstream not saved.
  - *Cross-probing* also highlights the pins or pads of the component
    signals the other tab reports (`fsmCrossProbe()`) and buses.
- **Library management (M4a):**
  - *Libraries panel:* the installer of `librepcb-network` downloads the
    ZIP and falls back to cloning the repository with git.
  - *Library tab:* the checks run synchronously after each change.
  - *Undo stacks:* the library tab and every element tab have their own
    undo stack (upstream shares one per library editor).
- **Library element editors (M4b):**
  - *Package editor:* keepout zones are drawn filled in the zone color
    with a hairline outline. 3D models are stored as-is (see Part 2 for
    the missing 3D support).
  - *Wizard of new elements:* the pages and the saving per page follow
    upstream; for components, the variants page is still part of the
    wizard (adding gates creates the signals) like upstream. The checks
    run only after the wizard, like upstream. New elements are named
    like upstream (`"New Symbol"`, ...; not translated) and new packages
    and components get a `"default"` footprint or symbol variant.
  - *Interface protection:* elements opened as new or duplicated keep
    being writable while their tab is open (upstream `mIsNewElement`);
    others become read-only after an interface-breaking change until
    "unlock" (upstream: same).
  - *Symbol previews* in the component and device tabs are images
    rendered with `librepcb-scene`; texts are shown raw (`{{NAME}}`).
  - *Device pinout:* "auto-connect" and "load from file" keep the
    existing connections (upstream asks whether to reset them first).
  - *Categories and organizations* use a snapshot undo stack over the
    element's metadata and content (like the other element editors).
  - *Slint 1.18.1 workaround:* the footprint tags panel of the package
    tab has a fixed border radius for its "new tag" field (see
    `ui/PROVENANCE.md`, item 9).
- **Keyboard shortcuts:** `Backend.is-shortcut` compares with the user's
  overrides of the workspace settings if a command has any (an empty list
  disables it), else with the default shortcut of the `.slint` command set
  (upstream's C++ command set has alternative default shortcuts for some
  commands, which are not available). The overrides are global to the
  process (shared by all windows).
- **Library import wizards (M4c):** the EAGLE and KiCad wizards show their
  pages in a form dialog; the element tree is one checkable list per kind
  (a partially checked element, i.e. a dependency of a checked element, is
  marked with an orange swatch; KiCad symbols have one row each for the
  symbols, the component and the device); the EAGLE library is parsed and
  the KiCad libraries are scanned and parsed synchronously when going to
  the next page (upstream: in a background thread with progress); the
  additional component/package category is chosen from a combo box of the
  workspace categories (upstream: category chooser dialog); the options
  and paths are not remembered in the client settings. The import itself
  runs in a worker thread with progress and is canceled when the dialog
  is closed (upstream asks first), and the libraries are rescanned
  afterwards.
- **Embedded MCP server** (no upstream counterpart): a status bar button
  (next to the notifications button) and `--mcp[=ADDR]` start LibrePCB's
  MCP server inside the application (`127.0.0.1:8766`). The agent edits the
  project of the active tab through the same undo stack; workspace
  switches by the agent are refused.
- **Rule checks:** approving or unapproving an ERC/DRC message is an
  undoable command (`SetErcApproval`/`SetDrcApproval` mutations; upstream
  modifies the project outside the undo stack). Approvals of messages
  which disappeared are removed after each run like upstream (no undo
  step, the project becomes modified), but not while an undo group is
  active (e.g. a tool is drawing; the next run removes them) or the
  project is read-only.
- **Plane rebuilds:** the outdated planes on the visible copper layers of
  the boards shown in board tabs are rebuilt in a worker thread, at most
  once per second, checked by the project poll timer (250 ms; upstream: a
  100 ms timer); "Rebuild All Planes" rebuilds all planes of the board.
- **DRC** runs in a worker thread which locks the project only to rebuild
  the planes and air wires and to extract the check data.
- **Outputs from the menus:** PDF and image export, pick&place, the BOM
  review and "Output Jobs" open their dialogs (see above);
  Gerber/Excellon, IPC-D-356A netlist and `*.lppz` export run without
  dialog with default settings into `<project>/output/<version>/` and
  report the files in a notification.

## STEP and mechanical board export

- STEP minification ports upstream's fixed-point entity deduplication and
  protects product/representation identities. It operates on the source
  document, retaining units, styles, assembly links and header/footer records;
  it does not use the flattened BRep exporter. Quoted `#123`, `-0.`, semicolons,
  escaped quotes and multiline literal whitespace are retained instead of
  being rewritten/split by upstream's regular expressions. DATA comments are
  treated as layout. Duplicate IDs, unresolved references and multiple DATA
  sections fail explicitly; minification is not a general BRep validator.
  Library commands validate changed geometry before staging a replacement;
  only `--save` commits the transaction. Standalone `--save-to` deliberately
  writes before model validation, matching upstream's diagnostic workflow.
- Unchanged board contours retain analytic arcs; contours changed by Boolean
  clipping use the existing 0.005 mm polygon approximation with a CLI warning.
  Upstream uses OpenCascade Booleans and falls back to 0.01 mm segments only
  when arc construction fails. Via drills are omitted by both mechanical
  exporters. Physical rendering needs a separate drill-depth policy.
- All model preparation and serialization complete before opening the STEP
  output. A failed model leaves an earlier output intact; upstream can create
  a partial assembly before returning a component-export error. Invalid suffix
  errors identify the format as STEP instead of upstream's copied netlist label.
- CLI, standalone/embedded MCP and desktop jobs install the same STEP
  exporter. Core retains output ownership, archives and cleanup without a
  CAD dependency. CPU mesh subdivision follows Monstertruck, so vertex counts
  can differ from OpenCascade.

# Part 2: Not ported yet / known gaps

## export

- **`GraphicsExportSettings` colors:** color schemes are not in core yet,
  so the default colors of `BaseColorScheme::schematicLibrePcbLight()` and
  `boardLibrePcbDark()` are embedded (`load_default_colors()` replaces
  `loadColorsFromScheme()`).
- **Graphics export:** printing, previews and the asynchronous API
  (progress signals, cancellation, clipboard) are not ported.

## network (crate librepcb-network)

- The HTTP disk cache is not ported (`setCacheLoadControl()`,
  `setMinimumCacheTime()`, `AlwaysCache`); `ApiEndpoint`'s `forceNoCache`
  parameter is therefore dropped.

## scene (crate librepcb-scene)

- Images (schematic and symbol images) are not drawn yet, only their
  borders.
- Board graphics export (`BoardPainter`): with no copper layer enabled,
  THT pads and vias are drawn from the top copper instance only, so vias
  not reaching the top layer are missing.

## workspace

- The workspace initialization (`Workspace::open_or_create()`) does not
  download the example projects.
- Not ported: UI themes and color scheme logic (`UiTheme`, `ColorRole`,
  `BaseColorScheme`, `UserColorScheme`).

## editor (crate librepcb-editor)

- **Removing traces as a side effect** (disconnecting component signals,
  replacing devices, changing the net of a schematic segment) does not
  remove unused project library elements; upstream's nested
  `CmdRemoveBoardItems` does. The explicit `RemoveBoardItems` and all
  schematic removals do, like upstream.

## STEP and mechanical board export

- The pure Rust CAD adapter currently flattens exported assembly hierarchy and
  drops unrelated metadata. Mixed length contexts, non-radian angle units,
  unsupported surface presentations and non-solid models fail explicitly.
  These are coverage gaps, not intentional file-format changes.
- Kernel unwinds become typed errors; size/nesting limits do not provide
  process isolation, cancellation or memory/time limits.

## app (crate librepcb-app)

- **Board 3D viewer:** Bevy renders the shared physical board geometry and STEP
  component meshes into an offscreen image. The current Slint adapter reads
  pixels back from the GPU; direct shared-texture presentation is not wired yet.
  A supported graphics adapter is required even with Slint's software renderer.
  There are no environment-map presets, manufacturing stackup editing, solder
  fillets or package-editor 3D previews. Drill depths use evenly spaced enabled
  copper layers because the model does not provide dielectric layer thicknesses.
- **Project lifecycle (M3c):** example projects are not downloaded when a
  workspace is created; wizard locations, EAGLE/KiCad import paths and
  options are not remembered in the client settings; the project library
  updater aborts instead of offering to save a project with unsaved
  changes; the language list shows locale codes instead of native names.
- **Color schemes:** the scenes use only the primary colors of the active
  schemes (no secondary/highlight or grid colors), and the symbol/package
  editors and previews always use the default schemes; 3D view color
  schemes and desktop integration are not available.
- **Import wizards (M4c):** EAGLE parsing and KiCad scanning/parsing run
  synchronously on the UI thread (upstream: background thread with
  progress); categories are chosen from a combo box instead of the
  category chooser.
- **Layers panel** lists the board layers used by the board in
  `Layer::all()` order, and the layer presets (top, bottom, ...) are
  simplified; the visibility is not stored in the board user settings yet.
- **Scene editing:** the ruler has no tick labels. Hovered items are not
  highlighted.
- **Context menus:** resource (datasheet) entries are missing.
- **Dialogs (M3b)**, missing parts (see Part 1 for the differences by
  design):
  - *Symbol properties:* assembly options can be removed and their
    assembly variants edited, not added.
  - *Pad properties:* holes and custom pad outlines are not editable.
  - *Path vertices:* vertices cannot be added or removed in the dialogs.
  - *Board setup:* DRC presets of organizations cannot be loaded yet
    (only "Reset to Default Settings" and "Remove Link to Imported
    Settings").
  - *Project setup:* locales are shown and added by their code (e.g.
    `de_CH`) instead of the language name; assembly variants cannot be
    reordered.
  - *Add component:* the symbol preview shows the first gate only; no live
    part information and no context menu.
  - *Graphics export:* no page preview, no printing, no copy to
    clipboard, all schematic pages; colors can be enabled/disabled but not
    changed.
  - *Output jobs:* no organization presets, no "Import Old Settings";
    unknown files in the output directory are not listed or removed; the
    interactive HTML BOM job's check box and component order lists are
    not editable.
  - *BOM review:* no live part information (availability, prices).
  - *Pick&place generator:* no "Browse Output Directory".
- **M3d tools in the tabs:**
  - *DXF import dialog:* its choices are remembered only while the
    application runs (upstream: in the client settings).
  - *Find:* the suggestions have no icons.
- **Not available yet in the tabs:** the unplaced components panel;
  "Show All Planes"/"Hide All Planes" (board).
- **Library management (M4a):**
  - *Libraries panel:* no automatic update check or installation timer;
    the online list is fetched when the panel is shown or "check for
    updates" is clicked. The download progress of a library is "running"
    or "done" only.
  - *Library tab:* moving/copying elements into other libraries is not
    ported. Closing a library tab discards its unsaved changes without
    asking.
  - *No file system watcher:* the "files modified" banner of the tabs is
    never shown; elements changed on disk are not reloaded.
- **Library element editors (M4b):**
  - *Symbol and package editors:* graphics export, printing and the
    background image are not available in the tabs yet; images in the
    symbol editor are invisible unless they have a border (the scenes do
    not draw images yet, see "scene"). There is no 3D view in the
    package editor: the 3D models can be added (the STEP file is stored
    as-is, not minified and not validated since there is no
    OpenCascade), renamed, replaced, reordered, removed and assigned to
    footprints with their transform, but not shown.
  - *Symbol previews:* the pins are labeled with the connected signal
    names (component tab) but not with the pad numbers (device tab). The
    device previews are not zoomable and have no measure tool.
  - *Chooser dialogs:* symbols, components and packages are chosen from
    one searchable list with the description of the selected element
    (upstream: a category tree, the list and a graphical preview).
  - *Organizations:* the PCB design rules and the output jobs of an
    organization cannot be edited yet (upstream opens the board setup
    and output jobs dialogs on a temporary project); design rules can be
    added (with default settings), renamed, copied and removed.
- **Keyboard shortcuts:** no alternative default shortcuts (upstream's
  C++ command set has some); the shortcut editor takes text key sequences
  instead of recording key presses and has no command categories.
- **Zooming** is not animated.
- **Rule checks:** selecting a message zooms to its location, but no
  location marker is drawn; automatic fixes are not available yet.
- **Outputs from the menus:** printing and Specctra export are not
  available yet.
