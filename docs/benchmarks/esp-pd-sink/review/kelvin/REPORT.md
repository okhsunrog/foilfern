# U6/R17 Kelvin routing review

2026-11-11. Continues the [U2 decoupling board](../u2/REPORT.md) at `75ae37a`.
The selected variant brings U6/C19 1.3 mm nearer R17 and routes both INA238
inputs on Top from the shunt's dedicated sense pads. It has **0 unrouted
connections, 0 ERC findings and 0 new DRC findings**. Three inherited errors
and nine warnings remain.

Latest native project:
`/workspace/esp-pd-sink/kelvin-review/selected/ESP-PD-Sink.lpp`.
[artifacts.tar.gz](artifacts.tar.gz) includes the input, fixed-placement control,
selected native project, intermediate draft, MCP records and independent
fabrication exports. After extraction, continue from
`artifacts/native/selected/ESP-PD-Sink.lpp`.

## Measured result

R17's four-terminal footprint maps pad 3 to IN+, pad 2 to IN−, pad 4 to VBUS
power and pad 1 to VOUT power. U6 pads 10/9 are Vin+/Vin−. The two sense nets
remain independent of the power pads in the circuit. R17's value (0.010 Ω),
package, all circuit assignments and assembly options are unchanged. The manual,
overlapping Kelvin and power-output groups retain stable resolved identities.

All lengths are shortest explicit trace centerlines between pad centers,
excluding via barrels and plane-only paths.

| Metric | Input | Fixed-placement control | Selected |
| --- | ---: | ---: | ---: |
| R17.3–U6.10, IN+, mm | 8.575 | 8.575 | 8.183 |
| R17.2–U6.9, IN−, mm | 7.565 | 7.485 | 7.488 |
| IN+ trace-layer changes | 2 | 2 | 0 |
| IN− trace-layer changes | 0 | 0 | 0 |
| Projected sense-loop area, mm² | 11.500 | 11.872 | 6.419 |
| U6.6–C19.2 supply, mm | 2.855 | 2.855 | 2.713 |
| U6.7–C19.1 ground, mm | 2.453 | 2.453 | 2.453 |

The selected sense paths use 0.2 mm copper and no via anchors. They converge
0.73 mm above the sense-pad center row, then run side by side toward U6.
The vertical section has 0.45 mm center spacing; the diagonal approach has
about 0.424 mm normal spacing. Both stay above R17, away from the U5/L3
switching-node copper. R17, U5, L3 and the power-output footprints stay fixed.

The **44.2% smaller projected area** is a closed 2D centerline polygon between
the two dedicated shunt pads and the two INA inputs. It omits barrel height,
internal package/shunt paths and surrounding fields. It is a geometry proxy,
not measured inductance, common-mode rejection, noise or current accuracy.
Length savings alone are modest: 0.392 mm on IN+ and 0.077 mm on IN−.
The control saves 0.081 mm on IN− but slightly increases projected area and
retains the two IN+ layer changes, so it is not selected.

U6 moves from `(124, −132)` to `(124, −133.3)`; C19 moves from
`(128.4, −133.3)` to `(128.4, −134.6)`. Their rotations and Top side are preserved.
C19's final supply branch connects directly to U6.6 using 0.3 mm copper;
the supply launch retains its 0.25 mm neck. The previous capacitor branch used
0.4 mm copper after its 0.25 mm pin neck. C19's ground route remains 0.2 mm.
A reviewed intermediate draft took the supply route back through the feed hub;
that 5.255 mm path was corrected before selection. The direct 0.4 mm launch
trial failed adjacent-pad clearance and was never applied.

## Power path and digital-routing tradeoff

A Top VBUS branch obstructed an all-Top IN+ route. Plane diagnosis shows three
separate VBUS pour fragments: J1 A9, J1 B9 and R17.4 occupy different fragments.
It would be unsafe to assume this branch is only an auxiliary tap. The selected
replacement **stays entirely on Top, adds no VBUS vias and widens from 0.4 to
0.6 mm**. Its new corridor lies to the left of the sense pads.

| Replaced VBUS section | Input | Selected |
| --- | ---: | ---: |
| Centerline length, mm | 13.586 | 16.363 |
| Width, mm | 0.4 | 0.6 |
| Sum of length/width, squares | 33.966 | 27.272 |

The longer section has a 19.7% lower rectangular-trace resistance proxy at equal
copper thickness. This excludes pour sharing, spreading resistance and thermal
conditions. It does not qualify current capacity. Plane settings remain fixed;
all three VBUS probe sites remain connected and whole-board connectivity is
preserved. The earlier Bottom-VBUS experiment was rejected and never applied.

