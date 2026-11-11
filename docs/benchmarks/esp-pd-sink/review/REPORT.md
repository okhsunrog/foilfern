# ESP-PD-Sink: first placement review batch

Subsequent work: [OLED RESET](reset/REPORT.md), [U2 decoupling](u2/REPORT.md),
[U6/R17 Kelvin routing](kelvin/REPORT.md), [ADC/SW5 review](adc/REPORT.md) and
[gate-clamp placement](gate/REPORT.md) are completed. The gate-clamp report
links the latest native board; guarded routing-baseline reuse profiling is next.

2026-11-11. Source: `968d90a` (the handoff commit; PCB implementation unchanged
from `c7df48a`). This batch completes the first OLED analog, U5 feedback and
100 nF input-capacitor experiments. It also establishes manual component groups,
reproducible measurements and an initial routing/validation timing baseline.
The remaining handoff work is listed below; this is not a completed board
qualification or a completed router optimization project.

## Saved result

The preferred checked variant combines three independently checked experiments.
The native project remains at `/workspace/esp-pd-sink/best/ESP-PD-Sink.lpp`.
[artifacts.tar.gz](artifacts.tar.gz) contains native copies of the OLED-only,
U5-feedback-only, U5-input-only, combined and fixed-placement builtin variants,
exact requests/responses, timing replays and both fabrication exports. Extract it
with `tar -xzf artifacts.tar.gz`; the combined project is
`artifacts/native/combined/ESP-PD-Sink.lpp`. Original input remains in the
handoff's `../inputs.tar.gz`.

- [Top render](board-top.png) and [bottom render](board-bottom.png).
- [Combined MCP arguments](candidates/combined.json), applied through
  `routing_copper_prepare`, complete `routing_preview`, then `routing_apply`.
- [Resolved baseline](baseline-measurements.json) and
  [combined measurements](best-measurements.json), including direct distances,
  widths, layers, via counts and trace UUIDs for each measured path.
- [Geometry comparison](geometry-comparison.json),
  [timings](timing-summary.json), and
  [fabrication comparison](fabrication-comparison.json).

Every applied variant has its own native directory. All trial requests were
checked against the same original board geometry. The combined request was
prepared again against a fresh original copy, reviewed and applied as one undo
step. The saved combined project reports no unsaved changes.

## Geometry and checks

Lengths are pad-center to pad-center trace centerlines, excluding via barrels
and paths through plane copper. Shared FB lengths are overlapping paths, not
independent lengths to add together.

| Connection | Before, mm | Combined, mm | Layers before → after | Via count before → after |
| --- | ---: | ---: | --- | --- |
| OLED1.27–C22.1, VCOMH | 9.374 | 3.185 | Bottom → Bottom | 0 → 0 |
| OLED1.28–C21.1, OLED VCC | 10.082 | 3.821 | Bottom/In2 → Bottom | 2 → 0 |
| U5.5–R14.1, FB | 4.613 | 4.176 | Top/In2 → Top | 2 → 0 |
| U5.5–R16.2, FB | 3.733 | 3.296 | Top/In2 → Top | 2 → 0 |
| U5.3–C16.2, VIN | 3.485 | 2.379 | Top → Top | 0 → 0 |

C21/C22 rotate from 90° to 270° at their original positions on Bottom.
C22's ground via moves from `(147.18, -109.053)` to `(146.6, -111.2)` mm.
Both OLED analog branches beat the handoff's KiCad comparison targets
(4.32/4.69 mm). The four charge-pump branches preserve their exact copper,
placement and 2.931/4.381/2.958/4.282 mm lengths. FPC, cutout and RESET are
unchanged. RESET still measures 7.890/12.125/15.064 mm with two vias per branch.

The divider stays at its original placement. Two PG branch segments are replaced
by a short In2 connection with two new through vias, making room for FB on Top.
The two FB vias are removed. This trades PG layer transitions for FB transitions;
the feedback-only variant's whole-board via count stays 145. R30/RT, BOOT, SW,
L3 and output-capacitor placement remain unchanged. The FB route lies below the
FB side of U5, away from the SW pin side; coupling and loop stability were not
simulated.

C16 moves from `(104.6, -146.3)` to `(105.25, -146.0)` mm and rotates from 90°
to 270° on Top. VIN is a direct 0.55 mm wide connection to U5.3. Its ground
trace is 0.4 mm wide and connects to a relocated 0.3/0.6 mm ground via at
`(105.9, -145.8)` mm. The former ground via at `(104.6, -147.95)` is removed.
The new via contacts the rebuilt GND copper; the full-board connectivity check
reports no open GND connections. The old U5.1–C16.1 return has no trace-only
measurement because it uses plane copper. Neither this check nor centerline
length is an inductance or hot-loop-area measurement. Input VBUS branches are
explicitly reconnected while retaining their 0.4/0.65/1.0 mm widths.

