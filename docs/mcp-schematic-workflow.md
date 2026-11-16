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

## Compose the whole local circuit

Prefer one `design_preview` document containing `add_components`, `add_nets`,
`wire_groups` and `notes` over separately adding every part, connecting every
net and repairing every annotation. Use stable document keys throughout.

- Give each new symbol an initial `position` and `rotation`. Arrange signal
  flow left to right, supply above and ground below. Give separate functional
  blocks distinct areas and short titles. Keep local decoupling with its IC.
- Set each component's `connections` and `assign_only:true`, then list its
  local physical connections in `wire_groups`.
- Start with `wire_groups` containing `net` and `pins`. Omit `paths` to let
  Foilfern draw the orthogonal connections and branch junctions. Reserve explicit
  paths for geometry that requires a particular route. Explicit path segments
  must be orthogonal, nonzero and include every selected pin.
- Pin references use document keys. If a component is declared under key
  `filter_r`, use `filter_r.1`; its designator `R1` is a separate field.
  To edit an existing part, first bind a key under `components`, for example
  `"filter_r":{"designator":"R1"}`.
- Put name and value styling in `add_components.<key>.fields` during creation.
  `offset` is relative to the final symbol origin in sheet axes. `rotation`
  is an absolute text angle. For a vertical passive, horizontal fields to
  its right are usually readable; for a horizontal passive, put the name
  above and the value below. Keep a consistent text height.
- Use labels to cross between blocks. Draw local relationships with continuous
  wires. Avoid joining unrelated functional blocks with a long shared ground
  wire solely because their nets have the same name.

Preview checks the proposed circuit and annotations and returns its image.
Inspect that image before accepting the layout. `review_components` selects
new or edited document keys for focused annotation review. Field edits and
local placement can be included in the same document. Preserve manual work by
limiting the managed components and wires.

## Check, inspect and save

After `design_apply`, run `schematic_review`, `erc_run` and `render`. A clean
ERC does not establish readability or datasheet correctness. Confirm the
rendered circuit follows the requested flow, local components belong together,
values are legible and wires do not obscure annotations. Correct any findings
and save with `project_save`.

`schematic_tidy` replaces a whole page's placement and wiring. Use it for an
intentional whole-page relayout and inspect the result. Plan functional blocks explicitly.

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

Replace the first call's arguments with your actual document. Preview IDs and
undo history belong to that session. The `rc-lowpass` guide is an executable
example document once its library parts are available. Save local edits before
exiting so they persist when the process closes.
