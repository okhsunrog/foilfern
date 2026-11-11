# U2 VDD and VCONN decoupling review

2026-11-11. Continues the [OLED RESET batch](../reset/REPORT.md) at `7d65aa2`.
The input is its saved compact board; previous OLED, RESET and U5 improvements
are preserved.

The selected variant places both 1 µF capacitors nearer their own U2 supply
pins, with local ground vias. It has **0 unrouted connections, 0 ERC findings,
0 new DRC errors or warnings**. Three inherited errors and nine warnings remain.

Latest native project:
`/workspace/esp-pd-sink/u2-review/selected/ESP-PD-Sink.lpp`.
[artifacts.tar.gz](artifacts.tar.gz) contains this project, the input RESET board,
a C30-only alternative, the accepted pre-chamfer trial, raw MCP evidence and
independent fabrication exports. After extraction, continue from
`artifacts/native/selected/ESP-PD-Sink.lpp`.

## Measured result and tradeoff

U2's library pin mapping identifies pins 3/4 as VDD on VCC, and 12/13 as
VCONN_1/VCONN_2 on +5V. C29 and C30 are both 1 µF; their values, packages,
assembly options and net assignments are unchanged. Group membership is manual
and follows those pins, not common supply-net inference. The input's U2 circuit
value `FUSB302MPX` and library identity `FUSB302BMPX` are preserved; this review
does not resolve the exact purchasing MPN or qualify its ratings.

Lengths are trace centerlines between pad centers, excluding via barrels and
plane-only paths. Each accepted alternative was prepared against the same input
geometry, reviewed and applied on its own native copy.

| Path | Input, mm | C30-only alternative, mm | Selected, mm |
| --- | ---: | ---: | ---: |
| U2.3–C29.2, VDD | 6.686 | 6.686 | 1.176 |
| U2.4–C29.2, VDD | 6.016 | 6.016 | 2.148 |
| U2.12–C30.2, VCONN | 6.145 | 2.681 | 2.681 |
| U2.13–C30.2, VCONN | 6.645 | 3.181 | 3.181 |
| U2.5–U1.6, INT | 28.232 | 28.232 | 30.773 |

Both selected capacitor paths stay on **Top**, with **zero trace-layer changes**.
Originally they changed layer twice: VDD used Bottom, VCONN used In2. The VDD
path now visits no via anchors. The VCONN path visits one existing via anchor as
a coplanar Top junction; it does not traverse its barrel. Accordingly, the
measurement JSON reports `via_count: 1` but `layer_changes: 0` for VCONN. Via
anchors on a path must not be confused with layer changes.

C29 moves from `(115.2, -130.3)`, 90°, to `(109.5, -132.4)`, 0°, beside VDD.
C30 moves from `(115.2, -132.4)`, 90°, to `(114.5, -128.9)`, 180°, nearer VCONN.
Both remain on Top. U2 itself, connectors, ADC divider and all other footprints
remain fixed.

The original INT route crossed the new C29 footprint and ground-return site.
Two explicitly selected INT segments are replaced by a detour using the same
0.18 mm width. The first accepted detour added 3.010 mm; chamfering its two
corners saves 0.469 mm, leaving a **2.541 mm INT increase** in the selected
variant. Its existing two layer changes remain. INT timing/crosstalk were not
simulated or measured. The C30-only alternative preserves the original INT and
VDD geometry and is retained for comparison.

Supply branches retain 0.4 mm width. C30 was previously part of a through feed;
its old neighboring +5V junctions are explicitly reconnected after moving it.
The C29 supply via remains a shared global feed hub and is preserved. No supply
via is removed merely to improve the local path's via count.

## Ground return and protected geometry

One existing ground via per capacitor is relocated, preserving its 0.3 mm drill
and 0.6 mm diameter. Other original ground stitching vias are retained.

| Return lead | Shortest original explicit lead, mm | Selected lead, mm | New via position, mm |
| --- | ---: | ---: | --- |
| C29.1–GND via | 0.850 | 0.620 | (108.4, −132.4) |
| C30.1–GND via | 0.620 | 0.670 | (115.65, −128.9) |