The combined board has 994 trace segments, 143 vias and 1517.093 mm total trace
centerline, versus 1009, 145 and 1529.788 mm originally. Only C16's coordinates
change; only C16/C21/C22 rotations change. Retained trace/via UUIDs preserve
position, width, layer and anchors; net-segment IDs may change on split/merge.
Every removed trace/via is in the explicit authorization lists. Outline, holes,
keepouts, copper layers and plane settings are unchanged.

Final Foilfern checks: **0 unrouted, 0 ERC findings, 3 inherited DRC errors,
9 inherited warnings, 0 new errors or warnings**. All final DRC messages,
including their locations, exactly equal the baseline. Netlist preservation is
confirmed by candidate and applied-model validation; saved `circuit.lp` is
byte-identical to the original.

Inherited errors: two J2 copper–hole clearances below 0.25 mm and AE1:2 in the
antenna copper keepout. Inherited warnings: eight 0.6 mm plated slots below the
0.7 mm setting and the board outline's inner radius below 1.0 mm. None were
approved away. Both renders were inspected; the imported silkscreen remains
crowded and still needs a separate cleanup pass.

## USB, RF and independent export checks

USB/RF traces, vias and footprints are unchanged. Rebuilt In1 GND reference
coverage is exactly equal before and after, including every uncovered interval:

| Signal | Checked length, mm | Covered centerline | Uncovered length, mm |
| --- | ---: | ---: | ---: |
| D+ | 25.562 | 91.113% | 2.272 |
| D− | 25.988 | 91.556% | 2.195 |
| ANT | 6.794 | 100% | 0 |
| Net-(AE1-FEED) | 6.719 | 100% | 0 |

There is one connected reference fragment. USB's existing gaps near layer
transitions are retained, not resolved. Centerline coverage does not qualify
USB impedance/return transitions or RF matching; C3/C31 remain DNP.

After closing the MCP project, upstream LibrePCB **2.1.1** reopened the combined
saved project and reproduced 0 ERC findings and exactly the same 12 DRC
findings. Its ERC/DRC command exits 1 because those inherited findings remain.
Foilfern exported 13 Gerber/Excellon files. Upstream independently exported the
same 13 files; all contents match after excluding generator-version,
creation-date and dependent MD5 metadata. The independent export exits 0 with
`LIBREPCB_SUPPRESS_DEPRECATION_WARNINGS=1`; this suppresses the CLI's documented
warning for its deprecated legacy fabrication-export option. No output jobs
exist in this input project. Board BOM export also completed with board
`default`, variant `Std`.

## Rejected alternatives

All rejected candidates were left unapplied. Exact inputs and reports are in
the evidence archive.

- Keeping C22's orientation and going right around its ground via collided
  with OLED1.26/R21.2. Four new errors; connectivity stayed closed.
- Rotating both capacitors and moving C22's ground via to `(146.6, -111.7)`
  collided with a Top GPIO7 trace. Moving the site to y = −111.2 cleared it.
- Moving the FB divider without explicitly replacing its attached supply trace
  was refused by protected-copper validation. The selected FB variant instead
  preserves placement and explicitly reroutes the crossing PG branch.
- Initial C16 requests omitted some attached copper and were refused. The first
  complete request placed its GND pad/trace too close to R13/EN (five errors).
  The selected site and plane-return via clear those objects.

These cases show useful API friction: placement changes need all attached
trace IDs, explicit return-path review and all-layer via-site inspection.
The current refusal identifies one protected object at a time; reporting the
complete affected attachment set would make planning faster. Data-only previews
also require geometry extraction or application on a copy to inspect an image.

## Routing and validation timing

The required default `cargo build --locked -p librepcb-mcp --bin librepcb-mcp`
succeeded in 2m57s. Default debug full DRC took 78.296 s. Subsequent experiments
used the same code with opt-level 2 for core, clipper, editor and autoroute:

```sh
cargo build --locked -p librepcb-mcp --bin librepcb-mcp \
  --config 'profile.dev.package.librepcb-core.opt-level=2' \
  --config 'profile.dev.package.clipper.opt-level=2' \
  --config 'profile.dev.package.librepcb-editor.opt-level=2' \
  --config 'profile.dev.package.librepcb-autoroute.opt-level=2'
```

