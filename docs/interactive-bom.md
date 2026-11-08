# Interactive assembly BOM

The `interactive_bom` output job generates a standalone HTML assembly
document. It contains the board drawings, footprint/pad identities, nets,
component groups and the HTML/CSS/JS viewer. No server, network connection,
Qt, Slint or GPU context is needed to open the exported file.

Use it next to the board while assembling: filter a value or reference,
select a BOM row to highlight its footprints, switch sides and mark parts
as sourced/placed. Marks persist in browser storage. Copying the original
HTML to another browser does not automatically transfer that progress.

## Export

Run the job by its configured name (this name is used by upstream fixtures):

```sh
target/debug/librepcb-cli open-project \
  --run-job 'Interactive BOM' --outdir assembly path/to/project.lpp
```

`.lppz` input is also supported. Output paths and selected boards/assembly
variants come from the project's job configuration, or from a file passed
with `--jobs`. Supported suffixes are `.html`, `.htm` and `.xhtml`, ignoring
case. Open the resulting file directly in a browser, for example with
`xdg-open assembly/asm/project_version_BOM_variant.html`.

The job rebuilds copper planes first and uses the indexed output writer.
This preserves archive dependencies, duplicate-name checks and removal of
obsolete HTML after a successful configuration change. Other jobs can package
the assembly documents in a ZIP.

## Implementation and compatibility

`BoardInteractiveBomGenerator` resolves the existing project views into
`interactive-html-bom` 0.2.0, the same Rust generator used by upstream
LibrePCB. Pad geometry, fonts and attribute lookup reuse the core model.
Assembly geometry includes footprint bounds, transformed pads and slots,
silkscreen/fabrication drawings, tracks, vias and current plane fragments.
The adapter stays synchronous, read-only and `Send + Sync`.

Mounted parts are grouped by value/MPN, package and custom columns. DNP parts
retain geometry and a skipped marker. Designators use the core's numeric
sorting with configurable component-prefix priority. Job settings control
view layout, pin-1 highlighting, theme, rotation, silkscreen/fabrication/pad
visibility and checklist columns.

The source representation uses the first copper outline and first hole of
each pad, approximates a slot from its first segment, omits standalone-pad
copper and exports only outer-layer copper. These source limits are preserved.
Upstream currently does not apply the serialized `show_tracks`/`show_zones`
job flags; this runner also retains all copper data. The viewer's own
visibility controls remain available.

## Validation

- The original upstream `BoardInteractiveHtmlBomGeneratorTest` HTML matches
  byte for byte at its fixed timestamp. Its decompressed PCB data is checked
  against the original JSON, with geometry compared at its native f32
  precision to account for JSON pretty-printer rounding.
- Fixture tests cover assembly selection, custom columns, DNP, file-content
  preservation, output indexing, ZIP contents, cleanup, collisions and plane
  freshness. Output-job comparisons include official CLI HTML configuration
  and decompressed PCB data, ignoring generation time.
- The [official CAN–USB example](https://github.com/LibrePCB/librepcb-example-projects/blob/3aad10dae67ecfea9535a2cac328497d67e4d142/can2usb.lppz)
  was exported by both implementations. HTML/configuration and decompressed
  data match after ignoring time, with the oracle using `LC_ALL=en_US.UTF-8`.
  Qt's `C.UTF-8` collation instead sorts references lexicographically; the
  Rust port keeps deterministic numeric sorting (see `COMPAT.md`).

Repeat browser acceptance with an installed Chrome/Chromium on `PATH`, or
pass its path with `--browser`. The script uses a fresh offline browser
context and leaves the input HTML unchanged:

```sh
uv run scripts/check-ibom-browser.py path/to/can2usb.html \
  --reference path/to/oracle.html --output path/to/screenshots
```

The optional `--reference` verifies data/configuration and the HTML template.
The check exercises filtering, footprint highlighting, checkbox persistence,
side switching and themes. It saves desktop (1440×960) and compact (390×844)
light/dark screenshots and a JSON report; JavaScript errors or HTTP(S)
requests fail the check. A fresh-context reload is covered; physical
touchscreen/tablet operation is a separate manual acceptance boundary.
