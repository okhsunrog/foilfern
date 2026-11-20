# Draw and finish a schematic

Use this workflow for new schematics and new functional blocks. A finished
schematic has checked connectivity, visible local circuits, readable annotations
and a saved native project. Initial free-space placement is a draft.

## Discover once

Read the complete tool catalog before starting. With the local binary, run
`foilfern-mcp tools` for all descriptions, schemas and server instructions, or
`foilfern-mcp tools design_preview` for one schema. Run
`foilfern-mcp guide rc-lowpass` for a complete small design document, and
`foilfern-mcp guide board` for the wider project workflow.

The CLI also accepts the resource names `guide schematic-design` and
`guide board-design`. For library details, use
`call library_element '{"uuid":"<library UUID>","kind":"component"}'`.
The component's variants identify its gate symbols. Query those symbols with
`kind:"symbol"` to inspect pin positions before choosing rotations.

Select exact library identities and symbol variants before drawing. Check the
actual pin mapping. A physical part needs its device UUID in `add_components`.
The selected library must contain the required symbol variant and dependencies.
A symbol-variant UUID identifies an entry inside its component. To read pin
geometry with `library_element`, use that variant gate's symbol UUID.

## Compose the whole local circuit

Prefer one `design_preview` document containing `add_components`, `add_nets`,
`wire_groups` and `notes` over separately adding every part, connecting every
net and repairing every annotation. Use stable document keys throughout.

- Use `block` to pack an ordered row or column of new components into an area.
  Omit their initial `position` and set their `rotation` and `fields` during
  creation. Give fixed anchors an explicit position. Arrange signal
  flow left to right, supply above and ground below. Give separate functional
  blocks distinct areas and short titles. Keep local decoupling with its IC; `anchors` place it at the pins.
- Set each component's `connections`. Use `assign_only:true` when every
  connection will be drawn through `wire_groups`. For a component with labeled
  single-pin signal ends, leave `assign_only:false`; local groups replace their
  member stubs while the other stubs keep their labels.
- Start with `wire_groups` containing `net` and `pins`. Omit `paths` to let
  Foilfern draw the orthogonal connections and branch junctions. Reserve explicit
  paths for geometry that requires a particular route. Explicit path segments
  must be orthogonal, nonzero and include every selected pin.
  Automatic groups need at least two pins and receive net labels automatically.
  The `label` position field requires explicit `paths`.
- Pin references use document keys. If a component is declared under key
  `filter_r`, use `filter_r.1`; its designator `R1` is a separate field.
  To edit an existing part, first bind a key under `components`, for example
  `"filter_r":{"designator":"R1"}`.
- Put name and value styling in `add_components.<key>.fields` during creation.
  `offset` is relative to the final symbol origin in sheet axes. `rotation`
  is an absolute text angle. For a vertical passive, horizontal fields to
  its right are usually readable; for a horizontal passive, put the name
  above and the value below. Keep a consistent text height.
  These are initial styles. For a field already present in the project, use
  `symbol_text_move` with `text:"name"` or `text:"value"`, or `place_text` in a
  preview to find a clear position.
- Use labels to cross between blocks. Draw local relationships with continuous
  wires. Avoid joining unrelated functional blocks with a long shared ground
  wire solely because their nets have the same name.

Preview checks the proposed circuit and annotations and returns its image.
Inspect that image before accepting the layout. Annotation review automatically
covers bound and new components. A preview with overlaps returns `partial`
and includes their locations and changes from the live sheet. Field edits and
local placement can be included in the same document. Preserve manual work by
limiting the managed components and wires.

## Place functional groups

Create one local group per `design_preview` document. `block.members` accepts
keys from `add_components` and bound existing `components`. Set `min`, `max`,
`direction`, `gap` and optionally `baseline` to define its area and alignment.
Foilfern measures symbol fields and existing independent wire stubs when packing
members. `gap` is clearance between occupied bounds; center spacing also
includes the symbols, fields and stubs. Keep the anchor IC and unrelated parts outside `members` when arranging
a nearby capacitor bank or pull-up row.

For imported rectangular passives, `symbol_styles` provides `capacitor`,
`normally_open_push_button` and `crystal` artwork. The crystal style takes
`resonator_pins` and `case_pins` as library symbol pin UUIDs. It adjusts the
embedded shared artwork while preserving pin anchors and circuit memberships.

