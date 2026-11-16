# Schematic workflow acceptance

The first priority after 0.1.0 is reproducible, readable schematic composition.
An agent should discover and follow the supported workflow from the binary's
contract and its starting instructions. Merely creating components, preserving a
netlist or returning zero ERC findings does not establish schematic quality.

## Initial scenarios

Start with small, fixed electrical tasks before a whole-board design:

1. A two-resistor divider with input, output and ground test points.
2. A series resistor and shunt capacitor, with left-to-right signal flow.
3. Two independent RC channels sharing ground, drawn as distinct named blocks.

Use fixed component identities and exact expected netlists so library discovery,
part sourcing and electrical-design choices do not confound layout evaluation.
Keep the user's request free of placement coordinates and tool-call recipes.
The agent receives the entire tool list and descriptions and must review them
before drawing. Provide schema access and a current release binary. Foilfern
source files and earlier agent conversations are inaccessible.

## Measurements and review

Control the agent through ACP using a fixed DeepSeek model identifier. Record
the Claude, adapter and protocol SDK versions, binary commit/profile/hash,
starting project, full prompt, actual tool calls, response errors, task wall
time and token usage. Measure the final feedback turn separately. Request
feedback about the complete workflow, including limitations outside MCP.
Disable automatic title generation and classifier model calls in trial sessions
so hidden secondary models do not contaminate the measurements.

An independent process reopens the saved project and checks:

- exact components, values and pin-to-net memberships;
- ERC results and the reported limits of their coverage;
- visible wiring within each functional block;
- annotation conflicts, alignment, spacing and logical signal flow;
- title and block labels, plus a readable rendered result.

A zero-overlap report alone is insufficient. Review the image as well as the
native geometry. Repeat every accepted case in at least two fresh agent sessions
without human hints or edits. Record failed attempts and harness failures
separately; do not discard them from the evidence.

## Improvement order

1. Establish the binary-only baseline and confirm that tools are executable.
2. Trace failures to discovery, contract, composition or rendering behavior.
3. Improve the ordinary tool workflow, including initial placement and actionable
   review. Test a short skill for task sequences without copying parameter
   schemas into it. Keep the complete MCP catalog.
4. Repeat the same cases and inspect quality, time and tokens after each change.
5. Add larger schematics and new topologies after the small cases are reliable.

Do not weaken electrical or routing validation to improve completion counts.
Preserve native LibrePCB file interoperability and existing manual work.
