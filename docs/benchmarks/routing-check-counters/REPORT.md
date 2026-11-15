# Routing-check categories and status

2026-11-15. F10 is delivered on master, API 62. The reported contradiction was a
comparison of different categories: the before/after error total includes missing
connections, while the pre-existing new/resolved counters exclude them. The
new `finding_delta` reports a complete, consistent breakdown rather than silently
changing the local routing gates.

## Result

The release replay routes two initially open nets in separate reviewed steps.
The second step reproduces the feedback's exact situation:

| Evidence | Baseline | Updated |
| --- | --- | --- |
| Total errors | 1 → 0 | 1 → 0 |
| Existing `resolved_error_count` (non-connectivity only) | 0 | 0 |
| `finding_delta.errors.resolved` | absent | 1 |
| `finding_delta.missing_connections.resolved` | absent | 1 |
| Explicit after-check status | absent | `no_errors_or_warnings` |

For errors, non-connectivity errors, copper errors, missing connections and
warnings, each delta reports before/after/added/resolved/unchanged. The equations
`before = resolved + unchanged` and `after = added + unchanged` hold independently
in every category. Native approval identity matching retains repeated messages,
rather than deduplicating their counts while retaining a raw summary count.
Samples still contain at most 50 distinct identities; counts are not truncated.

Adding a via connected on only one layer produces one real `UselessVia` warning.
A subsequent no-op candidate retains it: zero errors, one unchanged warning,
`drc_clean:true`, **`drc_status: warnings_only`**. The inherited warning is neither approved
nor removed in this replay. `acceptable` remains distinct from whole-board
error-free status. The final geometry, effective via sizes, zero air wires,
zero DRC errors and one DRC warning match in all six runs.

The counters concern full-board **unapproved** messages, not only edited nets.
Raw air-wire counts include approved missing connections. Hints are outside the
error/warning status. A changed rebuilt air-wire tree can change edge identities;
per-net connectivity comparisons remain the regression evidence. A warning
approved by the user leaves this reporting scope without its geometry vanishing.

If either check is incomplete, `finding_delta` is null and no resolved count is
claimed. A failed post-apply check also invalidates the candidate's prior delta
and status, while honestly reporting that copper was already committed. The
native DRC settings, approval serialization and project formats are unchanged.

## Workflow and measured cost

Three alternating pairs use fresh workspaces and projects, with the same source
library and the same 13-call sequence: server_info; prepare/preview/apply net A;
prepare/apply net B; add a via; prepare/preview/apply the no-op; DRC; board readback;
save. Workspace/library/project setup is excluded from timing. Measurements
include process startup, project loading, routing, checking and saving. No
additional fetch call is needed to obtain the new evidence.

| Measurement | Baseline | Updated |
| --- | ---: | ---: |
| Calls per replay | 13 | 13 |
| Elapsed seconds, sample 1 | 0.1721 | 0.1733 |
| Elapsed seconds, sample 2 | 0.1607 | 0.1692 |
| Elapsed seconds, sample 3 | 0.1590 | 0.1548 |
| Median seconds | 0.1607 | 0.1692 |
| Tool response bytes per replay | 19,879 | 24,267 |

The observed median increases by 8.5 ms (5.3%); three short observations on a
shared laptop do not establish a latency regression or improvement. Response
size increases by 4,388 bytes (22.1%); each validation-bearing response adds
547–555 bytes. Minor remaining byte differences come from local path text.
Byte counts use compact UTF-8 JSON tool responses, including content text and
structured content, excluding local batch framing. They are not model tokens.
No model/delegated-agent usability runs or tokenizer measurements were made.

## Provenance and resource limits

- Baseline binary: clean source `0b5bb62a8615ea9dd3bf30d8835da76c1eafc479`,
  API 61, Cargo release, opt-level 3. This is product-code-equivalent to pre-task
  master `3cad908a7c37225e60bbde16a03d35a117f60b66` (only evidence/docs followed).
  Path `/home/okhsunrog/tmp_zfs/codex_tmp/routing-check-counters/baseline-mcp`;
  SHA-256 `41c4088d48cbf1d3ded1756e7159892460ec1203171cd9d9237725e62c451694`.
- Updated binary: clean source `df9c3c7796ba8c76ed469d0f0a2c67210b516b18`,
  API 62, Cargo release, opt-level 3. Path
  `/home/okhsunrog/code/rust/foilfern/target/release/foilfern-mcp`;
  SHA-256 `6d180edc2d06f2b646d2d184134f0b610c24a48afc3b66a11a6b10f1a73149f6`.
- Build command: `cargo build --locked --release -p foilfern-mcp --bin foilfern-mcp`.
  Build took 3m45s under the resource cap; no build-speed comparison is claimed.
- Source library: upstream "Populated Library", checkout
  `99ff973ac20f3753bdb30d4e29608aadc882674f`, selected `R-0805`.
- All builds, tests and replay processes were pinned to logical CPUs 0–1,
  nice 10. Cargo jobs, test/Rayon/Tokio thread limits were two. The actual
  compiler affinity/priority was inspected; each replay records inherited
  affinity `[0,1]` and nice 10. Heavy tasks ran sequentially. Normal Cargo
  target/cache configuration was retained.
- Requests, raw JSONL, replay script, temporary projects and logs remain under
  `/home/okhsunrog/tmp_zfs/codex_tmp/routing-check-counters/`.

## Validation boundary

109 MCP integration tests passed with HTTP enabled; two live network-source tests
were ignored. Editor tests cover repeated native finding identities and failed
checks/netlist changes. Actual DRC scenarios cover removal of missing connections,
repair of width violations, introduction of clearance violations, retained
warnings, explicit approval scope and incomplete coverage. Existing candidate
isolation, revision checks and undo tests passed. Clippy with warnings denied
passed for editor/MCP, all targets, HTTP enabled; formatting and diff checks
passed. No full-workspace, live desktop, electrical-intent or manufacturing
acceptance is claimed.

[Machine-readable measurements](measurements.json).