Include local `wire_groups` with `pins` and omit `paths`. The block is placed
first, then these connections are drawn from the final pin positions. Existing
wires can be replaced when every pin on their segments belongs to the declared
wire group. Replaying the same document preserves the resulting layout.

## Place support parts at IC pins

For a part that belongs to one IC pin, such as a decoupling capacitor at a
supply or DEC pin, a load capacitor at an oscillator pin or a series resistor,
use `anchors` instead of coordinates. Give the IC an explicit position (or bind
it under `components`), add the part without `position` or `rotation`, and add
`{"component":"c13","pin":"1","target":"u2.DEC1"}`. Foilfern rotates the part
so that its `pin` faces the target pin, puts it on the target pin's axis and
draws one straight wire with the net label between them. The far pins get
labeled stubs unless `assign_only` is set. The distance starts at the room
needed for the net label (at least 5.08 mm, or `distance`) and grows in grid
steps while the part, its fields or the wire would touch other items; a part
that still has no clear position fails the preview. A single labeled stub
already at the target pin is replaced. List a pin in `wire_groups` instead
when it needs a branch to several parts, and set `wire:false` to keep the
connection for your own group. Anchors apply only when the part is created.

Use separate bounded groups for the MCU support circuitry, power stages, clock
circuits and interfaces. Inspect each group before adding the next one. The
`rc-lowpass` example uses a new-component block with automatic branch wiring.

## Check, inspect and save

After `design_apply`, run `schematic_review`, `erc_run` and `render`. A clean
ERC does not establish readability or datasheet correctness. Confirm the
rendered circuit follows the requested flow, local components belong together,
values are legible and wires do not obscure annotations. Correct any findings
and save with `project_save`.

`schematic_review` also checks fields, labels and notes against drawn symbol
artwork such as capacitor plates and outlines, not only against other text.
Its `metrics` cover the whole page. `defects` lists overlapping symbol bodies,
wires of different nets which run along each other or end on each other, and
wires drawn through a symbol; a reader takes these for connections, so fix them
before saving. Crossings, bends, wire length, `split_nets` (nets joined only by labels),
`pin_span_mm` (spread of connected pins), `symbol_fill` and `off_grid` are
measurements, not errors. Compare them before and after a layout change of the
same circuit: lower length, crossings and pin span usually read better.

`schematic_review` returns a repair action when annotations overlap. Use
`schematic_tidy` for whole-page repair. It normalizes ordinary name/value fields,
rebuilds placement and wiring, and checks the rendered annotation bounds. It
preserves the complete circuit, including unused nets and pin assignments.
`shared_artwork_symbols` lists embedded symbols whose pin-name positions were
adjusted. All instances using those symbols display the adjusted artwork.
An unresolved conflict rolls the operation back. Inspect the resulting render
and use `design_preview` for focused placement or field edits. Plan functional
blocks explicitly.

## Local CLI sessions

Each `call` or `batch` starts a fresh session. Keep `design_preview` and
`design_apply` in one batch, using the preview result reference:

```json
[
  {"tool":"design_preview","arguments":{"version":1,"schematic":"Main"}},
  {"tool":"design_apply","arguments":{"preview":{"$ref":"0#/structuredContent/result/preview"}}},
  {"tool":"project_save"}
]
```

Write the array to `calls.json` and run
`foilfern-mcp --project project.lpp batch @calls.json > results.jsonl`.
Both `batch calls.json` and `batch @calls.json` read a file. `batch -` reads
standard input. Each output line is one JSON record containing `index`, `tool`
and `response`. Tool data is under `response.structuredContent.result`, and
PNG attachments are under `response.content` with `type:"image"` and base64
`data`. Check `response.isError` before using a result. Decode images to files
and open them for visual review.

`call` returns one record with the same `index`, `tool` and `response` envelope.
Revision history starts again with each CLI process; preview IDs and revision
checks belong to that process.
Read one tool schema with `foilfern-mcp tools design_preview`. Its catalog
envelope contains the starting instructions and the selected tool.
For independent calls, `batch calls.json --continue-on-error` runs later calls
after a failure and retains a failing exit status.

Replace the first call's arguments with your actual document. Preview IDs and
undo history belong to that session. The `rc-lowpass` guide is an executable
example document once its library parts are available. Save local edits before
exiting so they persist when the process closes.

Mark parts that stay unassembled with `do_not_populate` on `add_components`,
bound `components` or `component_update` instead of raw mutations.
