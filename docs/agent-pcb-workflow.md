# Agent PCB workflow in Foilfern

Foilfern should let an agent design a board and explain why the specific saved
result meets its requirements. When a requirement fails, the agent should be
able to point to the net, objects, layer, location and rule involved, propose a
repair, and show what the repair changes. Passing ERC or DRC is evidence for
the rules those checks cover; it does not establish impedance, thermal margin,
EMI performance, enclosure fit or hardware behavior.

## From project to reviewed board

1. **Establish a baseline.** Import and verify netlist, footprint and pad
   mapping, board outline, stack, keepouts, planes and rules. Record inherited
   failures with their locations. Preserve the source and pin the revision
   used for comparisons.
2. **Capture constraints before moving copper.** Separate fixed constraints
   (connectors, mounting and enclosure, display cable, antenna, fab rules)
   from preferences (area, trace length, via count). Include the source of
   each constraint: user, manufacturer, part datasheet, or engineering
   assumption. Treat an unverified assumption as such in the final evidence.
3. **Try placement and mechanical alternatives on copies or snapshots.**
   Keep the schematic and required interfaces fixed by default. For each
   proposed footprint move or board resize, compare room to escape pads,
   short power loops, space for reference copper, enclosure access and
   the scope of reroute. Show the geometry, tradeoff and affected nets
   before applying a change. Explore several layouts when there is a real
   choice; a board shape imposed by a mechanical interface is a constraint.
4. **Route in functional regions.** Start with power and ground, then
   sensitive nets and finally ordinary signals. Route a chosen net or a
   rectangular region with an explicit list of copper that may be ripped up.
   Use the built-in search, explicit waypoint paths or both. A partial result
   may be useful if it improves its target without breaking protected nets.
   Rebuild planes and check return paths after adding obstacles and vias.
5. **Compare and review.** Capture exact changes and validation on one
   original revision. Check netlist, connectivity per net, new DRC errors
   and warnings, and applicable engineering requirements before comparing
   softer metrics. Offer a diagram or image with highlighted changes,
   blocked endpoints, severed plane contacts and proposed stitch sites.
   Applying a candidate should make one undo step; cancellation or refusal
   leaves the original intact.
6. **Finish the board.** Check silkscreen legibility and mask clearance,
   connector orientation/pin 1, mounting holes, cutouts and fabrication
   outputs. Reopen the native project and rerun ERC/DRC with an independent
   tool when possible. Record measured, modelled and unverified properties
   separately, with the files and checks needed to reproduce them.

## Existing MCP controls and gaps

- `routing_prepare` can select nets and layers. `routing_region_prepare`
  bounds search to a board rectangle and limits rip-up to authorized UUIDs.
  `routing_copper_prepare` can combine explicit paths, new vias and selected
  trace replacements. The same preview/apply guard checks the whole board,
  including nets outside the selected scope. Agents can already diagnose a
  local problem, try alternate routes and leave unrelated copper intact.
- `device_place` moves, rotates and flips a footprint; `board_add` can
  copy a board. These make trials possible, but placement, mechanical edits,
  local reroute and checks are not yet one reviewed candidate. Add grouped
  device moves with locks and affected-copper previews before treating
  automatic relocation on a routed board as a routine fix.
- `board_set_outline` replaces the outline with a rectangle (optional corner
  radius) or a straight-edged polygon. A concave polygon can express an
  edge notch for the display cable. `board_polygon_add` can add an internal
  polygon on `brd_cutouts`. Native arcs in an imported outline are preserved,
  but the typed MCP setter cannot edit a contour with mixed lines and arcs,
  nor replace one loop while preserving its identity.
- The native board model supports holes and slots. The raw mutation endpoint
  can edit board items today, but MCP lacks a typed hole/slot add, update and
  remove workflow in millimeters. `board_legend_list` and `board_text_move`
  support inspecting and repositioning existing silkscreen labels without
  moving copper. Prioritize round and slotted NPTH, closed cutouts with radii,
  stable IDs, mechanical clearance checks and per-side legend preview.
  Keep plated holes distinct from NPTH and net-connected vias.

## Dogfooding sequence

**ESP-PD-Sink.** Finish the imported four-layer board with the existing
connectors and OLED cable notch. The prior KiCad layout is a comparison
point. The first trials showed that a route with no new clearance errors
can still cut a plane-fed power or ground connection. Surface the rebuilt
air-wire endpoints, support repair of the exact rejected route, and make
new warnings actionable. Review the switcher loop, Kelvin sense, USB pair
and reference path, antenna keepout, remaining footprint clearances and
silkscreen as distinct checks. Compare physical routing length, via count,
reference geometry and agent effort only with the remaining gaps disclosed.

**Cheap FOCer 2.** After ESP-PD-Sink is compact, reviewed, and includes the
mechanical and silkscreen pass, use the published [Cheap FOCer 2 design](
https://github.com/shamansystems/Cheap-FOCer-2) as the next benchmark.
Reproduce the published two-layer stack, electrical design, mechanical
interfaces, cooling constraints and fabrication assumptions for the first
comparison. Verify the import and use the author's layout as a baseline.
Plan the three half-bridges, DC-link commutation loops, gate and return
paths, bootstrap placement, shunt Kelvin routes, power and logic returns,
current-carrying copper and thermal paths before routing control signals.
Review connectors, heat-sink/mounting clearances and silkscreen. Compare
layout and checks before discussing a smaller variant; establish electrical
and thermal equivalence with appropriate analysis and hardware measurements
before claiming a design is better for the same operating conditions.

## Ideas to evaluate on real boards

- Build a requirement-to-evidence report: status, units, rule source, net
  and object IDs, board coordinates, measurement, threshold, and the artifact
  revision. Include an explicit `not_checked` state.
- Offer candidate families rather than arbitrary moves: fixed placement,
  local component rotation, movement inside a permitted region, or an
  alternate outline with fixed mechanical interfaces. Show benefits and
  what must be rerouted before proposing a change.
- Use plane split endpoints to suggest one or two stitch candidates against
  the already routed copper. Rank by restored contacts and return coverage;
  keep unnecessary fragments and vias out. Cache the geometry from the
  failed trial instead of rerunning search just to repair a pour.
- Record decisions alongside replays: why a route uses a layer or width, why
  an alternative was rejected, and what a reviewer still needs to check.
  Promote recurring constraints into first-class tools only after real
  projects demonstrate what the representation needs to express.
