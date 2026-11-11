# Q1 gate-clamp and power-corridor review

Subsequent [Foilfern baseline-reuse work](../performance/REPORT.md) implements
the repeated source-check optimization. The native board below remains the
current layout continuation point.

2026-11-11. Continues the [ADC C4 board](../adc/REPORT.md) at `69209d2`.
That input is the same ESP-PD-Sink PCB, saved after moving C4 beside the MCU.

The selected variant brings D2 and R20 beside Q1, reducing the gate-to-clamp
path from **13.746 to 3.738 mm** and the gate-to-pull-up path from **15.823 to
5.089 mm**. It has **0 unrouted connections, 0 ERC findings and 0 new DRC
findings**. Three inherited errors and nine warnings remain.

Latest native project:
`/workspace/esp-pd-sink/gate-review/selected/ESP-PD-Sink.lpp`.
[artifacts.tar.gz](artifacts.tar.gz) includes the input, a D2-only alternative,
the selected project, MCP records and independent fabrication exports. Continue
after extraction from `artifacts/native/selected/ESP-PD-Sink.lpp`.

## Circuit and placement

Component pin mapping confirms that Q1.4 is gate, Q1.1–3 are source on VOUT,
and Q1.5's pads are drain on VMOS. D2 is `BZX84C15`: its anode is on the gate
net and cathode on VOUT/source. R20 is the 10 kΩ gate-to-source pull-up.
R27 is the existing 0 Ω link to Q2's collector. Values, pin assignments,
packages, assembly options and the complete circuit are preserved. This is a
source-referenced gate clamp, not a gate-to-GND clamp.

| Component | Input position / rotation | Selected position / rotation |
| --- | --- | --- |
| D2 | (133.4, −133.2), 0° | (130.7, −142), 180° |
| R20 | (130.6, −133.4), 90° | (131.2, −144.3), 0° |

Both stay on Top. Q1, Q2, R27, J3, the mounting geometry and all other devices
remain fixed. D2's unused pad remains unconnected. The new parts sit below/left
of Q1, outside the existing VOUT and VMOS pour regions. C33/C34 stay fixed.

The new gate/clamp and pull-up gate leads use 0.2 mm copper; source leads use
0.4 mm copper. No via is added, moved or removed. The six existing R27-to-Q1
segments and the complete R27-to-Q2 route are preserved. Moving R20 also removes
the old remote clamp/pull-up branch rather than leaving an unused gate stub.

R20's original source pad was a junction in the U6 VOUT-sense feed. Its two
retained branches are explicitly reconnected through the old pad-center
coordinate with the same 0.4 mm trace geometry. The former source branch to
D2 is removed. Moving the resistor does not disconnect the VOUT-sense feed.

## Measurements and tradeoffs

Lengths are shortest explicit trace centerlines between pad centers, excluding
barrels, component internals and paths through copper pours. Both accepted
alternatives were prepared against the same input on separate project copies.
A third, more spread-out D2/R20 placement also passed prepare but was superseded
by the compact pair before application; its exact arguments and result are archived.

| Explicit path, mm | Input | D2 only | Selected D2/R20 |
| --- | ---: | ---: | ---: |
| Q1.4–D2.1, gate to clamp | 13.746 | 3.738 | 3.738 |
| Q1.4–R20.2, gate to pull-up | 15.823 | 15.823 | 5.089 |
| D2.1–R20.2 | 2.077 | 19.560 | 1.352 |
| Q1.4–R27.1, driver link | 8.558 | 8.558 | 8.558 |
| R27.2–Q2.3 | 4.214 | 4.214 | 4.214 |
| R20.1–D2.3, source-side link | 3.986 | Not trace-only | 2.480 |
| D2.3–Q1.3, source return | Not trace-only | 5.933 | 5.933 |

All measured paths stay on Top with zero vias and zero trace-layer changes.
The selected gate lead is **72.8% shorter**, and the pull-up lead **67.8%
shorter**. The D2-only variant leaves the pull-up remote and increases its
separation from D2, so the compact pair is selected.

The baseline source return is connected through VOUT copper, but there is no
trace-only path to Q1.3. It is not assigned a fictitious length or treated as
an open connection. The new 5.933 mm explicit return includes copper overlapping
the existing source pour. Plane diagnosis puts its entry at
`(129.7625, −139.635)` and Q1.3 in the same connected VOUT fragment. Current
can also spread through that pour; the trace length is not the sole return path.
No before/after clamp-loop inductance or source-loop area is claimed.

