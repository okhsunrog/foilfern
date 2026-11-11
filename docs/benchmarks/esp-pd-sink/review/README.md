# ESP-PD-Sink review prototype

Manual, overlapping groups and sourced requirements for the archived project.
`intent.json` is the human input: designators and pad/signal names. The measurement
script resolves it to component-instance UUIDs, library signal UUIDs and footprint
pad UUIDs. Persist those identities for subsequent edits; designator/address fields
are display labels. Renaming a component must update labels by UUID, not re-resolve
an old designator. No LibrePCB native file format changes are needed.

Generate a report from the exact MCP `board_get` and `component_list` results
(or their outcome envelopes):

```sh
uv run --no-project measure.py BOARD_JSON COMPONENT_LIST_JSON NATIVE_PROJECT_DIR > report.json
```

The final optional argument selects a different intent JSON. Reports include
centerline length, direct pad-center distance, used layers, widths, unique via
count and the trace UUIDs of the shortest trace-only path. Via barrel lengths
and propagation through plane copper are excluded. An unavailable measurement
has an explicit reason. Shared supply nets do not imply membership of a group.

The OLED length limits are measured KiCad comparison targets from the handoff,
not manufacturer electrical ratings. Required constraints and preferences are
separate. A `fixed_placement` measurement records the placement; compare its
baseline and final values to establish preservation. Reference copper requires
separate rebuilt-plane evidence. This prototype does not evaluate every proposed
constraint type or enforce these requirements inside the editor.

Baseline validation: seven independently recorded OLED/FB/Kelvin/ADC path lengths
match the archived evidence within 0.001 mm, including via counts; every group pin
resolves to an existing component, signal and footprint pad. U5.1–C16.1 correctly
reports no trace-only measurement rather than assuming an absent ground return.

The product now provides read-only `trace_path_measure` for explicit pad pairs,
including resolved identities, board/project/revision, length, layers, via and
trace IDs, and an explicit unmeasured result for absent trace-only paths.
See [the MCP contract](../../../mcp-design.md#read-only-trace-path-measurements).
The group/requirements document remains an external review prototype; bulk group
storage and enforced allowed-area/reference constraints are planned in
[roadmap P2](../../../roadmap.md#p2--persisted-requirements-and-evidence).

The completed first batch, accepted candidate arguments, native project archive,
renders and performance evidence are described in [REPORT.md](REPORT.md).

The subsequent batch completed [OLED RESET placement and routing](reset/REPORT.md).
It is an intermediate snapshot; continue layout work from the selected gate-clamp
project linked below.

[U2 VDD/VCONN decoupling](u2/REPORT.md), [U6/R17 Kelvin routing](kelvin/REPORT.md)
and [ADC C4 alternatives/SW5 review](adc/REPORT.md) are completed.
[Q1/D2/R20 gate-clamp placement](gate/REPORT.md) now links the current selected
native project. [Guarded routing-baseline reuse](performance/REPORT.md) is now
implemented in Rust. [Release validation on the original fixture](performance/release/REPORT.md)
confirms faster repeated preparation with equivalent DRC, connectivity and geometry.
Bounded builtin profiling is recorded beside it; the read-only path API is
implemented in core/MCP, with [all 18 saved pairs replayed in release](path-api/REPORT.md).
Connected-footprint side changes remain later work.
