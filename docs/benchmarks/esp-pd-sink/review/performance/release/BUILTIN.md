# Bounded builtin profiling with competing OLED routes

On the original handoff board, builtin routes both selected nets but cuts a
plane-fed GND connection. The complete candidate check detects the regression
and apply refuses in both release binaries. Baseline reuse preserves this
protection after warmup. Repeated preparation falls from **26.553 to 14.667 s**
(mean of two repeats, **44.8% less time**). No route-search optimization is
justified by this case: master spends 94–95% of preparation in core DRC, leaving
at most 1.50 s for everything else, including search.

This uses the exact before/after commits, preserved release binaries, compiler,
native snapshot and serial execution described in [REPORT.md](REPORT.md) and
[context.json](context.json). There are no concurrent builds. The request selects
VCOMH and VCC in the same 7 × 8 mm OLED region, with 0.1 mm grid, 0.2 mm traces,
pad escape, Bottom/In2, and an explicit list of 20 traces and two vias permitted
for rip-up. Other copper remains protected. See [builtin-input.json](builtin-input.json).

Each fresh process prepares, previews and discards three candidates on revision
0; on the third, an attempted apply must refuse. Afterwards board geometry and
history equal the initial values, full DRC still reports the original three
errors/nine warnings, and native hashes remain unchanged. Each candidate's
after-check, including warmed candidates, is a fresh full DRC.

| Prepare wall time, seconds | Master before | Cache after |
| --- | ---: | ---: |
| First | 25.898 | 27.097 |
| Repeat 1 | 26.441 | 14.653 |
| Repeat 2 | 26.664 | 14.681 |
| Mean repeats | 26.553 | 14.667 |

Core before/after DRC times on master are 12.692/11.782, 12.485/12.575 and
12.035/13.135 seconds. Remaining wall times are 1.424, 1.381 and 1.494 s.
These are upper bounds for search, not isolated A* timings: snapshot I/O,
plane work, rip-up, model extraction and patch retention also occupy them.
Patched repeat after-checks take 13.399 and 13.457 s, with source reuse flags
`false, true, true`. Small timing samples do not establish distributions.

All six candidates have identical full validation and router output across
versions: **6 searches, 2 rip-ups, 2 routed target connections, 21 generated
traces and 2 generated vias**, no failed target requests. Whole-board checking
finds one new open GND connection, from the existing via at `(147.18, -109.053)`
to the footprint pad at `(146.05, -106.8)` mm. Existing copper-error findings
remain unchanged; resolving target air wires alone does not make this route safe.
Both apply attempts refuse and leave live state untouched.

The frozen candidate patches have the same normalized geometry in all repeats
and both versions. A restricted replay of their three mutation variants shows
1010 total traces / 145 vias / 1529.867641 mm centerline length, versus source
1009 / 145 / 1529.787543 mm. This geometry is never selected or saved. The replay
does not infer plane contacts or via-barrel length; full native candidate DRC
provides the separate connectivity evidence. Automatic via dimensions stay
marked as automatic in the comparison instead of being guessed.

Reproduce serially, using fresh labels/output directories:

```sh
python3 profile_region.py /path/to/before-mcp region-before /tmp/input/foilfern builtin-input.json /tmp/release-review
python3 profile_region.py /path/to/after-mcp region-after /tmp/input/foilfern builtin-input.json /tmp/release-review
python3 region_geometry.py /tmp/release-review/region-before-records.json builtin-input.json
python3 region_geometry.py /tmp/release-review/region-after-records.json builtin-input.json
```

[profile_region.py](profile_region.py) verifies the known rejection, unchanged
history/board/native files and baseline DRC. [region_geometry.py](region_geometry.py)
measures the frozen patch without applying it; unsupported mutation variants
refuse. [builtin-comparison.json](builtin-comparison.json) preserves exact
measurements and equivalence checks. [builtin-evidence.tar.gz](builtin-evidence.tar.gz)
contains all raw requests/replies, previews, logs and derived geometry summaries.

The next product task is the handoff's read-only pad-to-pad path measurement
API. This experiment provides no evidence for an A* speedup; it demonstrates
that the baseline optimization also helps real rip-up candidates while retaining
the rebuilt-plane guard.
