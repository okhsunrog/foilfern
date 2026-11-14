//! Process-level acceptance for local tool execution, persistence and failures.

use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_foilfern-mcp"))
        .args(args)
        .output()
        .unwrap()
}

fn records(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn batch(path: &Path, calls: Value, extra: &[&str]) -> Output {
    std::fs::write(path, serde_json::to_vec(&calls).unwrap()).unwrap();
    let mut args = vec!["batch", path.to_str().unwrap()];
    args.extend_from_slice(extra);
    run(&args)
}

#[test]
fn call_uses_the_registry_and_rejects_invalid_arguments() {
    let output = run(&["call", "server_info", "{}"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values = records(&output);
    assert_eq!(values.len(), 1);
    assert_eq!(
        values[0]["response"]["structuredContent"]["outcome"],
        "complete"
    );
    for args in [
        vec!["call", "server_info", r#"{"unknown":true}"#],
        vec!["call", "unknown_tool", "{}"],
        vec!["call", "server_info", "not JSON"],
        vec!["call", "server_info", "[]"],
    ] {
        let output = run(&args);
        assert!(!output.status.success(), "{args:?}");
    }
    let output = run(&["call", "server_info", r#"{"unknown":true}"#]);
    assert_eq!(
        records(&output)[0]["response"]["structuredContent"]["error"]["kind"],
        "invalid_argument"
    );
}

#[test]
fn file_and_stdin_inputs_are_supported() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("arguments.json");
    std::fs::write(&file, "{}").unwrap();
    let output = run(&["call", "server_info", &format!("@{}", file.display())]);
    assert!(output.status.success());
    for (args, input) in [
        (vec!["call", "server_info", "-"], "{}"),
        (vec!["batch", "-"], r#"[{"tool":"server_info"}]"#),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_foilfern-mcp"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(records(&output).len(), 1);
    }
}

#[test]
fn batch_shares_a_session_resolves_results_and_persists_only_explicitly() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("calls.json");
    let directory = tmp.path().join("project");
    let output = batch(
        &file,
        json!([
            {"tool":"project_create","arguments":{"name":"Local calls","directory":directory}},
            {"tool":"board_add","arguments":{"name":"Extra"}},
            {"tool":"project_save"},
            {"tool":"project_close"},
            {"tool":"project_open","arguments":{"path":{"$ref":"0#/structuredContent/result/path"}}},
            {"tool":"project_summary"}
        ]),
        &[],
    );
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let values = records(&output);
    assert_eq!(values.len(), 6);
    let path = values[0]["response"]["structuredContent"]["result"]["path"]
        .as_str()
        .unwrap();
    let output = run(&[
        "--project",
        path,
        "call",
        "board_add",
        r#"{"name":"Saved by flag"}"#,
        "--save",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(records(&output).last().unwrap()["tool"], "project_save");
    let output = run(&[
        "--project",
        path,
        "call",
        "board_add",
        r#"{"name":"Ephemeral"}"#,
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsaved changes"));
    let output = run(&[
        "--project",
        path,
        "call",
        "board_get",
        r#"{"board":"Ephemeral"}"#,
    ]);
    assert!(!output.status.success());
    let output = run(&[
        "--project",
        path,
        "call",
        "board_get",
        r#"{"board":"Saved by flag"}"#,
    ]);
    assert!(output.status.success());
    let output = batch(
        &file,
        json!([
            {"tool":"board_add","arguments":{"name":"Must not be saved"}},
            {"tool":"unknown"}
        ]),
        &["--save", "--project", path],
    );
    assert!(!output.status.success());
    assert!(records(&output).iter().all(|r| r["tool"] != "project_save"));
    let output = run(&[
        "--project",
        path,
        "call",
        "board_get",
        r#"{"board":"Must not be saved"}"#,
    ]);
    assert!(!output.status.success());
}

#[test]
fn batch_stops_or_continues_and_validates_before_execution() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("calls.json");
    let calls = json!([{"tool":"unknown"},{"tool":"server_info"}]);
    let output = batch(&file, calls.clone(), &[]);
    assert!(!output.status.success());
    assert_eq!(records(&output).len(), 1);
    let output = batch(&file, calls, &["--continue-on-error"]);
    assert!(!output.status.success());
    assert_eq!(records(&output).len(), 2);
    let project = tmp.path().join("must-not-exist");
    let output = batch(
        &file,
        json!([
            {"tool":"project_create","arguments":{"name":"Invalid batch","directory":project}},
            {"tool":"server_info","unexpected":true}
        ]),
        &[],
    );
    assert!(!output.status.success());
    assert!(!project.exists());
    assert!(output.stdout.is_empty());
    let output = batch(
        &file,
        json!([
            {"tool":"project_open","arguments":{"path":{"$ref":"1#/path"}}},
            {"tool":"server_info"}
        ]),
        &[],
    );
    assert!(!output.status.success());
    assert_eq!(records(&output).len(), 1);
}