| Board | Trace segments | Vias | Whole-board centerline, mm |
| --- | ---: | ---: | ---: |
| Input ADC board | 975 | 145 | 1520.239 |
| D2 only | 972 | 145 | 1525.923 |
| Selected pair | 964 | 145 | 1522.490 |

The selection adds **2.251 mm of total explicit copper** relative to the input,
primarily because it adds a direct source return previously supplied through
the plane. Shorter critical gate paths are the benefit; total centerline is not
presented as an improvement. Zener dynamic impedance, clamp current, gate
overshoot, switching loss, turn-off timing and transient/thermal margins still
require modelling or measurements. A nominal 15 V part designation alone does
not establish transient gate protection.

## Power corridor, reference and checks

[Geometry comparison](geometry-comparison.json) checks every retained trace and
via by UUID, ignoring only changed netsegment IDs after split/merge. Nineteen
original traces are replaced by eight. Only D2/R20 move. All original vias,
Q1/J3 geometry, VBUS/VMOS routes, outline, holes, keepouts, stack and plane
settings are preserved. Saved `circuit.lp` is byte-identical to the input in
both accepted variants.

[Power-region comparison](power-region-comparison.json) compares actual emitted
Gerber regions, independent of aperture IDs, starting vertices, winding and
record ordering. Top VBUS, Top/Bottom VOUT and Top VMOS regions are geometrically
identical to the input. [Plane diagnosis](plane-comparison.json) independently
matches their fragment counts, areas and bounds. This preserves the existing
power corridor; it does not qualify its current capacity.

[Lower-layer/drill comparison](baseline-fabrication-comparison.json) confirms
unchanged In1, In2 and Bottom copper primitives, all drill hits/slots, and the
outline. Some raw files differ because netsegment merging reorders aperture
numbers and emitted via/drill records. [compare_copper.py](compare_copper.py)
resolves apertures and compares canonical primitives for the linear,
positive-polarity Gerber and metric Excellon subset emitted here; it rejects
unsupported commands. This is a bounded evidence helper, not a general parser.
Top GND fill changes locally; its seventeen connected fragments remain, while
discarded-island geometry changes. The reference planes on In1/In2 are preserved.

[Reference comparison](reference-comparison.json) matches preview to applied
coverage for both adjacent reference layers. CC/USB/RF, Kelvin and ADC coverage
is unchanged, including inherited gaps. Both input and selected gate-net traces
are fully covered by In1 GND. The earlier C4 fanout and other accepted OLED,
U2, U5 and Kelvin improvements remain unchanged.

Each accepted write used prepare, full preview and revision-guarded apply.
Final ERC, full DRC with rebuilt planes, unrouted, explicit save and history
are archived. Every final DRC message and location equals the input: two J2
copper–hole errors, AE1:2 in the antenna keepout, eight plated-slot warnings and
one outline-radius warning. Nothing was approved away. History has one undo
step and no unsaved changes for each accepted variant.

Upstream LibrePCB 2.1.1 reopens both accepted native projects with zero ERC and
the same twelve inherited DRC findings (check exit 1). Independent selected
fabrication export exits 0. All thirteen Gerber/Excellon files
[match Foilfern](fabrication-comparison.json) after excluding generator,
creation-date and dependent checksum metadata. The archive is hash-verified and
its extracted selected project is reopened before delivery.

- [Input](baseline-region.png), [D2 only](diode-region.png) and
  [selected region](selected-region.png) show Top, Bottom and In2 geometry.
  Magenta is gate, orange VOUT/source, red VMOS/drain and gray GND. Pours and
  package bodies are omitted; these plots are not fabrication previews.
- [Top](board-top.png) and [bottom](board-bottom.png) are inspected selected
  renders. Crowded silkscreen remains a separate task.
- [Selected arguments](selected-input.json), [D2-only arguments](diode-input.json),
  [intent](intent.json), [measurement script](measure_gate.py),
  [input](baseline-measurements.json), [D2-only](diode-measurements.json) and
  [selected measurements](selected-measurements.json) reproduce the comparison.
  The manual group resolves gate/source pads to stable instance/signal/pad UUIDs.

No preconfigured-environment or dependency problem was encountered. No Rust
source, native format or validation rule changed. The prior SW5/48 V finding
remains open; this layout step does not establish 48 V / 5 A suitability.

Next: **profile guarded routing-baseline reuse**, preserving revision guards,
rebuilt-plane validation and connectivity checks. Mechanical/silkscreen review,
inherited violations and electrical qualification remain open.
