# Read-only path measurement in Foilfern

Rust implementation [`204800e`](https://github.com/okhsunrog/foilfern/commit/204800e)
adds `trace_path_measure` to MCP with its algorithm in core. It replaces the
manual board extraction/segment-collection step for reviewing explicit pad paths.
There is no new dependency and no native format change.

The release MCP process validates all **18 saved ESP-PD-Sink pairs**: 17 measured
paths match the independent Python review prototype within 0.000001 mm, with
identical layers and unique via counts. U5.1 → C16.1 has no explicit trace path;
it returns partial `not_measured`, null length and a reason mentioning the
unmeasured plane/contact connection. It does not report zero or disconnected.

The query returns project UUID, board UUID, source revision, resolved component/
physical-pad/signal identities, direct pad-center distance, shortest anchored
trace centerline length, ordered trace/segment IDs, layers, widths and via IDs.
Inputs accept human pad addresses or component-instance/footprint-pad UUIDs;
ambiguous physical endpoints refuse. Different nets, missing pads and stale
revision tokens refuse. UUID and label queries produce identical results.

The graph separates shared library pad UUIDs by component instance and local
anchors by segment/layer. Only via/pad copper joins layers. Plane copper,
unanchored copper overlaps and via barrels are explicitly excluded. Same-net
plane presence is a hint, not evidence that a plane joins the requested pads.
This length query does not replace DRC or rebuilt-plane connectivity checks.

| Original snapshot pair | Length, mm | Via anchors | Layers |
| --- | ---: | ---: | --- |
| OLED1.27 → C22.1 | 9.373741 | 0 | bot_cu |
| OLED1.28 → C21.1 | 10.081973 | 2 | bot_cu, in2_cu |
| OLED1.2 → C23.2 | 2.931394 | 0 | bot_cu |
| OLED1.3 → C23.1 | 4.381040 | 0 | bot_cu |
| OLED1.4 → C24.2 | 2.957805 | 0 | bot_cu |
| OLED1.5 → C24.1 | 4.282320 | 0 | bot_cu |
| OLED1.14 → R22.2 | 7.889563 | 2 | bot_cu, top_cu |
| OLED1.14 → C27.1 | 12.124960 | 2 | bot_cu, top_cu |
| OLED1.14 → D4.2 | 15.063831 | 2 | bot_cu, top_cu |
| U5.3 → C15.2 | 6.174727 | 0 | top_cu |
| U5.3 → C16.2 | 3.484777 | 0 | top_cu |
| U5.1 → C16.1 | not measured (plane/contact path excluded) | — | — |
| U5.5 → R14.1 | 4.612894 | 2 | in2_cu, top_cu |
| U5.5 → R16.2 | 3.732894 | 2 | in2_cu, top_cu |
| U5.4 → R30.2 | 2.032119 | 0 | top_cu |
| U6.9 → R17.2 | 7.565300 | 0 | top_cu |
| U6.10 → R17.3 | 8.575017 | 2 | in2_cu, top_cu |
| U1.8 → C4.2 | 32.484514 | 4 | bot_cu, in2_cu, top_cu |

The source is the unchanged original `foilfern/` snapshot from the handoff
archive, opened as a temporary copy. No mutations, save, DRC or plane rebuild
is requested. Board JSON and history before/after match exactly; revision 0
and all 482 native file hashes remain unchanged. No PCB revision is selected.

Validation passes: all 32 routing-candidate MCP integration tests (including
three new path-measurement tests), formatting, and clippy for all targets of
core and MCP with warnings denied. Tests cover equal pad UUIDs on different
instances, shortest competing branches, UUID addressing, no plane-length
inference, stale/missing/different-net refusals, real two-via layer transitions
and read-only state. Tests use the prior opt-level=2 dev overrides for core,
clipper, editor and autoroute; real-board replay uses the default release build:

```sh
cargo build --locked --release -p librepcb-mcp --bin librepcb-mcp
```

Example MCP request on the original snapshot:

```json
{"board":"4a16dd24-f4cb-42b9-bd1d-9bfc5daae272","from":"OLED1.28","to":"C21.1","expected_revision":0}
```

It returns 10.081973210208048 mm, two via anchors and Bottom/In2, together
with resolved identities and exact trace IDs. U5.1 → C16.1 instead returns
`status: "not_measured"`, `length_mm: null`, `plane_copper_measured: false`.

Reproduce after extracting the handoff archive to `/tmp/input`:

```sh
python3 replay.py /path/to/target/release/librepcb-mcp /tmp/input/foilfern /path/to/review/intent.json /tmp/path-api-review
```

The [replay script](replay.py) also checks tools/list advertisement, UUID lookup,
refusals, unchanged board/history and native files. The prototype `measure.py`
next to `intent.json` is its independent comparator. [Results](results.json)
contain exact requests' measured/reference values; [context](context.json)
records source commit, build profile, compiler and binary identity.
[evidence.tar.gz](evidence.tar.gz) preserves exact JSON-RPC requests/replies,
file manifests, test/clippy/build logs. Binaries and generated project copies
remain outside Git. This completes the handoff's release baseline/cache,
competing builtin profiling and read-only path measurement steps. PCB placement,
export and connected-footprint side changes remain separate tasks.
