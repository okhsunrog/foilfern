//! CLI execution of tools through the shared registry, with JSON-lines results.

use std::io::{Read, Write};

use clap::Subcommand;
use foilfern_mcp::{FoilfernMcp, local::LocalRunner};
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Call one tool. JSON can be inline, @file, or - for stdin.
    Call {
        tool: String,
        #[arg(default_value = "{}")]
        json: String,
    },
    /// Execute a JSON array of {tool, arguments} calls in one session.
    Batch {
        /// JSON file, or - for stdin.
        file: String,
        /// Continue after failed calls; the final exit status still reports failure.
        #[arg(long)]
        continue_on_error: bool,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    tool: String,
    #[serde(default)]
    arguments: Map<String, Value>,
}

fn input(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        Ok(text)
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
    }
}

fn prepare(command: Command) -> Result<(Vec<Call>, bool), String> {
    match command {
        Command::Call { tool, json } => {
            let text = if json == "-" {
                input("-")?
            } else if let Some(path) = json.strip_prefix('@') {
                input(path)?
            } else {
                json
            };
            let arguments =
                serde_json::from_str(&text).map_err(|e| format!("invalid arguments JSON: {e}"))?;
            Ok((vec![Call { tool, arguments }], false))
        }
        Command::Batch {
            file,
            continue_on_error,
        } => {
            let calls = serde_json::from_str(&input(&file)?)
                .map_err(|e| format!("invalid batch JSON: {e}"))?;
            Ok((calls, continue_on_error))
        }
    }
}

// A singleton {$ref: "0#/structuredContent/result/preview"} takes a value from
// an earlier response. Other objects retain their ordinary tool-argument meaning.
fn references(value: &mut Value, history: &[Value]) -> Result<(), String> {
    match value {
        Value::Object(object) if object.len() == 1 && object.contains_key("$ref") => {
            let reference = object["$ref"].as_str().ok_or("$ref must be a string")?;
            let (index, pointer) = reference
                .split_once('#')
                .ok_or("$ref must be INDEX#JSON_POINTER")?;
            let index: usize = index.parse().map_err(|_| "invalid $ref index")?;
            let selected = history
                .get(index)
                .and_then(|result| result.pointer(pointer))
                .ok_or_else(|| format!("unresolved prior result reference {reference:?}"))?;
            *value = selected.clone();
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                references(value, history)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                references(value, history)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn emit(index: usize, tool: &str, response: &Value) -> Result<(), String> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(
        &mut output,
        &json!({"index":index,"tool":tool,"response":response}),
    )
    .map_err(|e| e.to_string())?;
    output
        .write_all(b"\n")
        .and_then(|_| output.flush())
        .map_err(|e| e.to_string())
}

async fn invoke(runner: &LocalRunner, tool: &str, arguments: Map<String, Value>) -> Value {
    match runner.call(tool, arguments).await {
        Ok(response) => serde_json::to_value(response)
            .unwrap_or_else(|e| json!({"isError":true,"error":e.to_string()})),
        Err(error) => json!({"isError":true,"error":error}),
    }
}

pub async fn execute(server: FoilfernMcp, command: Command, save: bool) -> Result<bool, String> {
    // Parse the entire batch before executing its first operation.
    let (calls, continue_on_error) = tokio::task::spawn_blocking(move || prepare(command))
        .await
        .map_err(|e| e.to_string())??;
    let runner = LocalRunner::new(server);
    let result = async {
        let mut history = Vec::new();
        let mut success = true;
        for (index, call) in calls.into_iter().enumerate() {
            let mut arguments = Value::Object(call.arguments);
            let response = match references(&mut arguments, &history) {
                Ok(()) => {
                    // references only replaces nested values; the root started as an object.
                    let arguments = arguments.as_object().cloned()
                        .ok_or("tool arguments must resolve to an object")?;
                    invoke(&runner, &call.tool, arguments).await
                }
                Err(error) => json!({"isError":true,"error":{"message":error}}),
            };
            emit(index, &call.tool, &response)?;
            let failed = response["isError"] == true;
            history.push(response);
            if failed {
                success = false;
                if !continue_on_error { break; }
            }
        }
        if save && success && runner.state().project().is_some() {
            let response = invoke(&runner, "project_save", Map::new()).await;
            emit(history.len(), "project_save", &response)?;
            success = response["isError"] != true;
        }
        if let Some(project) = runner.state().project()
            && project.lock().has_unsaved_changes() {
            eprintln!("foilfern-mcp: project has unsaved changes; use project_save in the batch or --save to persist them.");
            // A successful ephemeral edit must not masquerade as a saved change.
            success = false;
        }
        Ok(success)
    }.await;
    runner.close().await?;
    result
}
