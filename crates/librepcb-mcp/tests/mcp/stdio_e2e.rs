//! End-to-end test over stdio: runs the `librepcb-mcp` binary as child
//! process and talks to it with the rmcp client.

use std::path::Path;

use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ReadResourceRequestParams,
    ResourceContents,
};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use serde_json::{Value, json};

async fn call(client: &RunningService<RoleClient, ()>, tool: &str, args: Value) -> CallToolResult {
    let args = match args {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"))
}

/// Calls a tool which must succeed; returns the `result` of the envelope.
async fn call_ok(
    client: &RunningService<RoleClient, ()>,
    tool: &'static str,
    args: Value,
) -> Value {
    let res = call(client, tool, args).await;
    let envelope = res.structured_content.clone().expect("structured content");
    assert_ne!(res.is_error, Some(true), "{tool} failed: {envelope:#}");
    assert_eq!(envelope["outcome"], "complete", "{tool}: {envelope:#}");
    assert!(
        matches!(res.content.first(), Some(ContentBlock::Text(_))),
        "{tool}: text summary first"
    );
    envelope["result"].clone()
}

#[tokio::test]
async fn stdio_repair_search_deduplicates_groups_and_publishes_a_checked_candidate() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    call_ok(&client, "project_create", json!({"name":"Repair search"})).await;
    for (designator, x) in [("R1", 5), ("R2", 15)] {
        call_ok(
            &client,
            "component_add",
            json!({"part":"R-0805","designator":designator}),
        )
        .await;
        call_ok(
            &client,
            "device_place",
            json!({"component":designator,"position":{"x":x,"y":10},"rotation":90}),
        )
        .await;
    }
    for (net, pins) in [("A", ["R1.1", "R2.1"]), ("B", ["R1.2", "R2.2"])] {
        call_ok(&client, "connect", json!({"net":net,"pins":pins})).await;
    }
    call_ok(
        &client,
        "board_set_outline",
        json!({"width":30,"height":20}),
    )
    .await;
    let trace = call_ok(
        &client,
        "trace_add",
        json!({"from":"R1.2","to":"R2.2","layer":"top","width":0.2}),
    )
    .await;
    let id = &trace["traces"][0]["uuid"];
    let before = call_ok(&client, "board_get", json!({})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let mut request = json!({
        "target":"A", "groups":[[id],[id]],
        "region":{"min":{"x":3,"y":7},"max":{"x":17,"y":13}},
        "rip_up_region":{"min":{"x":4,"y":10.5},"max":{"x":16,"y":12}},
        "route":{"layers":["top"]},"neighbor_route":{"layers":["top"]},
        "joint_route":{"layers":["top"]},
        "trace_requirements":[
            {"path":{"from":"R1.2","to":"R2.2"},"max_length_mm":10.1,"max_layer_changes":0},
            {"path":{"from":"R1.1","to":"R2.1"}}
        ],
    });
    let poll = |search: Value| {
        let client = &client;
        async move {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let status =
                        call_ok(client, "routing_repair_status", json!({"search":search})).await;
                    if status["phase"] == "finished" {
                        break status;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap()
        }
    };
    request["trace_requirements"][0]["max_length_mm"] = json!(9.9);
    let strict = call_ok(&client, "routing_repair_start", request.clone()).await;
    let rejected = poll(strict["search"].clone()).await;
    assert_eq!(rejected["candidates"], json!([]));
    assert_eq!(rejected["trials"][0]["target_unrouted"], 0);
    assert_eq!(rejected["trials"][0]["validation"]["acceptable"], false);
    assert_eq!(
        rejected["trials"][0]["trace_requirements"][0]["after"]["met"],
        false
    );
    assert_eq!(
        rejected["trials"][0]["trace_requirements"][1]["after"]["met"],
        true
    );

    request["trace_requirements"][0]["max_length_mm"] = json!(10.1);
    let started = call_ok(&client, "routing_repair_start", request).await;
    assert_eq!(started["planned_trials"], 2);
    let status = poll(started["search"].clone()).await;
    assert_eq!(status["stop_reason"], "completed", "{status:#}");
    assert_eq!(status["completed_trials"], 2);
    assert_eq!(status["trials"][0]["validation"]["acceptable"], true);
    assert_eq!(status["trials"][0]["strategy"], "sequential");
    assert_eq!(status["trials"][1]["strategy"], "joint");
    assert_eq!(
        status["trials"][1]["validation"]["acceptable"], true,
        "{status:#}"
    );
    assert_eq!(status["trials"][0]["rip_up"], status["trials"][1]["rip_up"]);
    assert_eq!(status["candidates"][0]["groups"], json!([0]));
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    assert_eq!(call_ok(&client, "board_get", json!({})).await, before);
    let joint = status["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice["strategy"] == "joint")
        .unwrap();
    let candidate = &joint["candidate"];
    let preview = call_ok(&client, "routing_preview", json!({"candidate":candidate})).await;
    assert!(preview["mutation_count"].as_u64().unwrap() > 0);
    assert_eq!(preview["repair"]["strategy"], "joint");
    let paths = &preview["repair"]["trace_requirements"];
    assert_eq!(paths[0]["before"]["length_mm"], 10.0);
    assert_eq!(paths[0]["after"]["length_mm"], 10.0);
    assert_eq!(paths[0]["after"]["met"], true);
    assert_eq!(paths[1]["before"]["met"], false);
    assert_eq!(paths[1]["after"]["met"], true);
    assert_eq!(preview["repair"]["joint"]["rip_up"], json!([id]));
    assert_eq!(
        call_ok(&client, "routing_apply", json!({"candidate":candidate})).await["validation"]["after"]
            ["unrouted"],
        0
    );
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "board_get", json!({})).await, before);
    call_ok(&client, "trace_remove", json!({"uuids":[id]})).await;
    call_ok(
        &client,
        "trace_add",
        json!({
            "from":"R1.2","to":"R2.2","layer":"top","width":0.2,
            "points":[{"x":6,"y":11},{"x":14,"y":11}]
        }),
    )
    .await;
    let source = call_ok(&client, "board_get", json!({})).await;
    let traces = source["traces"].as_array().unwrap();
    let branch = traces
        .iter()
        .find(|trace| trace["from"]["junction"].is_string() && trace["to"]["junction"].is_string())
        .unwrap();
    let (a, b) = (&branch["from"]["junction"], &branch["to"]["junction"]);
    let mut anchored = json!({
        "target":"A", "groups":[[branch["uuid"]]],
        "neighbor_anchors":[{"group":0,"from":{"junction":a},"to":{"junction":b}}],
        "region":{"min":{"x":3,"y":7},"max":{"x":17,"y":13}},
        "rip_up_region":{"min":{"x":5,"y":10.5},"max":{"x":15,"y":12}},
        "route":{"layers":["top"]},"neighbor_route":{"layers":["top"]},
        "joint_route":{"layers":["top"]},
    });
    anchored["neighbor_anchors"][0]["to"] = json!({"pad":"R2.2"});
    let invalid = call(&client, "routing_repair_start", anchored.clone()).await;
    assert_eq!(invalid.is_error, Some(true));
    assert!(
        invalid.structured_content.unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("single branch")
    );
    anchored["neighbor_anchors"][0]["to"] = json!({"junction":b});
    let started = call_ok(&client, "routing_repair_start", anchored).await;
    let status = poll(started["search"].clone()).await;
    let selected = status["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|choice| choice["strategy"] == "joint")
        .unwrap_or_else(|| panic!("{status:#}"));
    assert_eq!(call_ok(&client, "board_get", json!({})).await, source);
    let preview = call_ok(
        &client,
        "routing_preview",
        json!({"candidate":selected["candidate"]}),
    )
    .await;
    let wire = &preview["repair"]["neighbor_branches"][0];
    assert_eq!(wire["p1"]["Junction"], *a);
    assert_eq!(wire["p2"]["Junction"], *b);
    call_ok(
        &client,
        "routing_apply",
        json!({"candidate":selected["candidate"]}),
    )
    .await;
    let after = call_ok(&client, "board_get", json!({})).await;
    let after_traces = after["traces"].as_array().unwrap();
    assert_eq!(after["air_wires"], json!([]));
    assert!(
        !after_traces
            .iter()
            .any(|trace| trace["uuid"] == branch["uuid"])
    );
    for anchor in [a, b] {
        assert!(after_traces.iter().any(|trace| {
            !traces.iter().any(|old| old["uuid"] == trace["uuid"])
                && (trace["from"]["junction"] == *anchor || trace["to"]["junction"] == *anchor)
        }));
    }
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "board_get", json!({})).await, source);

    let mut auto = json!({
        "target":"A",
        "region":{"min":{"x":3,"y":7},"max":{"x":17,"y":13}},
        "neighbor_region":{"min":{"x":4,"y":8},"max":{"x":16,"y":12}},
        "route":{"via_drill":0.3,"via_size":0.6,"layers":["top","bottom"]},
        "target_launch_search":{"from":"R1.1","layer":"top","radius":0.8,"max_sites":2},
        "trace_requirements":[{"path":{"from":"R1.1","to":"R2.1"}}],
    });
    auto["target_launch_search"]["radius"] = json!(1.51);
    let invalid = call(&client, "routing_repair_start", auto.clone()).await;
    assert_eq!(invalid.is_error, Some(true));
    auto["target_launch_search"]["radius"] = json!(0.8);
    let history = call_ok(&client, "history", json!({})).await;
    let started = call_ok(&client, "routing_repair_start", auto).await;
    let status = poll(started["search"].clone()).await;
    let sites = status["launch_sites"].as_array().unwrap();
    assert_eq!(sites.len(), 2, "{status:#}");
    assert_ne!(sites[0]["position"], sites[1]["position"]);
    assert_eq!(status["planned_trials"], 2);
    assert_eq!(status["completed_trials"], 2);
    assert_eq!(status["trials"][0]["target_launch"], sites[0]);
    assert_eq!(status["trials"][1]["target_launch"], sites[1]);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    assert_eq!(call_ok(&client, "board_get", json!({})).await, source);
    let selected = &status["candidates"][0];
    assert!(selected["candidate"].is_string(), "{status:#}");
    let preview = call_ok(
        &client,
        "routing_preview",
        json!({"candidate":selected["candidate"]}),
    )
    .await;
    assert_eq!(
        preview["repair"]["target_launch"],
        selected["target_launch"]
    );
    call_ok(
        &client,
        "routing_apply",
        json!({"candidate":selected["candidate"]}),
    )
    .await;
    assert_eq!(
        call_ok(&client, "board_get", json!({})).await["air_wires"],
        json!([])
    );
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "board_get", json!({})).await, source);

    call_ok(&client, "design_rules_set", json!({"min_copper_width":0.2})).await;
    // A neighboring trace crosses the only via search area. Its exact
    // UUID must be authorized before any launch can be generated.
    let blocker = call_ok(
        &client,
        "trace_add",
        json!({
            "from":{"x":4.2,"y":8.95},"to":{"x":5.8,"y":8.95},
            "layer":"bottom","net":"B","width":0.9,
        }),
    )
    .await;
    let blocker = &blocker["traces"][0]["uuid"];
    let source = call_ok(&client, "board_get", json!({})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let mut request = json!({
        "target":"A",
        "region":{"min":{"x":2,"y":6},"max":{"x":17,"y":13}},
        "target_launch_search":{"from":"R1.1","layer":"top","radius":0.8,"max_sites":1},
        "route":{"via_drill":0.3,"via_size":0.6},
    });
    let started = call_ok(&client, "routing_repair_start", request.clone()).await;
    let status = poll(started["search"].clone()).await;
    assert_eq!(status["launch_sites"], json!([]), "{status:#}");
    request["groups"] = json!([[blocker]]);
    request["rip_up_region"] = json!({"min":{"x":3.5,"y":7},"max":{"x":6.5,"y":10}});
    let started = call_ok(&client, "routing_repair_start", request).await;
    let status = poll(started["search"].clone()).await;
    assert_eq!(
        status["launch_sites"].as_array().unwrap().len(),
        1,
        "{status:#}"
    );
    assert_eq!(status["trials"][0]["rip_up"], json!([blocker]));
    assert_eq!(call_ok(&client, "board_get", json!({})).await, source);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    let region = json!({"min":{"x":2,"y":6},"max":{"x":17,"y":13}});
    let edits = json!({"min":{"x":3.5,"y":7},"max":{"x":16,"y":12}});
    let started = call_ok(
        &client,
        "routing_repair_start",
        json!({
            "target":"A","region":region,"rip_up_region":edits,
            "groups":[[branch["uuid"]]],
            "target_launch":{
                "from":"R1.1","layer":"top","position":{"x":5,"y":8.4},
                "width":0.2,"drill":0.3,"size":0.6
            }
        }),
    )
    .await;
    let status = poll(started["search"].clone()).await;
    assert_eq!(status["candidates"], json!([]), "{status:#}");
    let diagnosed = call_ok(
        &client,
        "routing_diagnose",
        json!({
            "net":"A","repair":{
                "region":region,"rip_up_region":edits,"editable_nets":["B"],
                "evidence_search":started["search"]
            }
        }),
    )
    .await;
    let repair = &diagnosed["repair"];
    let failure = &repair["validation_failures"][0];
    assert_eq!(
        failure["source_copper"],
        json!([blocker]),
        "{repair:#}\n{status:#}"
    );
    assert_eq!(
        failure["clearance_conflicts"][0]["layers"],
        json!(["bot_cu"])
    );
    assert_eq!(repair["proposals"][0]["remove_traces"], json!([blocker]));
    assert_eq!(
        repair["proposals"][0]["addresses_clearance_in_trials"],
        json!([0])
    );
    assert_eq!(call_ok(&client, "board_get", json!({})).await, source);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    call_ok(&client, "undo", json!({})).await;

    let arrival = call_ok(
        &client,
        "via_add",
        json!({"net":"A","position":{"x":14,"y":8.5},"drill":0.3,"size":0.7}),
    )
    .await;
    call_ok(
        &client,
        "trace_add",
        json!({"from":"R2.1","to":arrival["uuid"],"layer":"top","width":0.2}),
    )
    .await;
    let wall = call_ok(
        &client,
        "trace_add",
        json!({
            "from":{"x":10,"y":-2},"to":{"x":10,"y":22},"layer":"bottom",
            "net":"B","width":0.2,
        }),
    )
    .await;
    let before = call_ok(&client, "board_get", json!({})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let region = json!({"min":{"x":0,"y":-3},"max":{"x":23,"y":23}});
    let started = call_ok(
        &client,
        "routing_repair_start",
        json!({
            "target":"A","region":region,
            "route":{
                "trace_width":0.2,"grid_pitch":0.1,"via_drill":0.3,"via_size":0.6,
                "layers":["bottom"],"search_evidence":true,
            },
            "target_launch_search":{"from":"R1.1","layer":"top","radius":0.8,"max_sites":1},
        }),
    )
    .await;
    let status = poll(started["search"].clone()).await;
    assert_eq!(status["stop_reason"], "completed", "{status:#}");
    assert_eq!(status["candidates"], json!([]), "{status:#}");
    let request = json!({
        "net":"A","layers":["bottom"],
        "repair":{
            "region":region,
            "rip_up_region":{"min":{"x":9,"y":-3},"max":{"x":11,"y":23}},
            "editable_nets":["B"],
            "evidence_search":started["search"],
        }
    });
    let diagnosed = call_ok(&client, "routing_diagnose", request.clone()).await;
    let repair = &diagnosed["repair"];
    let sites = repair["escape_sites"].as_array().unwrap();
    let wall = &wall["traces"][0]["uuid"];
    assert_eq!(sites.len(), 1, "{repair:#}\n{status:#}");
    assert_eq!(repair["proposals"][0]["remove_traces"], json!([wall]));
    assert_eq!(repair["proposals"][0]["escape_sites"], json!([0]));
    for site in sites {
        assert_eq!(site["reason"], "no_path");
        assert!(site["blockers"].as_array().unwrap().contains(wall));
        let launch = &site["target_launch"];
        assert_eq!(launch["layer"], "top_cu");
        assert!(launch["from"]["pad"].is_string());
        assert!((4.0..6.0).contains(&launch["position"]["x"].as_f64().unwrap()));
    }
    assert_eq!(call_ok(&client, "board_get", json!({})).await, before);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    call_ok(&client, "undo", json!({})).await;
    let stale = call(&client, "routing_diagnose", request).await;
    assert_eq!(
        stale.structured_content.unwrap()["error"]["kind"],
        "stale_revision"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_server_info_reports_embedded_build_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.current_dir(tmp.path()).env_remove("PATH");
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    assert_eq!(info["api_capability_version"], 38);
    let build = &info["build"];
    let commit = env!("FOILFERN_SOURCE_COMMIT");
    assert_eq!(
        build["source_commit"],
        json!((!commit.is_empty()).then_some(commit))
    );
    assert_eq!(
        build["tracked_source_state"],
        env!("FOILFERN_TRACKED_SOURCE_STATE")
    );
    assert_eq!(build["cargo_profile"], env!("FOILFERN_CARGO_PROFILE"));
    assert_eq!(build["cargo_opt_level"], env!("FOILFERN_CARGO_OPT_LEVEL"));
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_connects_selected_device_pads_before_placement() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let created = call_ok(&client, "project_create", json!({"name":"Selected Device"})).await;
    let path = &created["path"];

    // Cache a different device of the same component by placing it first.
    call_ok(
        &client,
        "component_add",
        json!({"part":"PSMN5R8","designator":"Q1"}),
    )
    .await;
    call_ok(
        &client,
        "device_place",
        json!({"component":"Q1","position":{"x":10,"y":10}}),
    )
    .await;
    let added = call_ok(
        &client,
        "component_add",
        json!({"part":"PSMN022-30PL","designator":"Q2"}),
    )
    .await;
    let selected = &added["selected_device"];
    assert_eq!(added["devices"], json!([]));
    let detail = call_ok(
        &client,
        "library_element",
        json!({"kind":"device","uuid":selected}),
    )
    .await;
    assert_eq!(detail["source"], "workspace");
    assert!(
        detail["pads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pad| { pad["pad_name"] == "1" && pad["signal_name"] == "G" })
    );
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({"path":path})).await;
    let reopened = call_ok(&client, "component_get", json!({"component":"Q2"})).await;
    assert_eq!(reopened["selected_device"], *selected);
    assert_eq!(reopened["devices"], json!([]));

    let invalid = call(&client, "connect", json!({"net":"WRONG","pins":["Q2.S@1"]})).await;
    assert_eq!(invalid.is_error, Some(true));
    assert_eq!(
        invalid.structured_content.unwrap()["error"]["kind"],
        "not_found"
    );

    let wiring = json!({"net":"GATE","pins":["Q2.1","Q1.G"]});
    let connected = call_ok(&client, "connect", wiring.clone()).await;
    assert_eq!(connected["pins"], json!(["Q1.G", "Q2.G"]));
    let disconnected = call_ok(&client, "disconnect", json!({"pins":["Q2.1"]})).await;
    assert_eq!(disconnected["disconnected"], 1);
    let net = call_ok(&client, "net_list", json!({"net":"GATE"})).await;
    assert_eq!(net["nets"][0]["pins"], json!(["Q1.G"]));
    call_ok(&client, "connect", wiring).await;
    let placed = call_ok(
        &client,
        "device_place",
        json!({"component":"Q2","position":{"x":20,"y":10}}),
    )
    .await;
    assert_eq!(placed["device"]["uuid"], *selected);
    assert!(
        placed["pads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pad| { pad["name"] == "1" && pad["signal"] == "G" && pad["net"] == "GATE" })
    );

    let position = json!({"x":25.0,"y":12.0});
    let response = call(
        &client,
        "device_place",
        json!({"component":"Q2","position":position,"rotation":90,"include_pads":false}),
    )
    .await;
    assert_ne!(response.is_error, Some(true));
    let envelope = response.structured_content.unwrap();
    assert_eq!(envelope["outcome"], "complete");
    assert_eq!(envelope["warnings"], json!([]));
    let moved = &envelope["result"];
    assert!(moved.get("pads").is_none());
    assert_eq!(moved["component"], placed["component"]);
    assert_eq!(moved["device"], placed["device"]);
    assert_eq!(moved["position"], position);
    assert_eq!(moved["rotation"], 90.0);
    assert!(envelope["revision"].as_u64().unwrap() > moved["revision_before"].as_u64().unwrap());
    assert!(moved["change_count"].as_u64().unwrap() > 0);
    let read_back = call_ok(&client, "component_get", json!({"component":"Q2"})).await;
    let full = call_ok(
        &client,
        "device_place",
        json!({"component":"Q2","include_pads":true}),
    )
    .await;
    assert_eq!(read_back["devices"][0]["position"], position);
    assert_eq!(full["pads"], read_back["devices"][0]["pads"]);
    assert_ne!(full["pads"], placed["pads"]);
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_symbol_variants_and_multisheet_four_layer_board() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let created = call_ok(&client, "project_create", json!({ "name": "Multisheet" })).await;
    let path = created["path"].as_str().unwrap();

    // Discover both representations of the upstream multi-gate fixture.
    let found = call_ok(
        &client,
        "library_search",
        json!({ "query": "Demo Component", "kind": "component" }),
    )
    .await;
    let part = found["elements"][0]["uuid"].as_str().unwrap();
    let detail = call_ok(&client, "library_element", json!({ "uuid": part })).await;
    let variants = detail["symbol_variants"].as_array().unwrap();
    for (name, count) in [("U1", 2), ("U2", 1)] {
        let variant = variants
            .iter()
            .find(|v| v["gates"].as_array().unwrap().len() == count)
            .unwrap();
        let added = call_ok(
            &client,
            "component_add",
            json!({
                "part": part, "designator": name, "symbol_variant": variant["uuid"]
            }),
        )
        .await;
        assert_eq!(added["symbol_variant"]["uuid"], variant["uuid"]);
        assert_eq!(added["symbols"].as_array().unwrap().len(), count);
        if count == 2 {
            let before = added["symbols"].clone();
            call_ok(
                &client,
                "symbol_move",
                json!({
                    "component": name, "gate": "A", "position": { "x": 80, "y": 30 }
                }),
            )
            .await;
            let moved = call_ok(&client, "component_get", json!({ "component": name })).await;
            for symbol in moved["symbols"].as_array().unwrap() {
                if symbol["gate_suffix"] == "A" {
                    assert_eq!(symbol["position"], json!({ "x": 80.0, "y": 30.0 }));
                } else {
                    let original = before
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|s| s["uuid"] == symbol["uuid"])
                        .unwrap();
                    assert_eq!(symbol["position"], original["position"]);
                }
            }
            call_ok(&client, "undo", json!({})).await;
            let restored = call_ok(&client, "component_get", json!({ "component": name })).await;
            assert_eq!(restored["symbols"], before);
            let symbols = before.as_array().unwrap();
            let gate = symbols
                .iter()
                .find(|symbol| symbol["gate_suffix"] == "A")
                .unwrap();
            let target = gate["uuid"].as_str().unwrap();
            let document = json!({"version":1,"schematic":"Main","components":{
                "opamp":{"uuid":added["uuid"],"symbols":{(target):{
                    "position":{"expected":gate["position"],"set":{"x":80,"y":30}},
                    "rotation":{"expected":gate["rotation"],"set":90},
                    "mirrored":{"expected":gate["mirrored"],"set":true}
                }}}
            }});
            let mut wrong_symbol = document.clone();
            wrong_symbol["components"]["opamp"]["symbols"] =
                json!({(part):{"rotation":{"expected":0,"set":90}}});
            assert_eq!(
                call(&client, "design_preview", wrong_symbol)
                    .await
                    .structured_content
                    .unwrap()["error"]["kind"],
                "not_found"
            );
            let mut conflicting = document.clone();
            conflicting["components"]["opamp"]["symbols"][target]["position"]["expected"] =
                json!({"x":999,"y":999});
            assert_eq!(
                call(&client, "design_preview", conflicting)
                    .await
                    .structured_content
                    .unwrap()["error"]["kind"],
                "conflict"
            );
            let preview = call_ok(&client, "design_preview", document.clone()).await;
            assert_eq!(preview["diff"].as_array().unwrap().len(), 3);
            assert!(
                preview["diff"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|row| row["symbol"] == target)
            );
            call_ok(
                &client,
                "design_apply",
                json!({"preview":preview["preview"]}),
            )
            .await;
            let moved = call_ok(&client, "component_get", json!({"component":name})).await;
            for symbol in moved["symbols"].as_array().unwrap() {
                if symbol["uuid"] == target {
                    assert_eq!(symbol["position"], json!({"x":80.0,"y":30.0}));
                    assert_eq!(symbol["rotation"], 90.0);
                    assert_eq!(symbol["mirrored"], true);
                } else {
                    assert!(symbols.contains(symbol));
                }
            }
            assert_eq!(
                call_ok(&client, "design_preview", document).await["diff"],
                json!([])
            );
            call_ok(&client, "undo", json!({})).await;
            assert_eq!(
                call_ok(&client, "component_get", json!({"component":name})).await["symbols"],
                before
            );
        }
    }
    // A malformed UUID and a UUID belonging to another entity must not add a component.
    for variant in ["invalid", part] {
        let failed = call(
            &client,
            "component_add",
            json!({
                "part": part, "designator": "U3", "symbol_variant": variant
            }),
        )
        .await;
        assert_eq!(failed.is_error, Some(true));
        let missing = call(&client, "component_get", json!({ "component": "U3" })).await;
        assert_eq!(
            missing.structured_content.unwrap()["error"]["kind"],
            "not_found"
        );
    }

    // Keep the selected multi-gate variant across a save/open cycle.
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({ "path": path })).await;
    let reopened = call_ok(&client, "component_get", json!({ "component": "U1" })).await;
    assert_eq!(reopened["symbols"].as_array().unwrap().len(), 2);
    for name in ["U1", "U2"] {
        call_ok(&client, "component_remove", json!({ "component": name })).await;
    }

    // A two-resistor network spans pages; both connections need visible labels.
    call_ok(&client, "schematic_add", json!({ "name": "IO" })).await;
    for (name, page) in [("R1", "Main"), ("R2", "IO")] {
        call_ok(
            &client,
            "component_add",
            json!({
                "part": "R-0805", "designator": name, "schematic": page, "value": "1k"
            }),
        )
        .await;
    }
    for (net, pins) in [("LINK", ["R1.1", "R2.1"]), ("RETURN", ["R1.2", "R2.2"])] {
        call_ok(&client, "connect", json!({ "net": net, "pins": pins })).await;
    }
    for page in ["Main", "IO"] {
        let schematic = call_ok(&client, "schematic_get", json!({ "schematic": page })).await;
        let pins = schematic["symbols"][0]["pins"].as_array().unwrap();
        assert!(pins.iter().all(|p| p["wired"] == true), "{schematic:#}");
        let segments = schematic["net_segments"].as_array().unwrap();
        assert_eq!(segments.len(), 2, "{schematic:#}");
        assert!(
            segments
                .iter()
                .all(|s| !s["labels"].as_array().unwrap().is_empty())
        );
    }
    let erc = call_ok(&client, "erc_run", json!({})).await;
    assert_eq!(erc["errors"], 0, "{erc:#}");
    call_ok(
        &client,
        "board_set_outline",
        json!({ "width": 30, "height": 20 }),
    )
    .await;
    let rules = call_ok(&client, "design_rules_set", json!({ "inner_layers": 2 })).await;
    assert_eq!(rules["copper_layers"], 4);
    call_ok(&client, "device_auto_place", json!({})).await;
    let routed = call_ok(&client, "autoroute", json!({ "backend": "builtin" })).await;
    assert_eq!(routed["unrouted"], 0, "{routed:#}");
    let edited = call_ok(
        &client,
        "trace_update",
        json!({"nets": ["LINK", "RETURN"], "width": 0.3}),
    )
    .await;
    assert_eq!(edited["validation"]["netlist_preserved"], true);
    assert_eq!(edited["validation"]["drc_clean"], true);
    let drc = call_ok(&client, "drc_run", json!({})).await;
    assert_eq!(drc["errors"], 0, "{drc:#}");
    let exported = call_ok(
        &client,
        "export_fabrication",
        json!({
            "output_dir": tmp.path().join("gerber")
        }),
    )
    .await;
    let files = exported["files"].as_array().unwrap();
    let copper = files
        .iter()
        .filter_map(Value::as_str)
        .filter(|f| f.contains("COPPER"))
        .count();
    assert_eq!(copper, 4, "{exported:#}");
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({ "path": path })).await;
    let board = call_ok(&client, "board_get", json!({})).await;
    assert_eq!(board["copper_layers"], 4);
    assert_eq!(board["devices"].as_array().unwrap().len(), 2);
    call_ok(&client, "project_close", json!({})).await;
    if let Some(cli) = crate::common::upstream_cli() {
        let copy = tempfile::tempdir().unwrap();
        crate::common::copy_dir(Path::new(path).parent().unwrap(), copy.path());
        let lpp = copy.path().join(Path::new(path).file_name().unwrap());
        let (ok, text) = crate::common::run_cli(
            &cli,
            &[
                "open-project",
                "--erc",
                "--drc",
                "--export-pcb-fabrication-data",
                lpp.to_str().unwrap(),
            ],
        );
        assert!(ok || text.contains("Finished with errors!"), "{text}");
        assert!(
            !text.contains("[ERROR]") && !text.contains("ERROR:"),
            "{text}"
        );
        assert!(text.contains("COPPER-IN1.gbr"), "{text}");
        eprintln!("{text}");
    } else {
        eprintln!("librepcb-cli not found, skipping the upstream check");
    }
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_exports_step_and_realistic_board_graphics() {
    let tmp = tempfile::tempdir().unwrap();
    let lpp = crate::common::copy_test_project("Project With Two Boards", tmp.path());
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--project").arg(lpp);
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let exported = call_ok(
        &client,
        "jobs_run",
        json!({
            "jobs": ["STEP Model", "Board Rendering PDF"],
            "output_dir": tmp.path().join("output")
        }),
    )
    .await;
    let files = exported["files"].as_array().unwrap();
    let step = files
        .iter()
        .filter_map(Value::as_str)
        .find(|file| file.ends_with(".step"))
        .unwrap();
    let model = librepcb_step::StepModel::from_step(&std::fs::read(step).unwrap()).unwrap();
    assert!(model.solid_count() > 0);
    let pdf = files
        .iter()
        .filter_map(Value::as_str)
        .find(|file| file.ends_with(".pdf"))
        .unwrap();
    assert!(std::fs::read(pdf).unwrap().starts_with(b"%PDF-"));
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(&ws);
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();

    // The bundled workflow is discoverable before a project is opened.
    assert!(client.peer_info().unwrap().capabilities.resources.is_some());
    let resources = client.list_all_resources().await.unwrap();
    let guide = resources
        .iter()
        .find(|r| r.uri == "librepcb://guides/board-design")
        .unwrap();
    assert_eq!(guide.mime_type.as_deref(), Some("text/markdown"));
    let read = client
        .read_resource(ReadResourceRequestParams::new(&guide.uri))
        .await
        .unwrap();
    let [
        ResourceContents::TextResourceContents {
            uri,
            text,
            mime_type,
            ..
        },
    ] = read.contents.as_slice()
    else {
        panic!("expected a Markdown guide: {read:?}");
    };
    assert_eq!(uri, &guide.uri);
    assert_eq!(mime_type.as_deref(), Some("text/markdown"));
    assert_eq!(guide.size, Some(text.len() as u64));
    assert!(text.contains("symbol_variant") && text.contains("expected_revision"));
    let example = resources
        .iter()
        .find(|resource| resource.uri == "librepcb://examples/esp32-c3-devboard")
        .unwrap();
    assert_eq!(example.mime_type.as_deref(), Some("application/json"));
    let missing = client
        .read_resource(ReadResourceRequestParams::new("librepcb://guides/missing"))
        .await
        .unwrap_err();
    let rmcp::ServiceError::McpError(error) = missing else {
        panic!("expected a resource error: {missing:?}");
    };
    assert_eq!(error.code, rmcp::model::ErrorCode::RESOURCE_NOT_FOUND);

    // Tools.
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in [
        "server_info",
        "workspace_open",
        "workspace_create",
        "project_create",
        "project_open",
        "project_save",
        "project_close",
        "project_summary",
        "library_list",
        "library_rescan",
        "library_install",
        "library_search",
        "library_element",
        "component_list",
        "component_get",
        "net_list",
        "netlist",
        "schematic_list",
        "schematic_get",
        "board_get",
        "erc_run",
        "export_fabrication",
        "export_bom",
        "export_pick_place",
        "export_netlist",
        "jobs_list",
        "render",
        "mutation_apply",
        "mutation_schema",
        // Phase 2.
        "undo",
        "redo",
        "history",
        "component_add",
        "component_remove",
        "component_update",
        "symbol_move",
        "connect",
        "disconnect",
        "net_rename",
        "net_class_set",
        "schematic_add",
        "board_add",
        "board_set_outline",
        "device_place",
        "device_auto_place",
        "trace_add",
        "trace_update",
        "via_add",
        "trace_remove",
        "plane_add",
        "design_rules_set",
        "planes_rebuild",
        "unrouted",
        "autoroute",
        "drc_run",
        "jobs_run",
        "schematic_tidy",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected}: {names:?}"
        );
    }
    for tool in &tools {
        assert!(tool.description.as_ref().is_some_and(|d| !d.is_empty()));
    }
    let search = tools.iter().find(|t| t.name == "library_search").unwrap();
    assert!(
        search
            .input_schema
            .get("properties")
            .unwrap()
            .get("query")
            .is_some()
    );
    let placement = tools.iter().find(|t| t.name == "device_place").unwrap();
    assert!(
        placement.input_schema["properties"]
            .get("include_pads")
            .is_some()
    );

    // Workspace from the command line; add the upstream test library.
    let info = call_ok(&client, "server_info", json!({})).await;
    assert_eq!(info["units"]["length"], "mm");
    let libraries_path = info["workspace"]["libraries_path"]
        .as_str()
        .unwrap()
        .to_owned();
    crate::common::install_populated_library(Path::new(&libraries_path));
    let scan = call_ok(&client, "library_rescan", json!({})).await;
    assert_eq!(scan["library_count"], 1);
    let found = call_ok(
        &client,
        "library_search",
        json!({ "query": "Diode", "kind": "component" }),
    )
    .await;
    let elements = found["elements"].as_array().unwrap();
    assert!(!elements.is_empty());
    let diode = elements[0]["uuid"].as_str().unwrap().to_owned();
    let detail = call_ok(&client, "library_element", json!({ "uuid": diode })).await;
    assert_eq!(detail["kind"], "component");
    let signals: Vec<&str> = detail["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(signals, ["A", "C"]);

    // New project in the workspace.
    let created = call_ok(&client, "project_create", json!({ "name": "E2E Test" })).await;
    assert_eq!(created["boards"][0]["name"], "default");
    let lpp = created["path"].as_str().unwrap().to_owned();
    assert!(Path::new(&lpp).is_file());
    call_ok(&client, "project_close", json!({})).await;

    // Upstream test project: read, ERC, Gerber, render.
    let project = crate::common::copy_test_project("Nested Planes", tmp.path());
    call_ok(
        &client,
        "project_open",
        json!({ "path": project.to_string_lossy() }),
    )
    .await;
    let summary = call_ok(&client, "project_summary", json!({})).await;
    assert_eq!(summary["counts"]["components"], 9);
    let component = call_ok(&client, "component_get", json!({ "component": "R1" })).await;
    assert_eq!(component["designator"], "R1");

    let erc = call_ok(&client, "erc_run", json!({})).await;
    assert_eq!(erc["ran"], true);

    let gerber_dir = tmp.path().join("gerber");
    let fab = call_ok(
        &client,
        "export_fabrication",
        json!({ "output_dir": gerber_dir.to_string_lossy() }),
    )
    .await;
    let files = fab["files"].as_array().unwrap();
    assert!(files.iter().any(|f| f.as_str().unwrap().ends_with(".gbr")));
    assert!(
        files
            .iter()
            .all(|f| Path::new(f.as_str().unwrap()).is_file())
    );

    let res = call(
        &client,
        "render",
        json!({ "target": "board", "width": 320, "height": 240 }),
    )
    .await;
    assert_ne!(res.is_error, Some(true));
    let image = res
        .content
        .iter()
        .find_map(|c| match c {
            ContentBlock::Image(img) => Some(img.clone()),
            _ => None,
        })
        .expect("image content");
    assert_eq!(image.mime_type, "image/png");
    assert!(!image.data.is_empty());

    // Errors are tool results with a stable kind.
    let res = call(&client, "component_get", json!({ "component": "X99" })).await;
    assert_eq!(res.is_error, Some(true));
    let envelope = res.structured_content.unwrap();
    assert_eq!(envelope["outcome"], "failed");
    assert_eq!(envelope["error"]["kind"], "not_found");
    // Revision of the open project (the plane rebuild of the export
    // journals derived data changes).
    assert!(envelope["revision"].is_u64());

    call_ok(&client, "project_close", json!({})).await;
    let res = call(&client, "project_summary", json!({})).await;
    assert_eq!(
        res.structured_content.unwrap()["error"]["kind"],
        "no_project"
    );

    client.cancel().await.unwrap();
}

