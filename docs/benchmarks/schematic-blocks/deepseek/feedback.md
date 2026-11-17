**Observed**
- Bounded workflow worked as documented: preview+apply in one batch, block packing, automatic VCC/GND wire groups, empty replay diffs, and saved netlist all verified.
- `design_preview` returned two identical PNG attachments per preview (same filename), wasting review time; `render` returned one.
- Preview images were mostly blank margin despite `image_region`; the dedicated `render` crop was tighter. Visual inspection needed external close-up renders.
- With vertical resistors, `assign_only:false` plus a VCC-only wire group produced correct VCC rails but left SDA/SCL/IRQ labels rotated 90°. Name/value fields stayed horizontal as requested. Hypotheses: label rotation follows the stub direction, and horizontal labels would require explicit L-shaped paths—untested.
- `block.baseline` 78 produced origins at 78.74 (grid snapping). The guide mentions “grid phase” but not the resulting offset.
- ERC reports three “less than two pins” warnings for intentionally labeled single-pin signal nets. No option marks these ends as intentional.
- Preview overlap reporting and `schematic_review` were consistent and clean.
- Reloaded `server_info` showed revision 0 despite a saved project, so revision numbers are session-relative; this can confuse saved-state verification.

**Improvements beyond MCP**
- Document preview image duplication, `image_region` semantics, baseline snapping, and label-orientation control.
- Provide an explicit “labeled signal end” construct or intent flag so single-pin nets don’t warn.
- Support horizontal net-label placement on stubs without hand-written paths.
- Return a single preview image or explain before/after pairs.
- Compact preview framing around changed geometry.

**Workarounds used**
- Separate close-up renders for readable review.
- `assign_only:false` for the pull-up bank per the guide; `assign_only:true` for the fully wired capacitor bank.
- Relied on saved netlist rather than ERC alone.