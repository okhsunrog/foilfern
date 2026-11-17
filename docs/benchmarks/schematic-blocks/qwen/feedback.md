**Feedback (observed unless marked hypothesis)**

**Documentation/schema**
- The guide's claim "for a component with labeled single-pin signal ends, leave `assign_only:false`" worked, but produced *vertical* stub labels with text touching the wire end. Nothing documents stub-label orientation or how to make it horizontal; I had to discover `RelativeCoordinate` in the raw schema. Document label orientation rules and the pin-relative path idiom prominently — it's the key to replay-safe explicit paths after block packing.
- `library_element` returns `symbol_variants`, not `variants`; my first extraction printed nothing. Consistent naming or a guide snippet would help.
- Guide says blocks measure "existing independent wire stubs" when packing, but not whether `assign_only` changes packing width. Observed: resistor spacing changed between stub and no-stub runs (20.32 mm vs tighter). Hypothesis: stubs count toward occupied bounds. This makes absolute-coordinate paths fragile after rebuilds.

**Workflow friction**
- No cross-process undo: fixing bank 2's labels required `remove_components` + recreate rather than undo. A persistent undo history or a "replace group" idempotent re-apply would avoid delete/recreate churn.
- Single-pin nets can't use automatic `wire_groups` (≥2 pins), forcing explicit paths for every labeled end — a `stub` direction/length option per connection would remove most manual path work.
- Preview PNGs are auto-cropped with no coordinate axes/scale; judging "outside its geometry" and separation required `schematic_get` numbers. A scale bar or mm grid overlay would improve visual review.

**Unnecessary calls**
- My `variants` key probe and one failed `cd` were wasted turns; the second full `library_element` batch could have been merged with the first had gate-symbol UUIDs been included inline.

**Beyond MCP**
- ERC OpenNet warnings for intentional labeled ends are noise; allow declared intentional single-pin nets (like `no_connects`).
- Netlist tool output matched disk parsing exactly — good. Replay-empty-diff semantics worked flawlessly and is the strongest feature.