The following final replays ran serially in fresh server processes on copies of
one original snapshot, with those identical flags and no other experiment or
build running. Earlier contended measurements are retained separately and are
not used in this table. One final sample per case; these are not distributions.
Memory is the maximum observed Linux process `VmHWM` over the whole trial,
including opening, inspection, prepare, preview, apply and save.

| Metric | Explicit OLED rotation/via candidate | Fixed-placement builtin VCOMH reroute |
| --- | ---: | ---: |
| Prepare, s | 29.908 | 29.180 |
| Preview, s | 0.004 | 0.009 |
| Apply, s | 13.424 | 12.487 |
| DRC core: before / after / apply, s | 14.831 / 14.082 / 13.003 | 12.971 / 14.824 / 11.954 |
| Separate planes_rebuild call, s | 0.430 | 0.395 |
| Separate unrouted call after fill, s | 0.015 | 0.019 |
| Whole-trial peak, KiB | 167948 | 178748 |

Builtin uses Bottom only, 0.1 mm grid, 0.2 mm width, pad escape, and the rectangle
x = 143–150, y = −113 to −105 mm. Only the original VCOMH trace IDs are ripped
up. It makes **1 search, 0 rip-ups, 0 congestion tiles**, produces 10 traces and
0 vias, and leaves 0 unrouted / 0 new DRC findings. Expanded-node counts are
not exposed. Its VCOMH path is 9.276 mm versus 9.374 mm before, with
whole-board trace length 1529.690 mm and 145 vias. The explicit OLED trial
produces 1516.950 mm and 143 vias. These use different allowed placement
changes, so their geometry is not an isolated router-engine comparison.
Preview/apply were measured separately and validated the applied model. Both benchmarks preserve netlist and the three inherited errors.

DRC core accounts for 28.913 of the OLED prepare's 29.908 s and 27.795 of the
builtin prepare's 29.180 s. The remainder includes snapshot creation, plane and
connectivity rebuilding, geometry edits/search, serialization and transport;
those subphases are not separately instrumented. The standalone fill/unrouted
calls are useful context, not a decomposition of the candidate's internal work.
No change to the router implementation or caching is claimed. The compiler
settings improve iteration speed; that is not an algorithmic routing speedup.

The measured next optimization target is the repeated same-revision baseline
DRC, followed by profiling individual DRC checks. Any reuse must include board,
project-session ownership, revision and save/path/text context. Keep the fresh
after-check and applied-model check, and test stale revisions, save-as/text
changes and plane-fed GND disconnections. The one-search replay does not yet
justify optimizing A* or congested rip-up behavior. More detailed phase counters,
baseline reuse with measured acceleration and a congested-region benchmark are
still open.

## Groups, environment and next work

[The prototype](README.md) resolves overlapping OLED/U5/Kelvin/power groups and
sourced requirements to component-instance, signal and footprint-pad UUIDs.
Baseline lengths agree with the archived independent measurements to 0.001 mm;
via counts agree. An R17 → RSENSE rename exercise preserves resolved identities
in both overlapping groups. Plane-only returns explicitly report `not_checked`
with a reason. No membership is inferred from common VCC/GND nets. The proposed
next API step is a revision-bound read-only measurement endpoint, not a new
LibrePCB file-format extension.

The preconfigured environment worked: the upstream checkout/font, Rust/Cargo,
LibrePCB CLI, required native dependencies, build, network access and Git push
all succeeded. No environment edits were needed. Non-blocking notes: the default
debug build is slow for DRC, and Cargo warns of future incompatibility in the
external `proc-macro-error2` dependency. Missing project-specific user settings
in the portable archive use defaults; they are not missing design data.

Continue from the combined native project, preserving the original baseline for
comparisons:

1. Evaluate OLED RESET after the now-short analog branches.
2. Review U2 local decoupling, paired Kelvin routing U6/R17, and both ADC C4
   placement alternatives with SW5 schematic-mode checks. Those geometries are
   unchanged in this batch.
3. Extend U5 input-loop review to copper-return/loop-area analysis and hardware
   measurements; do not infer performance from the reduced VIN centerline.
4. Implement and benchmark guarded baseline reuse and detailed phase/search
   counters, then use a genuinely congested builtin case.
5. Complete silkscreen/mechanical review and inherited-violation resolution after
   checking the relevant mechanical/antenna requirements.

Electrical qualification remains open: switching-loop parasitics, stability,
EMI, current/thermal margin, Kelvin accuracy, ADC noise, USB impedance and RF
matching need modelling or measurements. Exact J1/EPR suitability and SW5's
48 V path were not qualified; FUSB302B's VBUS absolute limit remains 28 V.
This work does not establish 48 V / 5 A hardware suitability.