/// Links the libraries (`*.lplib`) of `LIBREPCB_TEST_LIBRARIES_DIR` into
/// the local libraries of a workspace; `false` if the variable is not set
/// or the platform has no symbolic links.
fn link_test_libraries(libraries_path: &Path) -> bool {
    let Some(source) = std::env::var_os("LIBREPCB_TEST_LIBRARIES_DIR") else {
        return false;
    };
    if !cfg!(unix) {
        return false;
    }
    let local = libraries_path.join("local");
    std::fs::create_dir_all(&local).unwrap();
    let mut count = 0;
    for entry in std::fs::read_dir(source).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "lplib") {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&path, local.join(path.file_name().unwrap())).unwrap();
            count += 1;
        }
    }
    count > 0
}

/// Replays the published dev-board schematic from a fresh project using
/// the official library cache, with an optional independent upstream ERC.
#[tokio::test]
async fn stdio_esp32_c3_schematic_acceptance() {
    let tmp = tempfile::tempdir().unwrap();
    let cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let workspace = call_ok(
        &client,
        "workspace_create",
        json!({"path":tmp.path().join("ws")}),
    )
    .await;
    if !link_test_libraries(Path::new(workspace["libraries_path"].as_str().unwrap())) {
        eprintln!("LIBREPCB_TEST_LIBRARIES_DIR not set, skipping ESP32-C3 acceptance replay");
        client.cancel().await.unwrap();
        return;
    }
    call_ok(&client, "library_rescan", json!({})).await;
    let project = call_ok(
        &client,
        "project_create",
        json!({"name":"ESP32-C3 acceptance","create_schematic":false,"create_board":false}),
    )
    .await;
    let example = client
        .read_resource(ReadResourceRequestParams::new(
            "librepcb://examples/esp32-c3-devboard",
        ))
        .await
        .unwrap();
    let [
        ResourceContents::TextResourceContents {
            text, mime_type, ..
        },
    ] = example.contents.as_slice()
    else {
        panic!("expected one JSON resource: {example:?}");
    };
    assert_eq!(mime_type.as_deref(), Some("application/json"));
    let document: Value = serde_json::from_str(text).unwrap();
    let empty = call_ok(&client, "schematic_list", json!({})).await;
    assert_eq!(empty["schematics"], json!([]));
    let mut invalid = document.clone();
    invalid["no_connects"]["U1.EN"] = json!(true);
    let rejected = call(&client, "design_preview", invalid).await;
    assert_eq!(
        rejected.structured_content.unwrap()["error"]["kind"],
        "conflict"
    );
    assert_eq!(call_ok(&client, "schematic_list", json!({})).await, empty);
    let preview = call_ok(&client, "design_preview", document.clone()).await;
    assert_eq!(
        preview["erc"],
        json!({"introduced":[],"inherited":[],"resolved":[]})
    );
    assert_eq!(preview["diff"][0]["field"], "add_schematic");
    assert_eq!(call_ok(&client, "schematic_list", json!({})).await, empty);
    call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    let page = call_ok(&client, "schematic_get", json!({})).await;
    assert_eq!(page["uuid"], preview["schematic"]);
    assert_eq!(page["symbols"].as_array().unwrap().len(), 23);
    assert_eq!(page["net_segments"].as_array().unwrap().len(), 27);
    assert_eq!(page["texts"].as_array().unwrap().len(), 13);
    let marked_pins = |page: &Value| {
        page["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|symbol| symbol["pins"].as_array().unwrap())
            .filter(|pin| pin["no_connect"] == true)
            .map(|pin| {
                assert!(pin["net"].is_null());
                pin["pin"].as_str().unwrap().to_owned()
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    let expected_marks: std::collections::BTreeSet<_> = [
        "U1.IO0/XTAL_32K_P",
        "U1.IO1/XTAL_32K_N",
        "U1.IO7/MTDO",
        "U1.IO18/USB_D-",
        "U1.IO19/USB_D+",
    ]
    .map(str::to_owned)
    .into();
    assert_eq!(marked_pins(&page), expected_marks);
    let netlist = call_ok(&client, "netlist", json!({})).await;
    assert_eq!(netlist["nets"].as_object().unwrap().len(), 14);
    assert_eq!(
        netlist["nets"]["EN"],
        json!(["C3.2", "R1.1", "S1.2", "U1.EN"])
    );
    assert_eq!(netlist["nets"]["GPIO9"], json!(["R2.1", "S2.2", "U1.IO9"]));
    let erc = call_ok(&client, "erc_run", json!({})).await;
    assert_eq!(erc["messages"], json!([]));

    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_list", json!({})).await, empty);
    call_ok(&client, "redo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, page);
    let conflicting = call(
        &client,
        "design_preview",
        json!({"version":1,"schematic":"Main",
        "components":{"module":{"designator":"U1"}},"no_connects":{"module.EN":true}}),
    )
    .await;
    assert_eq!(
        conflicting.structured_content.unwrap()["error"]["kind"],
        "conflict"
    );
    let clear = call_ok(
        &client,
        "design_preview",
        json!({"version":1,"schematic":"Main",
        "components":{"module":{"designator":"U1"}},"no_connects":{"module.IO18/USB_D-":false}}),
    )
    .await;
    assert_eq!(clear["diff"][0]["field"], "no_connect");
    call_ok(&client, "design_apply", json!({"preview":clear["preview"]})).await;
    assert_eq!(
        marked_pins(&call_ok(&client, "schematic_get", json!({})).await).len(),
        4
    );
    assert_eq!(call_ok(&client, "netlist", json!({})).await, netlist);
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, page);
    call_ok(
        &client,
        "symbol_text_move",
        json!({"component":"C3","text":"name","position":{"x":15.24,"y":29.45}}),
    )
    .await;
    let hand_edited = call_ok(&client, "schematic_get", json!({})).await;
    assert_ne!(hand_edited, page);
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    let path = project["path"].as_str().unwrap();
    if let Some(cli) = crate::common::upstream_cli() {
        let (ok, text) = crate::common::run_cli(&cli, &["open-project", path, "--erc"]);
        assert!(ok && text.contains("Non-approved messages: 0"), "{text}");
    }
    call_ok(&client, "project_open", json!({"path":path})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let replay = call_ok(&client, "design_preview", document).await;
    assert_eq!(replay["diff"], json!([]));
    assert_eq!(replay["schematic"], page["uuid"]);
    assert_eq!(replay["erc"]["introduced"], json!([]));
    let applied = call_ok(
        &client,
        "design_apply",
        json!({"preview":replay["preview"]}),
    )
    .await;
    assert_eq!(applied["operation_count"], 0);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    assert_eq!(
        call_ok(&client, "schematic_get", json!({})).await,
        hand_edited
    );
    assert_eq!(call_ok(&client, "netlist", json!({})).await, netlist);
    client.cancel().await.unwrap();
}

/// The full agent workflow over stdio with the official libraries (LibrePCB
/// Base and Connectors, from `LIBREPCB_TEST_LIBRARIES_DIR`; skipped if not
/// set): a 2-pin connector powering a LED with a series resistor, supply
/// symbols, board outline, automatic placement, autorouting, GND plane,
/// ERC/DRC, Gerber export, save, and the official `librepcb-cli` opens the
/// result without errors.
#[tokio::test(flavor = "multi_thread")]
async fn stdio_design_led_board_with_official_libraries() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let instructions = client
        .peer_info()
        .and_then(|i| i.instructions.clone())
        .unwrap_or_default();
    assert!(instructions.contains("component_add"), "{instructions}");

    let created = call_ok(
        &client,
        "workspace_create",
        json!({ "path": ws.to_string_lossy() }),
    )
    .await;
    let libraries_path = created["libraries_path"].as_str().unwrap().to_owned();
    if !link_test_libraries(Path::new(&libraries_path)) {
        eprintln!("LIBREPCB_TEST_LIBRARIES_DIR not set, skipping");
        client.cancel().await.unwrap();
        return;
    }
    let scan = call_ok(&client, "library_rescan", json!({})).await;
    assert!(scan["library_count"].as_u64().unwrap() >= 2, "{scan}");
    for (query, kind) in [
        ("LED", "device"),
        ("Pin Header", "device"),
        ("GND", "component"),
    ] {
        let found = call_ok(
            &client,
            "library_search",
            json!({ "query": query, "kind": kind, "limit": 5 }),
        )
        .await;
        assert!(!found["elements"].as_array().unwrap().is_empty(), "{query}");
    }

    call_ok(&client, "project_create", json!({ "name": "LED Board" })).await;
    let mut designators = Vec::new();
    for part in [
        "Generic Pin Header 2.54mm 1x02 ⌀1.0mm",
        "Resistor 2012 (0805)",
        "LED ⌀3.0x4.5/2.54mm Red",
        "Supply VCC",
        "Supply GND",
    ] {
        let added = call_ok(&client, "component_add", json!({ "part": part })).await;
        designators.push(added["designator"].as_str().unwrap().to_owned());
    }
    assert_eq!(designators, ["J1", "R1", "LED1", "VCC1", "GND1"]);
    call_ok(
        &client,
        "component_update",
        json!({ "component": "R1", "value": "330R" }),
    )
    .await;
    let summary = call_ok(&client, "project_summary", json!({})).await;
    let revision = summary["revision"].as_u64().unwrap();
    let vcc = call_ok(
        &client,
        "connect",
        json!({
            "net": "VCC",
            "pins": ["J1.1", "R1.1", "VCC1.Net"],
            "expected_revision": revision,
        }),
    )
    .await;
    assert_eq!(vcc["name"], "VCC");
    call_ok(
        &client,
        "connect",
        json!({ "net": "LED_A", "pins": ["R1.2", "LED1.A"] }),
    )
    .await;
    call_ok(
        &client,
        "connect",
        json!({ "pins": ["LED1.C", "J1.2", "GND1.Net"] }),
    )
    .await;
    let netlist = call_ok(&client, "netlist", json!({})).await;
    assert_eq!(
        netlist["nets"],
        json!({
            "GND": ["GND1.Net", "J1.2", "LED1.C"],
            "LED_A": ["LED1.A", "R1.2"],
            "VCC": ["J1.1", "R1.1", "VCC1.Net"],
        })
    );
    let erc = call_ok(&client, "erc_run", json!({})).await;
    assert_eq!(erc["errors"], 0, "{erc:#}");
    assert_eq!(erc["warnings"], 0, "{erc:#}");

    // Undo and redo the last connection.
    let undone = call_ok(&client, "undo", json!({})).await;
    assert_eq!(undone["done"][0], "AI: connect");
    let netlist = call_ok(&client, "netlist", json!({})).await;
    assert!(netlist["nets"].get("GND").is_none());
    call_ok(&client, "redo", json!({})).await;

    // Board.
    call_ok(
        &client,
        "board_set_outline",
        json!({ "width": 30.0, "height": 20.0, "corner_radius": 1.0 }),
    )
    .await;
    let placed = call_ok(&client, "device_auto_place", json!({})).await;
    assert_eq!(placed["placed"].as_array().unwrap().len(), 3, "{placed:#}");
    let routed = call(&client, "autoroute", json!({})).await;
    let envelope = routed.structured_content.clone().unwrap();
    assert_ne!(routed.is_error, Some(true), "{envelope:#}");
    eprintln!(
        "autoroute: {} ({}): {:?}",
        envelope["result"]["backend"], envelope["outcome"], envelope["warnings"]
    );
    let unrouted = call_ok(&client, "unrouted", json!({})).await;
    assert_eq!(unrouted["count"], 0, "{unrouted:#}");
    call_ok(
        &client,
        "plane_add",
        json!({ "net": "GND", "layer": "bottom" }),
    )
    .await;
    let drc = call(&client, "drc_run", json!({})).await;
    let drc = drc.structured_content.unwrap()["result"].clone();
    eprintln!("DRC: {drc:#}");
    assert_eq!(drc["ran"], true);
    assert_eq!(drc["errors"], 0, "{drc:#}");
    let erc = call_ok(&client, "erc_run", json!({})).await;
    assert_eq!(erc["errors"], 0, "{erc:#}");
    let fab = call_ok(&client, "export_fabrication", json!({})).await;
    assert!(fab["files"].as_array().unwrap().len() >= 8);
    let saved = call_ok(&client, "project_save", json!({})).await;
    let lpp = saved["path"].as_str().unwrap().to_owned();
    let history = call_ok(&client, "history", json!({})).await;
    assert_eq!(history["unsaved_changes"], false);
    if let Some(out) = std::env::var_os("LIBREPCB_MCP_TEST_OUTPUT") {
        for (target, name) in [
            ("schematic", "led_schematic.png"),
            ("board", "led_board.png"),
        ] {
            let res = call(
                &client,
                "render",
                json!({ "target": target, "width": 1200, "height": 900 }),
            )
            .await;
            if let Some(ContentBlock::Image(img)) = res
                .content
                .iter()
                .find(|c| matches!(c, ContentBlock::Image(_)))
            {
                use base64::Engine;
                let png = base64::engine::general_purpose::STANDARD
                    .decode(&img.data)
                    .unwrap();
                std::fs::create_dir_all(&out).unwrap();
                std::fs::write(Path::new(&out).join(name), png).unwrap();
            }
        }
    }
    call_ok(&client, "project_close", json!({})).await;
    client.cancel().await.unwrap();

    match crate::common::upstream_cli() {
        Some(cli) => {
            let (ok, text) = crate::common::run_cli(
                &cli,
                &[
                    "open-project",
                    "--erc",
                    "--drc",
                    "--export-pcb-fabrication-data",
                    &lpp,
                ],
            );
            eprintln!("{text}");
            // Non-approved DRC warnings make the CLI exit with an error code.
            assert!(ok || text.contains("Finished with errors!"), "{text}");
            assert!(
                !text.contains("[ERROR]") && !text.contains("ERROR:"),
                "{text}"
            );
            assert!(text.contains("COPPER-BOTTOM.gbr"), "{text}");
        }
        None => eprintln!("librepcb-cli not found, skipping the upstream check"),
    }
}

/// Unknown arguments are rejected on every tool with `invalid_argument`
/// (naming the key and the valid ones) before anything runs.
#[tokio::test(flavor = "multi_thread")]
async fn stdio_unknown_arguments_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(&ws);
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();

    let tools = client.list_all_tools().await.unwrap();
    assert!(tools.len() > 40);
    for tool in &tools {
        let res = call(&client, &tool.name, json!({ "bogus_argument": 1 })).await;
        assert_eq!(res.is_error, Some(true), "{}", tool.name);
        let envelope = res.structured_content.unwrap();
        assert_eq!(
            envelope["error"]["kind"], "invalid_argument",
            "{}: {envelope:#}",
            tool.name
        );
        let message = envelope["error"]["message"].as_str().unwrap();
        assert!(message.contains("\"bogus_argument\""), "{message}");
        assert!(message.contains("valid arguments:"), "{message}");
        let props = tool
            .input_schema
            .get("properties")
            .and_then(|p| p.as_object());
        for key in props.into_iter().flat_map(|p| p.keys()) {
            assert!(message.contains(key.as_str()), "{}: {message}", tool.name);
        }
    }

    // The case from practice: `path` instead of `directory`.
    let res = call(
        &client,
        "project_create",
        json!({ "name": "X", "path": tmp.path().join("x").to_string_lossy() }),
    )
    .await;
    let envelope = res.structured_content.unwrap();
    assert_eq!(envelope["error"]["kind"], "invalid_argument");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("\"path\"") && message.contains("directory"),
        "{message}"
    );
    assert!(!tmp.path().join("x").exists());

    // Unknown keys of nested objects and wrong types are tool errors too.
    let res = call(
        &client,
        "board_set_outline",
        json!({ "width": 10, "height": 10, "origin": { "x": 0, "y": 0, "z": 1 } }),
    )
    .await;
    let envelope = res.structured_content.unwrap();
    assert_eq!(
        envelope["error"]["kind"], "invalid_argument",
        "{envelope:#}"
    );
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(message.contains("`z`"), "{message}");
    let res = call(&client, "library_search", json!({ "query": 5 })).await;
    let envelope = res.structured_content.unwrap();
    assert_eq!(
        envelope["error"]["kind"], "invalid_argument",
        "{envelope:#}"
    );

    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn stdio_reviewed_builtin_routing_save_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    for name in [
        "routing_prepare",
        "routing_staged_prepare",
        "routing_preview",
        "routing_apply",
        "routing_discard",
    ] {
        assert!(tools.iter().any(|t| t.name == name), "missing {name}");
    }
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let created = call_ok(&client, "project_create", json!({"name":"Reviewed"})).await;
    for (name, x) in [("R1", 5), ("R2", 15)] {
        call_ok(
            &client,
            "component_add",
            json!({"part":"R-0805","designator":name}),
        )
        .await;
        call_ok(
            &client,
            "device_place",
            json!({"component":name,"position":{"x":x,"y":10},"rotation":90}),
        )
        .await;
    }
    for (net, pins) in [("A", ["R1.1", "R2.1"]), ("B", ["R1.2", "R2.2"])] {
        call_ok(&client, "connect", json!({"net":net,"pins":pins})).await;
    }
    call_ok(
        &client,
        "board_set_outline",
        json!({"width":30,"height":20}),
    )
    .await;
    let before = call(&client, "board_get", json!({}))
        .await
        .structured_content
        .unwrap();
    let prepared = call_ok(
        &client,
        "routing_staged_prepare",
        json!({
            "options":{"nets":["A","B"],"expected_revision":before["revision"]},
            "batch_size":1,
        }),
    )
    .await;
    assert_eq!(prepared["validation"]["acceptable"], true);
    assert_eq!(prepared["validation"]["after"]["unrouted"], 0);
    let trials = prepared["staged"]["trials"].as_array().unwrap();
    assert_eq!(trials.len(), 2);
    assert!(trials.iter().all(|trial| trial["outcome"] == "retained"));
    let candidate = &prepared["candidate"];
    let preview = call_ok(&client, "routing_preview", json!({"candidate":candidate})).await;
    assert!(!preview["mutations"].as_array().unwrap().is_empty());
    let history = call_ok(&client, "history", json!({})).await;
    let rejected = call(
        &client,
        "mutation_apply",
        json!({"expected_revision":before["revision"], "mutation":{"Batch":[
            {"SetProjectSettings":librepcb_core::project::ProjectSettings::default()},
            {"RemoveBoard":librepcb_core::types::Uuid::new_random()}
        ]}}),
    )
    .await;
    let failure = rejected.structured_content.unwrap();
    assert_eq!(failure["outcome"], "failed");
    assert_eq!(failure["revision"], before["revision"]);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    let warm = call_ok(
        &client,
        "routing_prepare",
        json!({"expected_revision":before["revision"]}),
    )
    .await;
    assert_eq!(warm["baseline_reused"], true);
    call_ok(
        &client,
        "routing_discard",
        json!({"candidate":warm["candidate"]}),
    )
    .await;
    let unchanged = call(&client, "board_get", json!({}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(unchanged, before);
    let applied = call_ok(
        &client,
        "routing_apply",
        json!({"candidate":candidate,"expected_revision":before["revision"]}),
    )
    .await;
    assert_eq!(applied["applied"], true);
    assert_eq!(applied["consumed"], true);
    assert_eq!(applied["validation"]["after"]["unrouted"], 0);
    let consumed = call(&client, "routing_apply", json!({"candidate":candidate})).await;
    assert_eq!(
        consumed.structured_content.unwrap()["error"]["kind"],
        "not_found"
    );
    call_ok(&client, "undo", json!({})).await;
    call_ok(&client, "redo", json!({})).await;
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({"path":created["path"]})).await;
    let validation = call_ok(&client, "drc_run", json!({})).await;
    assert_eq!(validation["errors"], 0, "{validation:#}");
    call_ok(&client, "project_close", json!({})).await;
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_design_preview_preserves_manual_placement() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let project = call_ok(&client, "project_create", json!({"name":"Reset update"})).await;
    call_ok(
        &client,
        "component_add",
        json!({"part":"R-0805","designator":"R1","value":"10k"}),
    )
    .await;
    call_ok(&client, "connect", json!({"net":"EN","pins":["R1.2"]})).await;
    let detail = call_ok(&client, "component_get", json!({"component":"R1"})).await;
    let document = json!({
        "version":1,"schematic":"Main",
        "components":{"pullup":{"uuid":detail["uuid"],"value":{"expected":"10k","set":"10 kΩ"}}},
        "nets":{"EN":["pullup.2"]}
    });
    let history = call_ok(&client, "history", json!({})).await;
    let response = call(&client, "design_preview", document.clone()).await;
    assert!(
        response
            .content
            .iter()
            .any(|item| matches!(item, ContentBlock::Image(_)))
    );
    let preview = response.structured_content.unwrap()["result"].clone();
    assert_eq!(preview["diff"].as_array().unwrap().len(), 1);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    let moved = json!({"x":40.64,"y":25.4});
    call_ok(
        &client,
        "symbol_move",
        json!({"component":"R1","position":moved}),
    )
    .await;
    let rejected = call(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    assert_eq!(
        rejected.structured_content.unwrap()["error"]["kind"],
        "stale_revision"
    );

    let preview = call_ok(&client, "design_preview", document.clone()).await;
    let applied = call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    assert_eq!(applied["mutation_count"], 1);
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({"path":project["path"]})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let preview = call_ok(&client, "design_preview", document.clone()).await;
    assert_eq!(preview["diff"], json!([]));
    let applied = call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    assert_eq!(applied["mutation_count"], 0);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    let after = call_ok(&client, "component_get", json!({"component":"R1"})).await;
    assert_eq!(after["uuid"], detail["uuid"]);
    assert_eq!(after["symbols"][0]["position"], moved);
    assert_eq!(after["value"], "10 kΩ");

    call_ok(
        &client,
        "component_update",
        json!({"component":"R1","value":"12k"}),
    )
    .await;
    let conflict = call(&client, "design_preview", document).await;
    assert_eq!(
        conflict.structured_content.unwrap()["error"]["kind"],
        "conflict"
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_design_wire_group_keeps_other_connections() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    call_ok(&client, "project_create", json!({"name":"Local wires"})).await;
    let mut bindings = json!({});
    for (name, x, y) in [("R1", 0, 0), ("R2", 25, 0), ("R3", 0, 25)] {
        call_ok(
            &client,
            "component_add",
            json!({"part":"R-0805","designator":name}),
        )
        .await;
        call_ok(
            &client,
            "symbol_move",
            json!({"component":name,"position":{"x":x,"y":y}}),
        )
        .await;
        let component = call_ok(&client, "component_get", json!({"component":name})).await;
        bindings[name] = json!({"uuid":component["uuid"]});
    }
    call_ok(
        &client,
        "connect",
        json!({
            "net":"EN","pins":["R1.2","R2.1","R3.2"],"max_wire_length":0,
        }),
    )
    .await;
    let document = json!({
        "version":1,"schematic":"Main","components":bindings,
        "wire_groups":[{"net":"EN","pins":["R1.2","R2.1"]}],
    });
    let before = call_ok(&client, "schematic_get", json!({})).await;
    let netlist = call_ok(&client, "netlist", json!({})).await;
    let preview = call_ok(&client, "design_preview", document.clone()).await;
    assert_eq!(preview["diff"].as_array().unwrap().len(), 1);
    assert_eq!(preview["diff"][0]["field"], "wire_group");
    assert_eq!(preview["diff"][0]["net"], "EN");
    assert_eq!(preview["diff"][0]["pins"], json!(["R1.2", "R2.1"]));
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, before);
    call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    let after = call_ok(&client, "schematic_get", json!({})).await;
    let original_segments = before["net_segments"].as_array().unwrap();
    let new_segments = after["net_segments"].as_array().unwrap();
    assert_eq!(original_segments.len(), 3);
    assert_eq!(new_segments.len(), 2);
    let replaced: Vec<_> = original_segments
        .iter()
        .filter(|segment| !new_segments.contains(segment))
        .map(|segment| segment["uuid"].clone())
        .collect();
    assert_eq!(preview["diff"][0]["replaced_segments"], json!(replaced));
    assert_eq!(
        new_segments
            .iter()
            .filter(|s| original_segments.contains(s))
            .count(),
        1
    );
    assert_eq!(call_ok(&client, "netlist", json!({})).await, netlist);
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, before);
    call_ok(&client, "redo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, after);

    let mut unsafe_group = document.clone();
    unsafe_group["wire_groups"][0]["pins"] = json!(["R1.2", "R3.2"]);
    let denied = call(&client, "design_preview", unsafe_group).await;
    assert!(denied.is_error.unwrap_or(false));
    assert!(
        denied
            .structured_content
            .unwrap()
            .to_string()
            .contains("connections outside the group")
    );
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, after);

    let position = json!({"x":27.94,"y":0});
    call_ok(
        &client,
        "symbol_move",
        json!({"component":"R2","position":position}),
    )
    .await;
    let manual = call_ok(&client, "schematic_get", json!({})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let repeat = call_ok(&client, "design_preview", document).await;
    assert_eq!(repeat["diff"], json!([]));
    let applied = call_ok(
        &client,
        "design_apply",
        json!({"preview":repeat["preview"]}),
    )
    .await;
    assert_eq!(applied["mutation_count"], 0);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, manual);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_explicit_wire_paths_validate_and_replay() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let project = call_ok(&client, "project_create", json!({"name":"Wire paths"})).await;
    let mut bindings = json!({});
    for (name, x, y) in [("R1", 0, 0), ("R2", 40, 0), ("R3", 20, 20), ("R4", 0, -20)] {
        let added = call_ok(
            &client,
            "component_add",
            json!({"part":"R-0805","designator":name}),
        )
        .await;
        bindings[name] = json!({"uuid":added["uuid"]});
        call_ok(
            &client,
            "symbol_move",
            json!({"component":name,"position":{"x":x,"y":y}}),
        )
        .await;
    }
    for (net, pins, assign) in [
        ("EN", json!(["R1.2", "R2.1", "R3.1"]), true),
        ("GND", json!(["R4.2"]), true),
        ("TEST", json!(["R4.1"]), false),
    ] {
        let result = call_ok(
            &client,
            "connect",
            json!({"net":net,"pins":pins,"assign_only":assign}),
        )
        .await;
        if assign {
            assert_eq!(result["wires"], 0);
            assert_eq!(result["labels"], 0);
        }
    }
    let before = call_ok(&client, "schematic_get", json!({})).await;
    let pin = |name: &str| {
        before["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["pins"].as_array().unwrap())
            .find(|pin| pin["pin"] == name)
            .unwrap()["position"]
            .clone()
    };
    let r4 = pin("R4.2");
    let document = json!({
        "version":1,"schematic":"Main","components":bindings,
        "wire_groups":[
            {"net":"EN","pins":["R1.2","R2.1","R3.1"],
             "paths":[["R1.2","R2.1"],["R3.1",{"x":{"pin":"R3.1"},"y":{"pin":"R2.1"}}]]},
            {"net":"GND","pins":["R4.2"],"paths":[["R4.2",{"x":20,"y":r4["y"]}]],
             "label":{"x":20,"y":r4["y"]}}
        ],
        "notes":[{"text":"RESET","position":{"x":20,"y":30},"height":1.5}]
    });
    let summary = call_ok(&client, "project_summary", json!({})).await;
    let netlist = call_ok(&client, "netlist", json!({})).await;
    let history = call_ok(&client, "history", json!({})).await;

    for (paths, diagnostic) in [
        (json!([["R4.2",{"x":20,"y":-10}]]), "horizontal or vertical"),
        (
            json!([["R4.2",{"x":20,"y":r4["y"]},{"x":20,"y":10}]]),
            "crosses or overlaps",
        ),
        (
            json!([["R4.2",{"x":-2,"y":r4["y"]}]]),
            "passes through symbol R4",
        ),
    ] {
        let mut invalid = document.clone();
        invalid["wire_groups"][1]["paths"] = paths;
        invalid["wire_groups"][1]
            .as_object_mut()
            .unwrap()
            .remove("label");
        let result = call(&client, "design_preview", invalid).await;
        assert_eq!(result.is_error, Some(true));
        let error = result.structured_content.unwrap();
        assert!(error.to_string().contains(diagnostic), "{error}");
    }
    let preview = call_ok(&client, "design_preview", document.clone()).await;
    assert_eq!(preview["diff"].as_array().unwrap().len(), 3);
    assert_eq!(preview["diff"][0]["field"], "wire_paths");
    assert_eq!(
        call_ok(&client, "project_summary", json!({})).await,
        summary
    );
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, before);
    call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    let after = call_ok(&client, "schematic_get", json!({})).await;
    let mut expected_symbols = before["symbols"].clone();
    for symbol in expected_symbols.as_array_mut().unwrap() {
        for pin in symbol["pins"].as_array_mut().unwrap() {
            if matches!(pin["net"].as_str(), Some("EN" | "GND")) {
                pin["wired"] = json!(true);
            }
        }
    }
    assert_eq!(after["symbols"], expected_symbols);
    let segments = after["net_segments"].as_array().unwrap();
    assert!(segments.contains(&before["net_segments"][0]));
    let en = segments.iter().find(|s| s["net"] == "EN").unwrap();
    assert_eq!(en["junctions"].as_array().unwrap().len(), 1);
    assert_eq!(en["lines"].as_array().unwrap().len(), 3);
    let wire_text = call_ok(
        &client,
        "design_preview",
        json!({"version":1,"schematic":"Main","components":document["components"],
            "review_components":["R1"],
            "notes":[{"text":"DO NOT CROSS","position":{"x":15,"y":0},"height":3.0}]}),
    )
    .await;
    assert!(
        wire_text["annotation_changes"]["introduced"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["first"]["kind"] == "schematic_text"
                    && finding["second"]["kind"] == "wire"
                    && finding["second"]["net"] == "EN"
            }),
        "{wire_text}"
    );
    assert_eq!(call_ok(&client, "netlist", json!({})).await, netlist);
    let mut unsafe_edit = document.clone();
    unsafe_edit["wire_groups"][0]["pins"] = json!(["R1.2", "R2.1"]);
    unsafe_edit["wire_groups"][0]["paths"] = json!([["R1.2", "R2.1"]]);
    let denied = call(&client, "design_preview", unsafe_edit).await;
    assert!(
        denied
            .structured_content
            .unwrap()
            .to_string()
            .contains("connections outside the group")
    );

    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, before);
    call_ok(&client, "redo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, after);
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({"path":project["path"]})).await;
    let repeated = call_ok(&client, "design_preview", document.clone()).await;
    assert_eq!(repeated["diff"], json!([]));
    let applied = call_ok(
        &client,
        "design_apply",
        json!({"preview":repeated["preview"]}),
    )
    .await;
    assert_eq!(applied["operation_count"], 0);
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, after);

    let mut move_branch = document;
    move_branch["components"]["R3"] = json!({"designator":"R3"});
    move_branch["block"] = json!({"members":["R3"],"direction":"row","gap":2,
        "min":{"x":21,"y":15},"max":{"x":37,"y":28},"baseline":20});
    let preview = call_ok(&client, "design_preview", move_branch.clone()).await;
    assert_eq!(preview["diff"].as_array().unwrap().len(), 2);
    assert_eq!(preview["diff"][0]["field"], "block_position");
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, after);
    call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    let moved = call_ok(&client, "schematic_get", json!({})).await;
    for symbol in after["symbols"].as_array().unwrap() {
        if symbol["name"] != "R3" {
            assert!(moved["symbols"].as_array().unwrap().contains(symbol));
        }
    }
    for segment in segments.iter().filter(|s| s["net"] != "EN") {
        assert!(moved["net_segments"].as_array().unwrap().contains(segment));
    }
    assert_eq!(call_ok(&client, "netlist", json!({})).await, netlist);
    let repeat = call_ok(&client, "design_preview", move_branch).await;
    assert_eq!(repeat["diff"], json!([]));
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, after);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_design_block_moves_only_selected_symbols_and_stubs() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let project = call_ok(&client, "project_create", json!({"name":"Bounded layout"})).await;
    let mut bindings = json!({});
    for (name, x, y) in [("R1", 0.0, 0.0), ("R2", 25.4, 0.0), ("R3", 0.0, 50.8)] {
        call_ok(
            &client,
            "component_add",
            json!({"part":"R-0805","designator":name}),
        )
        .await;
        call_ok(
            &client,
            "symbol_move",
            json!({"component":name,"position":{"x":x,"y":y}}),
        )
        .await;
        let component = call_ok(&client, "component_get", json!({"component":name})).await;
        bindings[name] = json!({"uuid":component["uuid"]});
    }
    call_ok(
        &client,
        "connect",
        json!({"net":"EN","pins":["R1.2","R2.1","R3.2"],"max_wire_length":0}),
    )
    .await;
    let before = call_ok(&client, "schematic_get", json!({})).await;
    let netlist = call_ok(&client, "netlist", json!({})).await;
    let board = call_ok(&client, "board_get", json!({})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let summary = call_ok(&client, "project_summary", json!({})).await;
    let document = json!({
        "version":1,"schematic":"Main","components":bindings,
        "block":{"members":["R2","R1"],"direction":"row","gap":5,
            "min":{"x":-10,"y":40},"max":{"x":100,"y":65},"baseline":50.8},
    });
    let mut invalid = document.clone();
    invalid["block"]["max"]["x"] = json!(1);
    assert!(
        call(&client, "design_preview", invalid)
            .await
            .is_error
            .unwrap_or(false)
    );
    let mut column = document.clone();
    column["block"]["direction"] = json!("column");
    column["block"]["min"] = json!({"x":35,"y":25});
    column["block"]["max"] = json!({"x":75,"y":65});
    column["block"]["baseline"] = json!(50.8);
    let preview = call_ok(&client, "design_preview", column).await;
    let diff = preview["diff"].as_array().unwrap();
    assert_eq!(diff.len(), 2);
    assert!(diff.iter().all(|d| d["after"]["x"] == 50.8));
    assert!(diff[0]["after"]["y"].as_f64().unwrap() > diff[1]["after"]["y"].as_f64().unwrap());

    let reply = call(&client, "design_preview", document.clone()).await;
    assert!(reply.content.iter().any(|item| item.as_image().is_some()));
    let preview = &reply.structured_content.unwrap()["result"];
    let diff = preview["diff"].as_array().unwrap();
    assert_eq!(diff.len(), 2);
    assert!(
        diff.iter()
            .all(|d| d["field"] == "block_position" && d["after"]["y"] == 50.8)
    );
    assert!(diff[0]["after"]["x"].as_f64().unwrap() > 0.0);
    assert!(diff[0]["after"]["x"].as_f64().unwrap() < diff[1]["after"]["x"].as_f64().unwrap());
    assert_eq!(
        call_ok(&client, "project_summary", json!({})).await,
        summary
    );
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, before);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    let after = call_ok(&client, "schematic_get", json!({})).await;
    let original_segments = before["net_segments"].as_array().unwrap();
    let new_segments = after["net_segments"].as_array().unwrap();
    assert_eq!(original_segments.len(), 3);
    assert_eq!(new_segments.len(), original_segments.len());
    for segment in original_segments {
        let new = new_segments
            .iter()
            .find(|s| s["uuid"] == segment["uuid"])
            .unwrap();
        let line = &segment["lines"][0];
        let pin = line["from"]["pin"]
            .as_str()
            .or(line["to"]["pin"].as_str())
            .unwrap();
        if pin.starts_with("R3.") {
            assert_eq!(new, segment);
        } else {
            assert_ne!(
                new["junctions"][0]["position"],
                segment["junctions"][0]["position"]
            );
            assert_eq!(new["lines"][0]["uuid"], line["uuid"]);
            assert_eq!(new["labels"][0]["uuid"], segment["labels"][0]["uuid"]);
        }
    }
    let fixed = before["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "R3")
        .unwrap();
    assert!(after["symbols"].as_array().unwrap().contains(fixed));
    assert_eq!(call_ok(&client, "netlist", json!({})).await, netlist);
    assert_eq!(call_ok(&client, "board_get", json!({})).await, board);
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, before);
    call_ok(&client, "redo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, after);

    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({"path":project["path"]})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let repeat = call_ok(&client, "design_preview", document.clone()).await;
    assert_eq!(repeat["diff"], json!([]));
    let noop = call_ok(
        &client,
        "design_apply",
        json!({"preview":repeat["preview"]}),
    )
    .await;
    assert_eq!(noop["operation_count"], 0);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, after);

    let mut wire = document.clone();
    wire["wire_groups"] = json!([{"net":"EN","pins":["R3.2","R2.1"]}]);
    assert!(
        call(&client, "design_preview", wire.clone())
            .await
            .is_error
            .unwrap_or(false)
    );
    wire.as_object_mut().unwrap().remove("block");
    let preview = call_ok(&client, "design_preview", wire).await;
    call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    let wired = call_ok(&client, "schematic_get", json!({})).await;
    let denied = call(&client, "design_preview", document).await;
    assert!(denied.is_error.unwrap_or(false));
    assert!(
        denied
            .structured_content
            .unwrap()
            .to_string()
            .contains("shares wire segment")
    );
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, wired);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_design_creates_nets_parts_and_paths_together() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let project = call_ok(&client, "project_create", json!({"name":"One document"})).await;
    let device = call_ok(
        &client,
        "library_search",
        json!({"query":"R-0805","kind":"device"}),
    )
    .await;
    let device = &device["elements"][0];
    let component = &device["component"]["uuid"];
    let spec = |name, x, mirrored| {
        json!({"designator":name,"library_component":component,"device":device["uuid"],
            "value":"10 kΩ","position":{"x":x,"y":0},"mirrored":mirrored,
            "assign_only":true,"connections":{"2":"EN"}})
    };
    let mut document = json!({
        "version":1,"schematic":"Main","add_nets":["EN"],
        "add_components":{"pullup":spec("R1",0,false),"load":spec("R2",25,true)},
        "wire_groups":[{"net":"EN","paths":[["pullup.2","load.2"]]}],
        "nets":{"EN":["load.2"]},
    });
    document["add_components"]["load"]["fields"] = json!({
        "name":{"offset":{"x":2,"y":3},"rotation":0,"height":1.5}
    });
    let before = call_ok(&client, "project_summary", json!({})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let mut invalid = document.clone();
    invalid["add_components"]["pullup"]["connections"] = json!({"missing":"EN"});
    assert_eq!(
        call(&client, "design_preview", invalid).await.is_error,
        Some(true)
    );
    assert_eq!(call_ok(&client, "project_summary", json!({})).await, before);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    let preview = call_ok(&client, "design_preview", document.clone()).await;
    assert_eq!(preview["diff"].as_array().unwrap().len(), 4);
    assert_eq!(preview["diff"][0]["field"], "add_net");
    assert_eq!(preview["diff"][3]["field"], "wire_paths");
    assert_eq!(preview["diff"][3]["pins"], json!(["pullup.2", "load.2"]));
    assert_eq!(preview["verified_pins"], 2);
    assert_eq!(call_ok(&client, "project_summary", json!({})).await, before);
    call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    let after = call_ok(&client, "schematic_get", json!({})).await;
    assert_eq!(after["net_segments"].as_array().unwrap().len(), 1);
    assert_eq!(
        after["net_segments"][0]["lines"].as_array().unwrap().len(),
        1
    );
    assert!(
        after["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "R2" && s["mirrored"] == true)
    );
    let resistor = after["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "R2")
        .unwrap();
    let name = resistor["texts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["layer"] == "sym_names")
        .unwrap();
    assert_eq!(name["position"], json!({"x":27.0,"y":3.0}));
    assert_eq!(name["rotation"], 0.0);
    assert_eq!(name["height"], 1.5);
    assert_eq!(
        call_ok(&client, "netlist", json!({})).await["nets"]["EN"],
        json!(["R1.2", "R2.2"])
    );
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(
        call_ok(&client, "project_summary", json!({})).await["counts"],
        before["counts"]
    );
    call_ok(&client, "redo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, after);
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({"path":project["path"]})).await;
    let repeated = call_ok(&client, "design_preview", document).await;
    assert_eq!(repeated["diff"], json!([]));
    assert_eq!(
        call_ok(
            &client,
            "design_apply",
            json!({"preview":repeated["preview"]})
        )
        .await["operation_count"],
        0
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_design_topology_edits_are_guarded_and_replayable() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let project = call_ok(&client, "project_create", json!({"name":"Topology review"})).await;
    let r1 = call_ok(
        &client,
        "component_add",
        json!({"part":"R-0805","designator":"R1","position":{"x":0,"y":0}}),
    )
    .await;
    let r2 = call_ok(
        &client,
        "component_add",
        json!({"part":"R-0805","designator":"R2","position":{"x":40,"y":0}}),
    )
    .await;
    call_ok(
        &client,
        "connect",
        json!({"net":"EN","pins":["R1.2","R2.1"],"assign_only":true}),
    )
    .await;
    let prepared = call_ok(
        &client,
        "design_preview",
        json!({"version":1,"schematic":"Main","add_nets":["SPARE"],
            "components":{"R1":{"uuid":r1["uuid"]},"R2":{"uuid":r2["uuid"]}},
            "wire_groups":[{"net":"EN","paths":[["R1.2","R2.1"]]}]}),
    )
    .await;
    call_ok(
        &client,
        "design_apply",
        json!({"preview":prepared["preview"]}),
    )
    .await;
    let page = call_ok(&client, "schematic_get", json!({})).await;
    let netlist = call_ok(&client, "netlist", json!({})).await;
    let segments = page["net_segments"].as_array().unwrap();
    assert_eq!(segments.len(), 1);
    assert!(segments[0]["labels"].as_array().unwrap().is_empty());
    let removal = call_ok(
        &client,
        "design_preview",
        json!({"version":1,"schematic":"Main","remove_components":[r2["uuid"]]}),
    )
    .await;
    call_ok(
        &client,
        "design_apply",
        json!({"preview":removal["preview"]}),
    )
    .await;
    assert_eq!(
        call_ok(&client, "netlist", json!({})).await["nets"]["EN"],
        json!(["R1.2"])
    );
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, page);
    let summary = call_ok(&client, "project_summary", json!({})).await;
    let document = json!({"version":1,"schematic":"Main",
        "components":{"r":{"uuid":r1["uuid"]}},"remove_components":[r2["uuid"]],
        "add_nets":["RESET"],"pin_nets":{"r.2":{"expected":"EN","set":"RESET"}}});
    for edit in [
        json!({"expected":"OTHER","set":"RESET"}),
        json!({"expected":"EN"}),
    ] {
        let mut invalid = document.clone();
        invalid["pin_nets"]["r.2"] = edit;
        assert_eq!(
            call(&client, "design_preview", invalid).await.is_error,
            Some(true)
        );
        assert_eq!(
            call_ok(&client, "project_summary", json!({})).await,
            summary
        );
    }
    let preview = call_ok(&client, "design_preview", document.clone()).await;
    assert_eq!(preview["diff"].as_array().unwrap().len(), 3);
    let erc = &preview["erc"];
    let mentions = |group: &str, name: &str| {
        erc[group].as_array().unwrap().iter().any(|message| {
            message["kind"].as_str().unwrap().starts_with("OpenNet(")
                && message["message"].as_str().unwrap().contains(name)
        })
    };
    assert!(mentions("introduced", "RESET"));
    assert!(mentions("inherited", "SPARE"));
    assert!(!mentions("introduced", "SPARE"));
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, page);
    call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    let applied = call_ok(&client, "netlist", json!({})).await;
    assert_eq!(applied["nets"], json!({"RESET":["R1.2"],"SPARE":[]}));
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "netlist", json!({})).await, netlist);
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, page);
    call_ok(&client, "redo", json!({})).await;
    assert_eq!(call_ok(&client, "netlist", json!({})).await, applied);
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({"path":project["path"]})).await;
    assert_eq!(
        call_ok(&client, "design_preview", document).await["diff"],
        json!([])
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_design_preview_reports_resolved_erc_without_changing_approvals() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    call_ok(&client, "project_create", json!({"name":"ERC review"})).await;
    for (name, x) in [("R1", 0), ("R2", 40)] {
        call_ok(
            &client,
            "component_add",
            json!({"part":"R-0805","designator":name,"position":{"x":x,"y":0}}),
        )
        .await;
    }
    call_ok(&client, "connect", json!({"net":"EN","pins":["R1.2"]})).await;
    let net = call_ok(&client, "net_list", json!({"net":"EN"})).await["nets"][0]["uuid"].clone();
    use librepcb_core::serialization::{List, SExpression};
    let mut approval = List::new("approved");
    approval.push(SExpression::token("open_net"));
    let net: librepcb_core::types::Uuid = net.as_str().unwrap().parse().unwrap();
    approval.append_child("net", &net);
    call_ok(
        &client,
        "mutation_apply",
        json!({"mutation":{"SetErcApprovals":[SExpression::List(approval)]}}),
    )
    .await;
    let erc = call_ok(&client, "erc_run", json!({"include_approved":true})).await;
    assert_eq!(erc["approved"], 1);
    let before = call_ok(&client, "project_summary", json!({})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let response = call(
        &client,
        "design_preview",
        json!({"version":1,"schematic":"Main","image_region":{"min":{"x":20,"y":-10},"max":{"x":60,"y":10}},
        "components":{"r2":{"designator":"R2"}},"pin_nets":{"r2.1":{"expected":null,"set":"EN"}}}),
    )
    .await;
    assert_ne!(response.is_error, Some(true));
    let preview = &response.structured_content.as_ref().unwrap()["result"];
    assert_eq!(preview["images"], json!(["after", "before"]));
    assert_eq!(
        preview["image_region"],
        json!({"min":{"x":20.0,"y":-10.0},"max":{"x":60.0,"y":10.0}})
    );
    let images: Vec<_> = response
        .content
        .iter()
        .filter_map(|content| match content {
            ContentBlock::Image(image) => Some(image),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 2);
    assert_ne!(images[0].data, images[1].data);
    let render = call(
        &client,
        "render",
        json!({"schematic":"Main", "image_region":preview["image_region"]}),
    )
    .await;
    assert_ne!(render.is_error, Some(true));
    assert_eq!(
        render.structured_content.unwrap()["result"]["image_region"],
        preview["image_region"]
    );
    let live_png = render.content.iter().find_map(|content| match content {
        ContentBlock::Image(image) => Some(image),
        _ => None,
    });
    assert_eq!(live_png.unwrap().data, images[1].data);
    let invalid = call(
        &client,
        "render",
        json!({"target":"board", "image_region":preview["image_region"]}),
    )
    .await;
    assert_eq!(invalid.is_error, Some(true));
    assert_eq!(
        invalid.structured_content.unwrap()["error"]["kind"],
        "invalid_argument"
    );
    assert_eq!(preview["erc"]["introduced"], json!([]));
    assert!(
        preview["erc"]["resolved"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["approved"] == true
                && finding["kind"].as_str().unwrap().starts_with("OpenNet("))
    );
    assert_eq!(call_ok(&client, "project_summary", json!({})).await, before);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    assert_eq!(
        call_ok(&client, "erc_run", json!({"include_approved":true})).await,
        erc
    );
    let unknown = call(
        &client,
        "design_discard",
        json!({"preview":"00000000-0000-4000-8000-000000000000"}),
    )
    .await;
    assert_eq!(unknown.is_error, Some(true));
    assert_eq!(
        unknown.structured_content.unwrap()["error"]["kind"],
        "not_found"
    );
    let args = json!({"preview":preview["preview"]});
    assert_eq!(call_ok(&client, "design_discard", args.clone()).await, args);
    let refused = call(&client, "design_apply", args).await;
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(
        refused.structured_content.unwrap()["error"]["kind"],
        "not_found"
    );
    assert_eq!(call_ok(&client, "project_summary", json!({})).await, before);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_board_requirements_gate_routing_and_survive_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let project = call_ok(
        &client,
        "project_create",
        json!({"name":"Board requirements"}),
    )
    .await;
    for (name, x) in [("R1", 5), ("R2", 15), ("R3", 23)] {
        call_ok(
            &client,
            "component_add",
            json!({"part":"R-0805","designator":name}),
        )
        .await;
        call_ok(
            &client,
            "device_place",
            json!({
                "component":name,"position":{"x":x,"y":10},"rotation":90
            }),
        )
        .await;
    }
    call_ok(
        &client,
        "connect",
        json!({"net":"A","pins":["R1.1","R2.1"]}),
    )
    .await;
    call_ok(
        &client,
        "connect",
        json!({"net":"B","pins":["R1.2","R3.2"]}),
    )
    .await;
    call_ok(
        &client,
        "board_set_outline",
        json!({"width":30,"height":20}),
    )
    .await;
    let protected = call_ok(
        &client,
        "trace_add",
        json!({
            "from":"R1.2","to":"R3.2","layer":"top","width":0.3
        }),
    )
    .await["traces"][0]["uuid"]
        .clone();
    let baseline = call_ok(&client, "board_get", json!({})).await;
    let stale = call_ok(&client, "routing_prepare", json!({"nets":["A"]})).await;
    let empty = call_ok(&client, "board_requirements_get", json!({})).await;
    assert_eq!(empty["document"]["requirements"], json!([]));
    let document = json!({"version":1,"requirements":[
        {"id":"placement","source":"Mechanical review","severity":"hard","board":"default",
         "check":{"kind":"device_region","component":"R2","min":{"x":11,"y":9},"max":{"x":15,"y":11}}},
        {"id":"distance","source":"Board layout","severity":"hard","board":"default",
         "check":{"kind":"pad_distance","from":"R1.1","to":"R2.1","max_distance_mm":10.1}},
        {"id":"short-path","source":"Board layout","severity":"hard","board":"default",
         "check":{"kind":"trace_path","from":"R1.1","to":"R2.1","max_length_mm":7,"max_layer_changes":0,"layers":["top"]}},
        {"id":"top-only","source":"Layer plan","severity":"hard","board":"default",
         "check":{"kind":"net_routing","net":"A","layers":["top"],"allow_vias":false,"priority":true}},
        {"id":"reference","source":"Board layout","severity":"hard","board":"default",
         "check":{"kind":"protected_copper","object":protected}},
        {"id":"distance-advisory","source":"Board layout","severity":"warning","board":"default",
         "check":{"kind":"pad_distance","from":"R1.1","to":"R2.1","max_distance_mm":5}}
    ]});
    let stored = call_ok(
        &client,
        "board_requirements_set",
        json!({
            "expected_revision":empty["revision"],"document":document
        }),
    )
    .await["document"]
        .clone();
    let status = call_ok(&client, "board_requirements_check", json!({})).await;
    assert_eq!(status["pending_requirements"], json!(["short-path"]));
    assert_eq!(status["blocking_requirements"], json!([]));
    let stale_apply = call(
        &client,
        "routing_apply",
        json!({"candidate":stale["candidate"]}),
    )
    .await;
    assert_eq!(
        stale_apply.structured_content.unwrap()["error"]["kind"],
        "stale_revision"
    );
    call_ok(
        &client,
        "routing_discard",
        json!({"candidate":stale["candidate"]}),
    )
    .await;
    let revision = status["revision"].clone();
    let unchanged = call_ok(
        &client,
        "board_requirements_set",
        json!({
            "expected_revision":revision,"document":stored
        }),
    )
    .await;
    assert_eq!(unchanged["change_count"], 0);
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(
        call_ok(&client, "board_requirements_get", json!({})).await["document"],
        empty["document"]
    );
    call_ok(&client, "redo", json!({})).await;
    let saved = call_ok(&client, "board_requirements_get", json!({})).await;
    assert_eq!(saved["document"], stored);

    let blocked = call(&client, "trace_remove", json!({"uuids":[protected]})).await;
    assert_eq!(
        blocked.structured_content.unwrap()["error"]["kind"],
        "refused"
    );
    let blocked = call(
        &client,
        "routing_repair_start",
        json!({
            "target":"A","groups":[[protected]],
            "region":{"min":{"x":0,"y":0},"max":{"x":30,"y":20}},
            "rip_up_region":{"min":{"x":0.5,"y":8},"max":{"x":27,"y":13}}
        }),
    )
    .await;
    let error = &blocked.structured_content.unwrap()["error"];
    assert_eq!(error["kind"], "refused", "{error:#}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("protected copper"),
        "{error:#}"
    );

    let denied = call(
        &client,
        "routing_prepare",
        json!({
            "nets":["A"],"layers":["bottom"]
        }),
    )
    .await
    .structured_content
    .unwrap()["result"]
        .clone();
    assert_eq!(denied["routing"]["traces"], 0, "{denied:#}");
    call_ok(
        &client,
        "routing_discard",
        json!({"candidate":denied["candidate"]}),
    )
    .await;
    let too_long = call(&client, "routing_prepare", json!({"nets":["A"]}))
        .await
        .structured_content
        .unwrap();
    let result = &too_long["result"];
    let findings = result["validation"]["board_requirements_after"]
        .as_array()
        .unwrap();
    let path = findings
        .iter()
        .find(|finding| finding["id"] == "short-path")
        .unwrap();
    assert_eq!(path["status"], "fail");
    assert_eq!(path["measured"]["layers"], json!(["top_cu"]));
    let refused = call(
        &client,
        "routing_apply",
        json!({"candidate":result["candidate"]}),
    )
    .await;
    assert_eq!(
        refused.structured_content.unwrap()["error"]["kind"],
        "refused"
    );
    call_ok(
        &client,
        "routing_discard",
        json!({"candidate":result["candidate"]}),
    )
    .await;
    let direct = call(
        &client,
        "autoroute",
        json!({"backend":"builtin","nets":["A"]}),
    )
    .await;
    assert_eq!(
        direct.structured_content.unwrap()["error"]["kind"],
        "refused"
    );
    assert_eq!(call_ok(&client, "board_get", json!({})).await, baseline);

    let compare = call_ok(
        &client,
        "routing_placement_compare",
        json!({
            "targets":[{"component":"R2","positions":[{"x":11,"y":10},{"x":10,"y":10}]}],
            "copper":{"route":{"nets":["A"]}}
        }),
    )
    .await;
    assert_eq!(
        compare["trials"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|trial| trial["candidate"].is_string())
            .count(),
        1,
        "{compare:#}"
    );
    assert!(
        compare["trials"].as_array().unwrap().iter().any(|trial| {
            trial["validation"]["board_requirements_after"]
                .as_array()
                .is_some_and(|findings| {
                    findings
                        .iter()
                        .any(|finding| finding["id"] == "placement" && finding["status"] == "fail")
                })
        }),
        "{compare:#}"
    );
    let applied = call_ok(
        &client,
        "routing_apply",
        json!({"candidate":compare["recommended"]}),
    )
    .await;
    assert_eq!(applied["validation"]["acceptable"], true, "{applied:#}");
    let status = call_ok(&client, "board_requirements_check", json!({})).await;
    assert_eq!(status["pending_requirements"], json!([]));
    assert_eq!(status["blocking_requirements"], json!([]));
    assert!(
        status["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["id"] == "distance-advisory" && finding["status"] == "fail")
    );
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({"path":project["path"]})).await;
    assert_eq!(
        call_ok(&client, "board_requirements_get", json!({})).await["document"],
        stored
    );
    assert_eq!(
        call_ok(&client, "board_requirements_check", json!({})).await["findings"],
        status["findings"]
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_schematic_requirements_persist_and_gate_design_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let project = call_ok(
        &client,
        "project_create",
        json!({"name":"Design requirements"}),
    )
    .await;
    call_ok(
        &client,
        "component_add",
        json!({"part":"R-0805","designator":"R1","position":{"x":0,"y":0}}),
    )
    .await;
    call_ok(&client, "connect", json!({"net":"EN","pins":["R1.1"]})).await;
    let part = call_ok(&client, "component_get", json!({"component":"R1"})).await;
    let sheet = call_ok(&client, "schematic_get", json!({})).await;
    let net = call_ok(&client, "net_list", json!({"net":"EN"})).await["nets"][0]["uuid"].clone();
    let signal = part["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|signal| signal["net"] == "EN")
        .unwrap();
    let symbol = json!({"schematic":sheet["uuid"],"symbol":sheet["symbols"][0]["uuid"]});
    let empty = call_ok(&client, "requirements_get", json!({})).await;
    let stale = call_ok(
        &client,
        "design_preview",
        json!({"version":1,"schematic":"Main"}),
    )
    .await;
    let document = json!({"version":1,"requirements":[
        {"id":"anchor","source":"placement review","severity":"hard",
         "check":{"kind":"symbol_position","symbol":symbol,"position":{"x":0.0,"y":0.0}}},
        {"id":"region","source":"placement review","severity":"warning",
         "check":{"kind":"symbol_region","symbol":symbol,"min":{"x":-2.0,"y":-2.0},"max":{"x":-1.0,"y":-1.0}}},
        {"id":"rotation","source":"placement review","severity":"hard",
         "check":{"kind":"symbol_orientation","symbol":symbol,"rotation":360.0,"mirrored":false}},
        {"id":"enable","source":"EN/RESET circuit","severity":"hard",
         "check":{"kind":"pin_net","pin":{"component":part["uuid"],"signal":signal["uuid"]},"net":net}}
    ]});
    let mut invalid = document.clone();
    invalid["requirements"][3]["check"]
        .as_object_mut()
        .unwrap()
        .remove("net");
    assert_eq!(
        call(
            &client,
            "requirements_set",
            json!({"expected_revision":empty["revision"],"document":invalid})
        )
        .await
        .is_error,
        Some(true)
    );
    assert_eq!(call_ok(&client, "requirements_get", json!({})).await, empty);
    let set = call_ok(
        &client,
        "requirements_set",
        json!({"expected_revision":empty["revision"],"document":document}),
    )
    .await;
    assert_eq!(set["change_count"], 1);
    let error = call(&client, "design_apply", json!({"preview":stale["preview"]}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(error["error"]["kind"], "stale_revision");
    let saved = call_ok(&client, "requirements_get", json!({})).await;
    let mut reversed = document.clone();
    reversed["requirements"].as_array_mut().unwrap().reverse();
    let repeated = call_ok(
        &client,
        "requirements_set",
        json!({"expected_revision":saved["revision"],"document":reversed}),
    )
    .await;
    assert_eq!(repeated["change_count"], 0);
    assert_eq!(call_ok(&client, "requirements_get", json!({})).await, saved);
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(
        call_ok(&client, "requirements_get", json!({})).await["document"],
        empty["document"]
    );
    call_ok(&client, "redo", json!({})).await;
    assert_eq!(
        call_ok(&client, "requirements_get", json!({})).await["document"],
        saved["document"]
    );
    let net_key = net.as_str().unwrap();
    let rename = json!({"version":1,"schematic":"Main","rename_nets":{(net_key):{"expected":"EN","set":"ENABLE"}},
        "components":{"r":{"uuid":part["uuid"]}},"nets":{"ENABLE":["r.1"]}});
    let mut conflict = rename.clone();
    conflict["rename_nets"][net_key]["expected"] = json!("WRONG");
    let before = call_ok(&client, "netlist", json!({})).await;
    let rejected = call(&client, "design_preview", conflict)
        .await
        .structured_content
        .unwrap();
    assert_eq!(rejected["error"]["kind"], "conflict");
    let renamed = call_ok(&client, "design_preview", rename.clone()).await;
    assert_eq!(
        renamed["diff"],
        json!([{"field":"net_name","uuid":net,"before":"EN","after":"ENABLE"}])
    );
    assert_eq!(renamed["erc"]["introduced"], json!([]));
    assert_eq!(renamed["erc"]["resolved"], json!([]));
    assert_eq!(call_ok(&client, "netlist", json!({})).await, before);
    call_ok(
        &client,
        "design_apply",
        json!({"preview":renamed["preview"]}),
    )
    .await;
    assert_eq!(
        call_ok(&client, "net_list", json!({"net":"ENABLE"})).await["nets"][0]["uuid"],
        net
    );
    assert_eq!(
        call_ok(&client, "design_preview", rename).await["diff"],
        json!([])
    );
    let live = call_ok(&client, "requirements_check", json!({})).await;
    assert_eq!(live["findings"][0]["status"], "pass");
    assert_eq!(live["findings"][1]["status"], "pass");
    assert_eq!(live["findings"][1]["measured"]["net"], net);
    assert_eq!(live["findings"][2]["status"], "fail");
    assert_eq!(live["findings"][3]["status"], "pass");
    assert_eq!(
        live["findings"][3]["measured"],
        json!({"rotation":0.0,"mirrored":false})
    );
    assert_eq!(live["blocking_requirements"], json!([]));

    let allowed = call_ok(&client, "design_preview", json!({"version":1,"schematic":"Main","components":{"r":{"uuid":part["uuid"],"value":{"expected":part["value"],"set":"10k"}}}})).await;
    call_ok(
        &client,
        "design_apply",
        json!({"preview":allowed["preview"]}),
    )
    .await;
    let before = call_ok(&client, "project_summary", json!({})).await;
    let removal = call_ok(
        &client,
        "design_preview",
        json!({"version":1,"schematic":"Main","remove_components":[part["uuid"]]}),
    )
    .await;
    assert_eq!(
        removal["requirements"]["findings"][0]["missing_reference"],
        "symbol"
    );
    assert_eq!(
        removal["requirements"]["findings"][1]["missing_reference"],
        "component"
    );
    let moved = call_ok(&client, "design_preview", json!({"version":1,"schematic":"Main","components":{"r":{"uuid":part["uuid"],"position":{"expected":{"x":0,"y":0},"set":{"x":5,"y":5}}}}})).await;
    let evidence = &moved["requirements"];
    assert_eq!(
        evidence["artifact"],
        json!({"kind":"design_preview","base_revision":moved["base_revision"],"preview":moved["preview"]})
    );
    assert_eq!(evidence["blocking_requirements"], json!(["anchor"]));
    assert_eq!(
        evidence["findings"][0]["measured"]["position"],
        json!({"x":5.0,"y":5.0})
    );
    let refused = call(&client, "design_apply", json!({"preview":moved["preview"]}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(refused["error"]["kind"], "refused");
    assert_eq!(call_ok(&client, "project_summary", json!({})).await, before);

    let oriented = call_ok(
        &client,
        "design_preview",
        json!({"version":1,"schematic":"Main",
        "components":{"r":{"uuid":part["uuid"],"symbols":{(symbol["symbol"].as_str().unwrap()):{
            "rotation":{"expected":0,"set":90},"mirrored":{"expected":false,"set":true}
        }}}}}),
    )
    .await;
    assert_eq!(
        oriented["requirements"]["blocking_requirements"],
        json!(["rotation"])
    );
    assert_eq!(
        oriented["requirements"]["findings"][3]["measured"],
        json!({"rotation":90.0,"mirrored":true})
    );
    let refused = call(
        &client,
        "design_apply",
        json!({"preview":oriented["preview"]}),
    )
    .await;
    assert_eq!(
        refused.structured_content.unwrap()["error"]["kind"],
        "refused"
    );
    assert_eq!(call_ok(&client, "project_summary", json!({})).await, before);

    call_ok(&client, "project_save", json!({})).await;
    let path = Path::new(project["path"].as_str().unwrap());
    let asset = path
        .parent()
        .unwrap()
        .join(librepcb_core::project::schematic_requirements::PATH);
    let bytes = std::fs::read(&asset).unwrap();
    call_ok(&client, "project_close", json!({})).await;
    if let Some(cli) = crate::common::upstream_cli() {
        let (ok, output) =
            crate::common::run_cli(&cli, &["open-project", path.to_str().unwrap(), "--save"]);
        assert!(ok, "upstream save failed: {output}");
        assert_eq!(std::fs::read(&asset).unwrap(), bytes);
    }
    call_ok(&client, "project_open", json!({"path":path})).await;
    let reopened = call_ok(&client, "requirements_get", json!({})).await;
    assert_eq!(reopened["document"], saved["document"]);
    assert_eq!(
        call_ok(&client, "requirements_check", json!({})).await["findings"],
        live["findings"]
    );
    call_ok(
        &client,
        "requirements_set",
        json!({"expected_revision":reopened["revision"],"document":empty["document"]}),
    )
    .await;
    call_ok(&client, "project_save", json!({})).await;
    assert!(!asset.exists());
    call_ok(&client, "undo", json!({})).await;
    call_ok(&client, "project_save", json!({})).await;
    assert_eq!(std::fs::read(asset).unwrap(), bytes);

    let archive_job =
        librepcb_core::job::OutputJob::new_default::<librepcb_core::job::LppzOutputJob>();
    call_ok(
        &client,
        "mutation_apply",
        json!({"mutation":{"SetOutputJobs":[archive_job]}}),
    )
    .await;
    let archive_output = call_ok(
        &client,
        "jobs_run",
        json!({"output_dir":tmp.path().join("archive")}),
    )
    .await;
    let archive =
        librepcb_core::fileio::FilePath::new(archive_output["files"][0].as_str().unwrap()).unwrap();
    let directory = librepcb_core::fileio::TransactionalDirectory::new_temporary().unwrap();
    directory.file_system().load_from_zip(&archive).unwrap();
    let archived_project = librepcb_core::project::ProjectLoader::new()
        .open(directory, path.file_name().unwrap().to_str().unwrap())
        .unwrap();
    use librepcb_core::fileio::FileSystem;
    assert_eq!(
        archived_project
            .directory()
            .read(librepcb_core::project::schematic_requirements::PATH)
            .unwrap(),
        bytes
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_design_add_component_replays_library_and_keeps_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let project = call_ok(&client, "project_create", json!({"name":"Design Add"})).await;
    call_ok(
        &client,
        "component_add",
        json!({"part":"R-0805","designator":"R1"}),
    )
    .await;
    for (pin, net) in [("R1.1", "VCC"), ("R1.2", "GND")] {
        call_ok(&client, "connect", json!({"pins":[pin],"net":net})).await;
    }
    let device = call_ok(
        &client,
        "library_search",
        json!({"query":"C-0805","kind":"device"}),
    )
    .await["elements"][0]
        .clone();
    let document = json!({
        "version":1,"schematic":"Main","components":{},
        "add_components":{"decoupler":{
            "designator":"C1","library_component":device["component"]["uuid"],"device":device["uuid"],
            "value":"100 nF","position":{"x":40.64,"y":40.64},"connections":{"1":"VCC","2":"GND"}
        }}
    });
    let before = call_ok(&client, "project_summary", json!({})).await;
    let mut invalid = document.clone();
    invalid["add_components"]["decoupler"]["connections"]["2"] = json!("missing");
    let rejected = call(&client, "design_preview", invalid).await;
    assert_eq!(
        rejected.structured_content.unwrap()["error"]["kind"],
        "not_found"
    );
    assert_eq!(call_ok(&client, "project_summary", json!({})).await, before);

    let preview = call_ok(&client, "design_preview", document.clone()).await;
    assert_eq!(preview["diff"].as_array().unwrap().len(), 1);
    let change = &preview["diff"][0];
    let id = &change["uuid"];
    assert_eq!(change["connections"].as_array().unwrap().len(), 2);
    assert_eq!(call_ok(&client, "project_summary", json!({})).await, before);
    let applied = call_ok(
        &client,
        "design_apply",
        json!({"preview":preview["preview"]}),
    )
    .await;
    assert!(
        applied["operation_count"].as_u64().unwrap() > applied["mutation_count"].as_u64().unwrap()
    );
    let installed = call_ok(&client, "project_summary", json!({})).await;
    assert_eq!(
        installed["counts"]["library"]["devices"].as_u64().unwrap(),
        before["counts"]["library"]["devices"].as_u64().unwrap() + 1
    );
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(
        call_ok(&client, "project_summary", json!({})).await["counts"],
        before["counts"]
    );
    call_ok(&client, "redo", json!({})).await;
    let rename = json!({"version":1,"schematic":"Main", "components":{
        "cap":{"uuid":id,"name":{"expected":"C1","set":"C7"}}
    }});
    let mut conflict = rename.clone();
    conflict["components"]["cap"]["name"]["expected"] = json!("C2");
    let refused = call(&client, "design_preview", conflict).await;
    assert_eq!(
        refused.structured_content.unwrap()["error"]["kind"],
        "conflict"
    );
    let before_rename = call_ok(&client, "schematic_get", json!({})).await;
    let renamed = call_ok(&client, "design_preview", rename.clone()).await;
    assert_eq!(
        renamed["diff"],
        json!([{"component":"cap","uuid":id,"field":"name","before":"C1","after":"C7"}])
    );
    assert_eq!(
        call_ok(&client, "schematic_get", json!({})).await,
        before_rename
    );
    call_ok(
        &client,
        "design_apply",
        json!({"preview":renamed["preview"]}),
    )
    .await;
    let moved = json!({"x":48.26,"y":40.64});
    call_ok(
        &client,
        "symbol_move",
        json!({"component":"C7","position":moved}),
    )
    .await;
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({"path":project["path"]})).await;
    let repeat = call_ok(&client, "design_preview", document).await;
    assert_eq!(repeat["diff"], json!([]));
    let noop = call_ok(
        &client,
        "design_apply",
        json!({"preview":repeat["preview"]}),
    )
    .await;
    assert_eq!(noop["operation_count"], 0);
    let component = call_ok(&client, "component_get", json!({"component":"C7"})).await;
    assert_eq!(&component["uuid"], id);
    assert_eq!(component["symbols"][0]["position"], moved);
    let renamed_again = call_ok(&client, "design_preview", rename).await;
    assert_eq!(renamed_again["diff"], json!([]));
    call_ok(
        &client,
        "component_add",
        json!({"part":"C-0805","designator":"C8"}),
    )
    .await;
    let mut target_text = Value::Null;
    for component in ["C7", "C8"] {
        call_ok(
            &client,
            "symbol_text_move",
            json!({
                "component":component,"text":"name","position":{"x":50,"y":48},"height":3.0,"rotation":0
            }),
        )
        .await.clone_into(&mut target_text);
    }
    let reviewed = call_ok(
        &client,
        "design_preview",
        json!({
            "version":1,"schematic":"Main","components":{"cap":{"uuid":id}},
            "review_components":["cap"]
        }),
    )
    .await;
    assert_eq!(reviewed["diff"], json!([]));
    let live = call_ok(
        &client,
        "schematic_review",
        json!({"schematic":"Main","components":["C7"]}),
    )
    .await;
    assert_eq!(live["annotation_overlaps"], reviewed["annotation_overlaps"]);
    assert_eq!(reviewed["annotation_changes"]["introduced"], json!([]));
    assert_eq!(reviewed["annotation_changes"]["resolved"], json!([]));
    assert_eq!(
        reviewed["annotation_changes"]["inherited"],
        live["annotation_overlaps"]
    );
    assert!(
        reviewed["annotation_overlaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|overlap| {
                [
                    overlap["first"]["component"].as_str(),
                    overlap["second"]["component"].as_str(),
                ]
                .contains(&Some("C8"))
            }),
        "{reviewed}"
    );
    let component = call_ok(&client, "component_get", json!({"component":"C8"})).await;
    let moved = call_ok(
        &client,
        "design_preview",
        json!({
            "version":1,"schematic":"Main","review_components":["cap"],
            "components":{"cap":{"uuid":component["uuid"],
                "text_positions":{(target_text["uuid"].as_str().unwrap()):{
                    "expected":{"x":50,"y":48},"set":{"x":70,"y":70}
                }}
            }}
        }),
    )
    .await;
    assert_eq!(moved["diff"][0]["field"], "text_position");
    assert_eq!(moved["annotation_overlaps"], json!([]));
    assert_eq!(
        moved["annotation_changes"]["resolved"],
        live["annotation_overlaps"]
    );
    assert_eq!(moved["annotation_changes"]["introduced"], json!([]));
    call_ok(&client, "design_apply", json!({"preview":moved["preview"]})).await;
    let reverted = call_ok(
        &client,
        "design_preview",
        json!({
            "version":1,"schematic":"Main","review_components":["cap"],
            "components":{"cap":{"uuid":component["uuid"],
                "text_positions":{(target_text["uuid"].as_str().unwrap()):{
                    "expected":{"x":70,"y":70},"set":{"x":50,"y":48}
                }}
            }}
        }),
    )
    .await;
    assert_eq!(
        reverted["annotation_changes"]["introduced"],
        live["annotation_overlaps"]
    );
    assert_eq!(reverted["annotation_changes"]["inherited"], json!([]));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_places_one_symbol_field_within_bounds() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    let project = call_ok(&client, "project_create", json!({"name":"Field placement"})).await;
    for (name, x) in [("R1", 0), ("R2", 20)] {
        call_ok(
            &client,
            "component_add",
            json!({"part":"R-0805","designator":name,"position":{"x":x,"y":0}}),
        )
        .await;
        call_ok(&client, "symbol_text_move", json!({"component":name,"text":"name","position":{"x":10,"y":8},"height":3.0,"rotation":0})).await;
    }
    call_ok(
        &client,
        "connect",
        json!({"net":"LINK","pins":["R1.2","R2.1"]}),
    )
    .await;
    let before = call_ok(&client, "schematic_get", json!({})).await;
    let netlist = call_ok(&client, "netlist", json!({})).await;
    let history = call_ok(&client, "history", json!({})).await;
    let request = json!({"version":1,"schematic":"Main","components":{"part":{"designator":"R1"}},
        "place_text":{"component":"part","text":"name","min":{"x":5,"y":4},"max":{"x":15,"y":12}}});
    let mut too_dense = request.clone();
    too_dense["place_text"]["step_mm"] = json!(0.001);
    let refused = call(&client, "design_preview", too_dense).await;
    assert_eq!(
        refused.structured_content.unwrap()["error"]["kind"],
        "invalid_argument"
    );
    let mut too_small = request.clone();
    too_small["place_text"]["max"] = json!({"x":5.01,"y":4.01});
    let no_fit = call_ok(&client, "design_preview", too_small).await;
    assert_eq!(no_fit["text_placement"]["status"], "no_fit");
    assert_eq!(no_fit["diff"], json!([]));
    let proposed = call_ok(&client, "design_preview", request.clone()).await;
    assert_eq!(proposed["text_placement"]["status"], "moved");
    assert_eq!(proposed["diff"].as_array().unwrap().len(), 1);
    assert_eq!(proposed["diff"][0]["field"], "text_position");
    assert_eq!(proposed["annotation_changes"]["introduced"], json!([]));
    assert_eq!(
        proposed["annotation_changes"]["resolved"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, before);
    assert_eq!(call_ok(&client, "history", json!({})).await, history);
    call_ok(
        &client,
        "design_apply",
        json!({"preview":proposed["preview"]}),
    )
    .await;
    let after = call_ok(&client, "schematic_get", json!({})).await;
    let mut expected = before.clone();
    let text = expected["symbols"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|symbol| symbol["component"] == "R1")
        .unwrap()["texts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|text| text["uuid"] == proposed["text_placement"]["text"])
        .unwrap();
    text["position"] = proposed["text_placement"]["position"].clone();
    assert_eq!(after, expected);
    assert_eq!(call_ok(&client, "netlist", json!({})).await, netlist);
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, before);
    call_ok(&client, "redo", json!({})).await;
    call_ok(&client, "project_save", json!({})).await;
    call_ok(&client, "project_close", json!({})).await;
    call_ok(&client, "project_open", json!({"path":project["path"]})).await;
    let repeated = call_ok(&client, "design_preview", request).await;
    assert_eq!(repeated["text_placement"]["status"], "unchanged");
    assert_eq!(repeated["diff"], json!([]));
    assert_eq!(call_ok(&client, "schematic_get", json!({})).await, after);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stdio_placement_search_generates_poses_and_stops_on_budget_or_cancel() {
    use std::time::Duration;

    let tmp = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_librepcb-mcp"));
    cmd.arg("--workspace").arg(tmp.path().join("ws"));
    let client = ().serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
    let info = call_ok(&client, "server_info", json!({})).await;
    crate::common::install_populated_library(Path::new(
        info["workspace"]["libraries_path"].as_str().unwrap(),
    ));
    call_ok(&client, "library_rescan", json!({})).await;
    call_ok(
        &client,
        "project_create",
        json!({"name":"Placement search"}),
    )
    .await;
    for (designator, x) in [("R1", 5), ("R2", 15)] {
        call_ok(
            &client,
            "component_add",
            json!({"part":"R-0805","designator":designator}),
        )
        .await;
        call_ok(
            &client,
            "device_place",
            json!({"component":designator,"position":{"x":x,"y":10},"rotation":90}),
        )
        .await;
    }
    call_ok(
        &client,
        "connect",
        json!({"net":"A","pins":["R1.1","R2.1"]}),
    )
    .await;
    call_ok(
        &client,
        "board_set_outline",
        json!({"width":30,"height":20}),
    )
    .await;
    let before = call_ok(&client, "board_get", json!({})).await;
    let mut request = json!({
        "targets":[{"component":"R2",
            "region":{"min":{"x":9,"y":10},"max":{"x":12,"y":10.5}}, "pitch_mm":1
        }],
        "trace_requirements":[{"requirement":{"path":{"from":"R1.1","to":"R2.1"}},"minimize":true}],
        "copper":{"route":{"nets":["A"],"layers":["top"]}},
        "max_trials":2
    });
    let started = call_ok(&client, "routing_placement_start", request.clone()).await;
    assert_eq!(started["total_poses"], 4);
    assert_eq!(started["planned_trials"], 2);
    let poll = |search: Value| {
        let client = &client;
        async move {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let status =
                        call_ok(client, "routing_placement_status", json!({"search":search})).await;
                    if status["phase"] == "finished" {
                        break status;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap()
        }
    };
    let finished = poll(started["search"].clone()).await;
    assert_eq!(finished["stop_reason"], "trial_limit", "{finished:#}");
    assert_eq!(finished["completed_trials"], 2);
    let result = &finished["comparison"];
    assert_eq!(
        result["trials"][0]["placements"][0]["position_mm"],
        json!([9.0, 10.0])
    );
    assert_eq!(
        result["trials"][1]["placements"][0]["position_mm"],
        json!([10.0, 10.0])
    );
    let candidate = &result["recommended"];
    let preview = call_ok(&client, "routing_preview", json!({"candidate":candidate})).await;
    assert_eq!(preview["validation"]["after"]["unrouted"], 0);
    assert_eq!(call_ok(&client, "board_get", json!({})).await, before);

    request["max_time_ms"] = json!(1);
    let started = call_ok(&client, "routing_placement_start", request.clone()).await;
    let finished = poll(started["search"].clone()).await;
    assert_eq!(finished["stop_reason"], "time_limit", "{finished:#}");
    assert_eq!(finished["completed_trials"], 0);

    request["targets"][0]["region"]["max"] = json!({"x":15,"y":16});
    request["max_trials"] = json!(49);
    request["max_time_ms"] = json!(10_000);
    let started = call_ok(&client, "routing_placement_start", request).await;
    let search = started["search"].clone();
    let acknowledgement = call_ok(
        &client,
        "routing_placement_status",
        json!({"search":search,"cancel":true}),
    )
    .await;
    assert_eq!(acknowledgement["cancel_requested"], true);
    let stopped = poll(search).await;
    assert_eq!(stopped["stop_reason"], "cancelled");
    assert!(stopped["completed_trials"].as_u64().unwrap() < 49);

    let applied = call_ok(&client, "routing_apply", json!({"candidate":candidate})).await;
    assert_eq!(applied["validation"]["after"]["unrouted"], 0);
    call_ok(&client, "undo", json!({})).await;
    assert_eq!(call_ok(&client, "board_get", json!({})).await, before);
    let guided = call_ok(
        &client,
        "routing_placement_start",
        json!({
            "targets":[{"component":"R2",
                "region":{"min":{"x":5.2,"y":10},"max":{"x":5.95,"y":10.2}},
                "pitch_mm":1,"rotations":[270]
            }],
            "copper":{}, "avoid_blockers":true, "max_trials":2
        }),
    )
    .await;
    assert_eq!(guided["total_poses"], 1);
    assert_eq!(guided["planned_trials"], 2);
    let stopped = poll(guided["search"].clone()).await;
    let trials = stopped["comparison"]["trials"].as_array().unwrap();
    assert_eq!(trials.len(), 2, "{stopped:#}");
    assert_eq!(trials[0]["status"], "rejected");
    assert_eq!(
        trials[0]["clearance_conflicts"][0]["layers"],
        json!(["top_cu"])
    );
    assert_eq!(trials[1]["suggested_by"], json!([0, 0]));
    assert_eq!(
        trials[1]["placements"][0]["position_mm"],
        json!([5.7, 10.0])
    );
    assert_eq!(call_ok(&client, "board_get", json!({})).await, before);
    client.cancel().await.unwrap();
}
