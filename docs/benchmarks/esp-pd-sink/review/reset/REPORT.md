# OLED RESET placement and routing

2026-11-11. Continues the [first batch](../REPORT.md) at `f58b690`. The input is
that batch's combined board, not the original unmodified handoff project.

The selected compact variant shortens all three OLED RESET branches and removes
both RESET vias. It has **0 unrouted connections, 0 ERC findings, 0 new DRC
errors or warnings**. The three inherited errors and nine warnings remain.

Latest native project:
`/workspace/esp-pd-sink/reset-review/compact/ESP-PD-Sink.lpp`.
[artifacts.tar.gz](artifacts.tar.gz) preserves the input combined project and
both applied alternatives. Extract with `tar -xzf artifacts.tar.gz`; continue
from `artifacts/native/compact/ESP-PD-Sink.lpp`.

## Comparison and decision

Both alternatives were prepared against the same input geometry and explicitly
reviewed with `routing_preview` before applying on separate native copies.
Lengths are trace centerlines between pad centers; via barrels are excluded.

| Path from OLED1.14 | Input, mm | Fixed footprints, mm | Selected compact, mm |
| --- | ---: | ---: | ---: |
| R22.2 | 7.890 | 3.030 | 3.030 |
| C27.1 | 12.125 | 8.200 | 4.055 |
| D4.2 | 15.064 | 11.139 | 4.872 |

The input paths use Bottom and Top with two vias each. Both alternatives use
Bottom only with zero RESET vias. The compact variant also reduces the local
R22.2–C27.1 path from 5.169 to 1.025 mm and C27.1–D4.2 from 2.939 to 0.817 mm.
R22 remains fixed. C27 moves from `(156.0, -113.2)`, 90°, to
`(154.6, -110.2)`, 180°. D4 moves from `(157.9, -113.0)`, 0°, to
`(155.0, -111.7)`, 90°. Both remain on Bottom. Component values and signal
assignments are unchanged: R22 = 4.7 kΩ, C27 = 2.2 µF, D4 = 1N4148.
No reset timing or part-rating requirement was changed or newly qualified.

A VCC branch crossed the direct escape from OLED1.14. In both alternatives,
four selected Bottom VCC segments are replaced by a 0.4 mm Top connection from
the existing VCC via at `(155.7, -108.6)` to one new 0.3/0.6 mm via at
`(157.5, -108.6)`, then two Bottom branches. This adds one VCC via and removes
the two RESET vias: the board goes from **143 to 142 vias**. In the compact
variant, C27's existing ground via is relocated from `(155.84, -112.034)` to
`(153.9, -110.8)`. `plane_diagnose` confirms that site belongs to the connected
In1 GND fragment; no ground connection opens.

The chosen geometry accepts a longer VCC feed in exchange for a shorter,
more local RESET node:

| Supply/reference path | Input, mm | Selected compact, mm | Via count before → after |
| --- | ---: | ---: | --- |
| OLED1.15–R22.1 | 5.012 | 6.585 | 0 → 2 |
| OLED1.15–C26.2 | 6.655 | 7.330 | 0 → 2 |
| R22.1–D4.1 | 2.700 | 4.250 | 0 → 0 |
| OLED1.9–C26.2 | 3.107 | 3.107 | 0 → 0 |
| OLED1.11–C26.2 | 3.558 | 3.558 | 0 → 0 |

OLED1.15 is the VCC-tied D/C input, not the supply entry point. The via counts
above describe those measured pad-to-pad paths. Local C26 decoupling paths and
all four charge-pump branches retain their exact geometry. The earlier OLED
VCOMH/VCC and U5 improvements are preserved. The compact variant is preferred
for its shorter capacitor/diode branches; the fixed-footprint alternative is
also retained as a valid option. These comparisons do not establish supply
transient behavior, reset timing or power-cycle performance.

Whole-board totals:

| Variant | Trace segments | Vias | Trace centerline, mm |
| --- | ---: | ---: | ---: |
| Input combined board | 994 | 143 | 1517.093 |
| Fixed footprints | 992 | 142 | 1513.112 |
| Compact | 980 | 142 | 1508.391 |

## Review evidence

- [Before](baseline-region.png), [fixed alternative](fixed-region.png) and
  [selected compact region](compact-region.png) show copper in board coordinates:
  green RESET, orange VCC, gray GND, blue other signals. Plane fills and package
  bodies are omitted; these are review plots, not fabrication previews.
- [Top](board-top.png) and [bottom](board-bottom.png) are Foilfern board renders.
  Both were generated and inspected. Existing crowded text still needs the
  separate silkscreen pass.
- [Fixed input](fixed-input.json) and [compact input](compact-input.json) are exact
  `routing_copper_prepare` arguments against this batch's input. Every trace/via
  removal is explicitly listed; no raw native-file edits were used.
- [Input measurements](baseline-measurements.json),
  [fixed measurements](fixed-measurements.json) and
  [compact measurements](compact-measurements.json) include direct distance,
  trace length, layers, widths, vias and trace UUIDs. They use the manual
  [intent/groups](intent.json) and the shared [measurement script](../measure.py).
- [Geometry comparison](geometry-comparison.json) confirms retained copper has
  identical anchors, coordinates, layers and widths, allowing only net-segment
  IDs to change on split/merge. Only C27/D4 placements change. Outline, holes,
  keepouts, copper layers and plane settings are unchanged. Saved `circuit.lp`
  is byte-identical to the input.

One compact trial was rejected and never applied: its C27 ground via at
`(153.4, -110.2)` was too close to an existing 0.4 mm VCC trace on In2 at
`y = -109.8`. Moving the via to `(153.9, -110.8)` removes the violation. The
trial preserved connectivity but added one clearance error, so it was left unapplied.
Its exact input and validation are in the archive. This reinforces the need
for all-layer via-site inspection, even when the visible Bottom region is open.

## Verification and continuation

Final applied-model checks ran through MCP: ERC, full DRC with rebuilt planes,
unrouted, explicit save and history. History shows one undo step and no unsaved
changes. All final DRC messages and locations exactly equal the input: two J2
copper–hole errors, AE1:2 in the antenna keepout, eight plated-slot warnings and
one outline-radius warning. Nothing was approved away.

[USB/RF reference coverage](reference-comparison.json) is exactly equal before,
in the candidate, and after application, including every uncovered interval.
The USB/RF footprints, traces and vias are unchanged. Existing USB return gaps
remain; centerline coverage is not impedance qualification.

After closing MCP, LibrePCB 2.1.1 reopened the native compact project and returned
0 ERC findings and the same twelve inherited DRC findings. Independent
fabrication export completed successfully. All thirteen Gerber/Excellon outputs
[match Foilfern](fabrication-comparison.json) after excluding generator version,
creation date and dependent checksum metadata. Native archive contents are
hash-verified and the extracted compact project is reopened before delivery.

The optimized MCP binary and upstream CLI remained available; no environment
changes or new environment blockers were encountered. No Rust source changed.
The existing check/prepare/apply path was used without weakening validation.

The next layout step is **U2 local decoupling**, followed by the U6/R17 Kelvin
and ADC C4 alternatives from the handoff. Start with the compact project above
and retain this batch's input for comparisons. Guarded baseline reuse and the
remaining performance work, mechanical/silkscreen review, inherited violations
and hardware qualification remain open. No 48 V / 5 A suitability is established.
