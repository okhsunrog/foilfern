# ADC C4 alternatives and SW5-mode review

2026-11-11. Continues the [Kelvin board](../kelvin/REPORT.md) at `1cff7a1`.
The selected variant moves C4 beside the MCU ADC input and connects it directly
on Top. It has **0 unrouted connections, 0 ERC findings and 0 new DRC findings**.
The three inherited errors and nine warnings remain unchanged.

Latest native project:
`/workspace/esp-pd-sink/adc-review/selected/ESP-PD-Sink.lpp`.
[artifacts.tar.gz](artifacts.tar.gz) contains the input Kelvin project, an accepted
via-junction alternative, the selected native project, raw MCP evidence and
fabrication exports. Continue after extraction from
`artifacts/native/selected/ESP-PD-Sink.lpp`. The rejected full-divider trial is
archived as arguments, preview and reconstructed read-only geometry; it is not
a native continuation project.

## Placement and measured comparison

C4 moves from `(111.8, −128)` to `(113.5, −117)` mm, retaining its Top side,
90° rotation, package and 100 nF value. R10 (100 kΩ), R11 (4.7 kΩ), U1, U2,
SW5 and all other footprints remain fixed. Existing shared ground vias remain;
a 0.3 mm drill / 0.6 mm diameter GND via is added at `(112.95, −117.2)`.
C4 reaches it through a 0.25 mm lead, and plane diagnosis places it in the
connected In1 GND fragment.

Measurements below are shortest explicit trace centerlines between pad centers.
They exclude via barrels, component internals and paths through ground pours.
The two accepted variants were prepared against the same input board on
separate project copies.

| Metric | Input | Via-junction alternative | Selected direct fanout | Rejected full-divider trial |
| --- | ---: | ---: | ---: | ---: |
| U1.8–C4.2, mm | 32.485 | 2.498 | 2.174 | 2.934 |
| Layer changes on that path | 4 | 0 | 0 | 1 |
| Via anchors on that path | 4 | 1 | 0 | 1 |
| C4 ground lead to nearest explicit via, mm | 2.230 | 0.617 | 0.617 | 1.359 |
| R10.2–U1.8, mm | 28.849 | 28.849 | 28.849 | 8.912 |
| R11.1–U1.8, mm | 27.643 | 27.643 | 27.643 | 3.827 |
| U2.2–R10.1, mm | 3.134 | 3.134 | 3.134 | 22.961 |
| DRC errors / warnings | 3 / 9 | 3 / 9 | 3 / 9 | 34 / 9 |
| Unrouted connections | 0 | 0 | 0 | 0 |

The selected C4 path is **93.3% shorter**, stays on Top at 0.18 mm width and
visits no via. Its direct fanout avoids the existing ADC via's reference-plane
clearance. The via-junction alternative uses that existing via as a coplanar
Top junction: its `via_count: 1` does not mean a barrel traversal. Both variants
preserve the long divider-to-MCU route and its four layer changes. Moving C4
alone does **not** shorten the high-impedance node or prove lower ADC noise.

The divider-to-capacitor path moves to the ADC end of that node: R10.2–C4.2
increases from 6.963 to 31.024 mm. This is the intended placement tradeoff,
not a reduction in every analog path. R10/R11 source impedance and the actual
ADC sampling/attenuation settings still need electrical validation.

| Saved board | Trace segments | Vias | Whole-board centerline, mm |
| --- | ---: | ---: | ---: |
| Input Kelvin board | 979 | 144 | 1523.689 |
| Via-junction alternative | 974 | 145 | 1519.709 |
| Selected direct fanout | 975 | 145 | 1520.239 |

The direct route uses 0.530 mm more total copper than the via-junction option,
but shortens the pin-to-capacitor path and has full In1 coverage beneath that
path. All original ADC vias remain for the global divider feed.

## Full-divider trial and tool limitation

The tested alternative relocates R10, R11 and C4 to Bottom near the MCU:
R10 `(110, −114)`, 90°; R11 `(111.6, −116.2)`, 0°; C4 `(113, −115.8)`, 90°.
It removes most of the long ADC route and extends the unattenuated switched
VBUS feed instead. Its 0.2 mm In2 input feed and Bottom signal/ground fanouts
conflict with existing CC, INT, GPIO0, ESP_EN and other copper. The checked
candidate closes connectivity but has 34 DRC errors, **31 more than the input**.
It was rejected and its routing patch was never applied. This is evidence
against this specific trial, not proof that every full-divider layout is worse.
A broader redesign may find a better placement and a suitable VBUS corridor.

`device_place` cannot flip a footprint while it has attached traces. On the
isolated trial copy, `trace_remove` first detaches only the explicitly listed
ADC/divider attachments; `device_place` then moves/flips the three devices.
The subsequent routing candidate is checked against that staged model. Its
report therefore says 28 new errors relative to six staged errors, rather than
31 relative to the original three. Final comparison uses the original input.
The unsaved trial setup is discarded after preview; the accepted board is
unaffected. Supporting a side change inside `routing_copper_prepare` would allow
this comparison in one atomic, original-baseline candidate.

[Rejected measurements](whole-rejected-measurements.json) use a read-only graph
reconstructed from the exact preview mutations and placed `board_get`, preserving
actual trace UUIDs. No native file is edited or failed validation bypassed.
DRC and coverage come from MCP's checked preview, not the reconstruction.