C29's selected ground lead is 0.3 mm wide; C30's is 0.2 mm. `plane_diagnose`
confirms both sites belong to the connected In1 GND fragment. Rebuilt whole-board
checks show no open GND connections. The C30 return lead is 0.05 mm longer;
this is not presented as an improvement in every return metric. Paths through
the plane to U2's ground pads are not trace-only measurements. Supply/return
loop inductance, effective capacitance, transient response and VCONN switching
performance still require modelling or measurements.

[Geometry comparison](geometry-comparison.json) checks every retained trace and
via by UUID: coordinates, endpoints, layers and widths are unchanged, allowing
net-segment IDs to change on split/merge. Only the listed eleven traces and two
vias are removed, and only C29/C30 placements change. CC1/CC2, USB, RF, earlier
OLED/U5 routes and the ADC divider's copper are preserved. Outline, holes,
keepouts, stack and plane settings are unchanged. Saved `circuit.lp` is
byte-identical to the input in all three accepted variants.

| Variant | Trace segments | Vias | Whole-board centerline, mm |
| --- | ---: | ---: | ---: |
| Input RESET board | 980 | 142 | 1508.391 |
| C30 only | 980 | 142 | 1508.020 |
| Both, before corner trimming | 979 | 142 | 1508.246 |
| Selected, with chamfered INT corners | 981 | 142 | 1507.777 |

The selected variant prioritizes local VDD/VCONN paths over global segment
count. More segments result from the chamfered corners; via count is unchanged.

## Checks and evidence

The first placement trial left INT unchanged and was never applied: it preserved
connectivity but added four Top clearance violations involving C29 and its
return. Its exact arguments and report are archived. The bounded INT detour
removes those conflicts. No Rust source or validation rule was changed.

Final selected-model checks: ERC, full DRC with rebuilt planes, unrouted,
explicit save and history. History shows one undo step and no unsaved changes.
Every DRC message and location exactly equals the input: two J2 copper–hole
errors, AE1:2 in the antenna keepout, eight plated-slot warnings and one
outline-radius warning. Nothing was approved away.

[Reference comparison](reference-comparison.json) matches before, candidate and
applied models for In1 GND under CC/USB/RF. Bottom CC traces were also checked
against In2 GND; those results match exactly. The coverage tool checks adjacent
signal layers only, so both reference layers are needed for CC routing. Existing
uncovered intervals remain, including CC gaps; no new gaps are introduced.
This is preservation evidence, not USB/PD impedance or RF qualification.

After closing MCP, LibrePCB 2.1.1 reopens the selected native project and reports
0 ERC findings and the same twelve inherited DRC findings (check exit 1 for
those findings). Independent fabrication export completes with exit 0. All
thirteen Gerber/Excellon files [match Foilfern](fabrication-comparison.json) after
excluding generator version, creation date and dependent checksum metadata.
The native archive is hash-verified and its extracted selected project is
reopened before delivery.

- [Before](baseline-region.png), [C30-only alternative](vconn-only-region.png),
  [selected region](selected-region.png): Top, Bottom and In2 in board coordinates.
  Orange is VDD/VCC, purple VCONN/+5V, green INT, gray GND, blue other signals.
  Pours and package bodies are omitted; these are review plots, not fab previews.
- [Top](board-top.png) and [bottom](board-bottom.png) are the selected Foilfern
  board renders. Both are inspected; crowded silkscreen remains a separate task.
- [Selected MCP arguments](selected-input.json) and
  [C30-only arguments](vconn-only-input.json) reproduce candidates against the
  input RESET board. Each write used prepare/preview/apply with revision guards.
- [Input](baseline-measurements.json), [C30-only](vconn-only-measurements.json)
  and [selected measurements](selected-measurements.json) include distances,
  trace UUIDs, widths, layers, via-anchor counts and actual layer-change counts.
  [measure_u2.py](measure_u2.py) extends the shared measurement script with that
  last count; [intent.json](intent.json) retains the manual group definitions.

The preconfigured environment and existing optimized MCP binary worked without
changes. No new dependency or environment blocker was encountered.

Next: **U6/R17 Kelvin routing**, followed by the ADC C4 alternatives and SW5-mode
review. Continue from the selected project above. Router baseline reuse, other
performance work, silkscreen/mechanical review, inherited violations and hardware
qualification remain open. This work does not establish 48 V / 5 A suitability.