SDA crosses this Top power corridor on Bottom; SCL crosses on In2. Each reuses
its existing left via and adds one via near U6. A low-current VCC feed crosses
on Bottom using one additional via. One ground via is added at
`(122.8, −132.5)`, attached to U6.2. C19's ground via is relocated to
`(129.1, −134.12)` with the same 0.3/0.6 mm drill/diameter. Both new ground sites
are in the connected In1 GND fragment; the existing ground stitching remains.

| Explicit I²C path | Input, mm | Selected, mm |
| --- | ---: | ---: |
| U6.4–U2.7, SDA | 10.115 | 12.636 |
| U6.5–U2.6, SCL | 10.952 | 13.704 |

SDA still changes trace layer twice along this path; SCL changes three times
instead of twice. Both visit three via anchors instead of two. Bus timing,
capacitance and crosstalk were not simulated. These costs are retained explicitly
in the comparison rather than presenting every metric as improved.

| Whole board | Trace segments | Vias | Centerline, mm |
| --- | ---: | ---: | ---: |
| Input U2 board | 981 | 142 | 1507.777 |
| Fixed-placement control | 977 | 142 | 1507.697 |
| Selected | 979 | 144 | 1523.689 |

## Validation and reproducibility

[Geometry comparison](geometry-comparison.json) checks every retained trace and
via by UUID, allowing netsegment IDs to change after split/merge. Only the
explicitly listed fifty traces and four vias are replaced; only U6/C19 move.
Outline, holes, keepouts, stack and plane settings are unchanged. CC, USB, RF,
ADC, earlier OLED/U2/U5 improvements and all other footprint placements are
preserved. `circuit.lp` is byte-identical in the input, control and selection.

[Reference comparison](reference-comparison.json) matches the exact candidate
preview to the applied model. In1 GND completely covers both selected sense
paths; IN+ previously had 2.197 mm of uncovered centerline. In1 coverage for
CC/USB/RF and In2 coverage for Bottom CC match the input exactly, including
inherited gaps. The new I²C signal vias create short uncovered intervals at their
clearances: total uncovered SDA length across adjacent reference layers grows
by 1.069 mm, and SCL by 1.093 mm. The added local ground stitch provides a nearer
return connection; it does not eliminate those signal-via clearances. These are
coverage measurements, not impedance or return-inductance qualification.

Each accepted variant used prepare, full preview and revision-guarded apply on
its own project copy. Final ERC, full DRC with rebuilt planes, unrouted, explicit
save and history are archived. Every final DRC message and location equals the
input exactly: two J2 copper–hole errors, AE1:2 in the antenna keepout, eight
plated-slot warnings and one outline-radius warning. Nothing was approved away.
History has one undo step and no unsaved changes.

After MCP closes, upstream LibrePCB 2.1.1 opens the control and selected native
projects with zero ERC and the same twelve inherited DRC findings (exit 1).
Independent selected fabrication export exits 0. All thirteen Gerber/Excellon
files [match Foilfern](fabrication-comparison.json) after excluding generator,
creation-date and dependent checksum metadata. The delivery archive is
hash-verified and its extracted selected project is reopened before delivery.

- [Before](baseline-region.png), [control](fixed-region.png) and
  [selected region](selected-region.png) show Top, Bottom and In2 centerlines,
  pad geometry and vias. Green is IN+, blue IN−, red VBUS, purple VOUT,
  orange VCC and gray GND. Pours and package bodies are omitted.
- [Top](board-top.png) and [bottom](board-bottom.png) are inspected Foilfern
  renders. Crowded silkscreen remains a separate review task.
- [Selected arguments](selected-input.json) and [control arguments](fixed-input.json)
  reproduce the candidates against the input U2 board.
- [Input](baseline-measurements.json), [control](fixed-measurements.json) and
  [selected measurements](selected-measurements.json) resolve manual group/pin
  identities and record exact trace UUIDs, widths, via anchors and layer changes.
  [Kelvin comparison](kelvin-comparison.json) records the projected-area and
  VBUS-section proxies. [measure_kelvin.py](measure_kelvin.py) uses the shared
  measurement script; [intent.json](intent.json) contains the manual requirements.

The preconfigured environment and optimized MCP binary worked without changes.
No dependency or environment blocker was encountered. No Rust source or
validation rules were changed.

Next: **ADC C4 placement alternatives and SW5 schematic-mode review**, continuing
from the selected project above. Router baseline reuse, silkscreen/mechanical
review, inherited violations and hardware qualification remain open. This work
does not establish 48 V / 5 A suitability.
