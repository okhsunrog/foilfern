# Agent-assisted routing research

Foilfern uses the builtin router for ordinary connections. When fixed copper
blocks a route, the operator can run a bounded search over authorized removals,
supply a pad escape or choose placement alternatives. Every published repair
is a revision-bound patch checked against the native DRC and rebuilt plane
connectivity. Preview shows the exact patch and the measured paths so the
operator can apply it, discard it or try another request.
[Routing tools](../mcp-design.md#reviewed-builtin-routing)

## Engine comparison on STM32G474

The input was a two-layer board with 18 devices and 57 open connections, no
traces, vias or planes, 0.20 mm trace width and clearance, and 0.70/0.30 mm
via diameter/drill. The two Specctra implementations used the same 10 nm DSN.
Pipeline 9 used an SRJ translation with simplified pad shapes and without
separate THT-hole and drill-to-edge rules. Each result was imported onto a copy
of the native snapshot and checked there. These are single-board geometry
results. Build and worker overlap prevented a timing comparison.

| Router and settings | Open connections | New DRC errors | Vias |
| --- | ---: | ---: | ---: |
| Foilfern builtin A*, default via cost | 0 | 0 | 43 |
| Foilfern builtin A*, via cost 10 mm | 0 | 0 | 24 |
| Java Freerouting 2.4.1 | 0 | 20 | 19 |
| fastroute 0.1.14, strict width and no automatic neck-down | 0 | 0 | 18 |
| fastroute, no fanout, multiple orders | 0 | 0 | 11 |
| tscircuit Pipeline 9, capacity autorouter 0.0.1068 | 0 | 0 | 45 |

Freerouting's new findings were 0.15 mm tracks below the 0.20 mm minimum.
Pipeline 9 reached zero opens with 91 copper errors still present after its
global improvement pass. Its subsequent joint repair removed those errors.
The 11-via fastroute result used 891.160 mm of copper. Foilfern's 24-via result
used 743.282 mm. A separate PD sweep found larger losses of existing GND
connectivity at higher builtin via costs, so the production default stays
at 2 mm. Keep existing connectivity and measured pad-pair costs in the
comparison alongside via count.

KRT 0.24.0 rerouted one open U2.2 connection on the original KiCad PD board.
Refining the local pitch from 0.10 to 0.05 mm shortened the result from
2.376 to 1.980 mm at zero vias and zero new KiCad DRC errors. On the entire
STM32 board a smaller pitch left opens. Local refinement is most useful
where there is a known access or corridor problem.

A pinned pnsrouter 0.1.1 probe demonstrated single-layer shove on a synthetic
obstacle with retained endpoints. It shortened the target by 3.740 mm and
lengthened the displaced branch by 4.148 mm. On the held two-layer ESP32-C3
GPIO3 case, moving the authorized EN chain gave the same stubs as keeping it
fixed and did not reach the destination. That adapter snapshot omitted the
GND plane and outline. Treat shove as a candidate strategy for further study.

## Native workflow results

| Case | Checked result | Remaining condition |
| --- | --- | --- |
| ESP32-C3 GPIO3 | Local repair followed by builtin continuation after diagnostics reduced six opens to five. The target stayed on Top and no new errors were introduced. | The result depends on the explicitly authorized EN/GND copper. |
| ESP32-C3 GPIO6 | An explicit escape and joint search closed GPIO6 on a copy with C5 at 1.550 mm. The matched exploratory route took 42.593 mm with three transitions. | GPIO2 grew from 46.884 mm and one transition to 70.902 mm and three. With the experimental GPIO2 limit of 50 mm and one transition, 58 trials with exact branch endpoints yielded no candidate. |
| ESP32-C3 GPIO6 with continuation evidence | An explicit escape exposed the Bottom GPIO2 diagonal and V3V3 branch. Joint repair of the two proposed branches replaced seven traces and closed GPIO6 at 37.147 mm with four transitions. C5 stayed at 1.550 mm and GPIO2 measured 47.030 mm with its original one transition. | The launch and edit permissions were supplied by the operator. Four other opens remain on this snapshot. |
| ESP32-C3 GPIO6, generated escape sites | Starting from U1.12 and a 1 mm radius, the worker found five sites. All five closed GPIO6 when the same seven neighbor traces were authorized. The top-ranked site independently reproduced the 37.147 mm result. | None of the sites closed GPIO6 with fixed neighbor copper. The agent still selects which branch removals to authorize. |
| ESP-PD-Sink, staged 146-open board | Joint repair replaced three GPIO4 traces between the retained via and junction. Both ESP_EN opens closed, for 146 → 144. Apply, undo/redo, save/reopen and LibrePCB 2.1.1 agreed. | One new useless-via warning in GPIO4. Sequential repair left both ESP_EN connections open. |
| ESP-PD-Sink, separate fully connected review snapshot | Region-based placement tried 16 of 81 possible C29/C30 poses. The selected candidate changed the sum of four measured supply paths from 7.971 to 7.736 mm, preserved both In1 contacts and added no findings. | The comparison measures centerline geometry and connectivity. The two PD snapshots serve different tests. |

The native additions include trials over order and via cost, pad-pair
preferences, checked staging with smaller batches, explicit copper edits
with automatic continuation, bounded repair with fixed outer anchors, and
bounded placement and rotation search. The joint-repair experiment informs
the existing A* workflow. External engines remain comparison tools, apart
from the existing Freerouting integration.

GPIO6 covers diagnosis through a staged escape, generated escape sites and
joint branch repair. The next repair step is to feed failed site continuations
into blocker diagnosis before the operator authorizes branch edits. Board-level
persistence of pad-pair and placement requirements is tracked in
[P2](../roadmap.md#p2--persisted-requirements-and-evidence).
