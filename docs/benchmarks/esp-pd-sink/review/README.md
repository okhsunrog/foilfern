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
python3 measure.py BOARD_JSON COMPONENT_LIST_JSON NATIVE_PROJECT_DIR > report.json
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

Next small API step: a read-only `review_measure` accepting these explicit groups
and endpoints, returning resolved identities, measurement status/reason and the
source project/revision. Store resolved references alongside human labels; reject
unresolved/ambiguous pins. Add enforced allowed-area or reference-copper constraints
only after the measured board experiments establish the needed semantics.

The completed first batch, accepted candidate arguments, native project archive,
renders and performance evidence are described in [REPORT.md](REPORT.md).