## SW5 open and closed

[Mode evidence and calculations](sw5-mode-review.json) record component/pad
UUIDs, pin nets and the ideal-divider assumptions. The circuit is unchanged:

```text
VBUS ── SW5 ──┬── U2.2 VBUS
              └── R10 100k ──┬── U1.8 ADC_VBUS
                             ├── R11 4.7k ── GND
                             └── C4 100nF ── GND
```

| SW5 state | Schematic behavior |
| --- | --- |
| Closed | Main VBUS directly reaches U2.2 and the top of R10. ADC receives nominally VBUS × 0.0448902. |
| Open | Main VBUS is disconnected from both U2.2 and R10. The ideal unloaded circuit settles toward ground through R11 and R10; ADC no longer measures main VBUS. Real leakage, bias and transients can change the voltage. |

| Main VBUS with SW5 closed, V | Nominal ADC voltage, V |
| --- | ---: |
| 5 | 0.224 |
| 20 | 0.898 |
| 28 | 1.257 |
| 48 | 2.155 |

**SW5 closed at 48 V exposes U2's VBUS pin to 48 V**, exceeding the FUSB302B
28 V absolute maximum cited in the [handoff](../../HANDOFF.md) and
[manufacturer datasheet](https://www.onsemi.com/pdf/datasheet/fusb302b-d.pdf).
The divider does not protect that pin. Absolute maximum is not a recommended
operating voltage. The input's `FUSB302MPX` circuit value and `FUSB302BMPX`
library identity are preserved; purchasing identity still needs confirmation.
Opening SW5 removes that direct DC connection but also removes VBUS sensing;
it does not by itself establish a safe EPR mode or correct PD/firmware behavior.

For the ideal closed-switch circuit, R10 || R11 is 4.489 kΩ, the 100 nF pole is
354.5 Hz and five time constants are 2.245 ms. These values ignore tolerances,
input loading and distributed parasitics. With SW5 open and no other injection,
C4's discharge time constant through R11 is 0.470 ms. At 48 V closed, R10 would
have about 45.85 V across it and dissipate 21.0 mW; exact resistor working-voltage,
switch ratings, ADC configured range, leakage and transient protection remain
unqualified. A nominal 2.155 V ADC result does not establish valid calibrated
ADC operation or 48 V board suitability.

## Checks and evidence

[Geometry comparison](geometry-comparison.json) verifies every retained trace
and via by UUID, ignoring only netsegment IDs after split/merge. Eight original
traces are replaced by four; no original via is removed. Only C4 moves. All
VBUS and switched-VBUS traces, R10/R11 geometry, CC/USB/RF, Kelvin, previous
OLED/U2/U5 changes and all other placements are preserved. Outline, holes,
keepouts, stack and plane settings remain fixed. Saved `circuit.lp` is
byte-identical to the input in both accepted variants.

[Reference comparison](reference-comparison.json) matches before, preview and
applied coverage. CC/USB/RF/Kelvin and switched-VBUS coverage is unchanged for
both adjacent reference layers, including inherited gaps. The selected
U1.8–C4.2 path has no In1 GND coverage gap. Across the entire ADC net, In1
uncovered length decreases from 5.123 to 4.182 mm; Bottom-to-In2 uncovered
length remains 5.296 mm. These checks do not qualify impedance, return
inductance or ADC noise.

The accepted variants use prepare, full preview and revision-guarded apply.
Final ERC, full DRC with rebuilt planes, unrouted, explicit save and history
are archived. Every final DRC message and location equals the input: two J2
copper–hole errors, AE1:2 in the antenna keepout, eight plated-slot warnings and
one outline-radius warning. Nothing is approved away. Selected history has one
undo step and no unsaved changes.

Upstream LibrePCB 2.1.1 reopens the selected native project with zero ERC and
the same twelve inherited DRC findings (check exit 1). Independent fabrication
export exits 0; all thirteen Gerber/Excellon files
[match Foilfern](fabrication-comparison.json) after excluding generator,
creation-date and dependent checksum metadata. The native delivery is
hash-verified and its extracted selected project is reopened before delivery.

- [Input](baseline-region.png), [via-junction alternative](local-region.png),
  [selected](selected-region.png) and [rejected full-divider preview](whole-rejected-region.png)
  show copper/pads/vias in board coordinates, with pours and package bodies omitted.
  Magenta is ADC, orange switched VBUS, red main VBUS and gray GND.
- [Top](board-top.png) and [bottom](board-bottom.png) are inspected selected board
  renders. Crowded silkscreen remains a separate task.
- [Selected arguments](selected-input.json), [via-junction arguments](local-input.json),
  [intent](intent.json), [measurement script](measure_adc.py),
  [input measurements](baseline-measurements.json) and
  [selected measurements](selected-measurements.json) reproduce the accepted comparison.
  The archive includes the rejected trial's staged arguments and full preview.

The preconfigured environment and existing optimized MCP binary worked without
changes. No environment or dependency blocker was encountered. No Rust source,
native format or validation rule changed.

Next: **Q1/D2/R20 gate-clamp placement and power-corridor review**, followed by
router baseline-reuse profiling. Mechanical/silkscreen review, inherited
violations and hardware qualification remain open.
