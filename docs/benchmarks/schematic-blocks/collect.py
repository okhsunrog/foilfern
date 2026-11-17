"""Normalize observed runtime counters without double-counting cached input."""

from collections import Counter
import hashlib
import json
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve()
case = root / "case"
metrics = json.loads((case / "metrics.json").read_text())
model = metrics["model"]
turns = list(metrics["turns"])
feedback_path = case / "feedback-metrics.json"
if feedback_path.exists():
    turns.extend(json.loads(feedback_path.read_text())["turns"])
normalized = []
previous = {}
for turn in turns:
    if "response" in turn:
        response = turn["response"]
        entries = response.get("_meta", {}).get("quota", {}).get("model_usage", [])
        if entries:
            if {entry["model"] for entry in entries} != {model}:
                raise ValueError("Unexpected reported model")
            usage = entries[0]["token_count"]
        else:
            usage = response["usage"]
        uncached = usage["inputTokens"]
        cached = usage.get("cachedInputTokens", usage.get("cachedReadTokens", 0))
        written = usage.get("cachedWriteTokens", 0)
        output = usage["outputTokens"]
        total = usage["totalTokens"]
        stop = response.get("stopReason")
        source = "ACP quota.model_usage" if entries else "ACP usage"
    else:
        cumulative = turn["usage"][-1]
        usage = {key: value - previous.get(key, 0) for key, value in cumulative.items()}
        previous = cumulative
        cached = usage.get("cached_input_tokens", 0)
        written = usage.get("cache_write_input_tokens", 0)
        uncached = usage["input_tokens"] - cached - written
        output = usage["output_tokens"]
        total = usage["input_tokens"] + output
        stop = "complete" if turn["exit_code"] == 0 else "failed"
        source = "Codex turn.completed cumulative delta"
    normalized.append(
        {
            "phase": turn["phase"],
            "wall_s": turn["wall_s"],
            "session_wall_s": turn.get("session_wall_s"),
            "input_uncached": uncached,
            "input_cached": cached,
            "input_cache_write": written,
            "output": output,
            "total": total,
            "stop_reason": stop,
            "counter_source": source,
        }
    )
    if total != uncached + cached + written + output:
        raise ValueError("Token counters do not sum")
models = set()
if (root / "codex-state").exists():
    for path in (root / "codex-state/sessions").rglob("*.jsonl"):
        for line in path.read_text().splitlines():
            event = json.loads(line)
            if event.get("type") == "turn_context":
                models.add(event["payload"]["model"])
    events = [
        json.loads(line)
        for line in (case / "task-events.jsonl").read_text().splitlines()
    ]
    kinds = Counter(
        event["item"]["type"] for event in events if event["type"] == "item.completed"
    )
    tool_calls = sum(count for kind, count in kinds.items() if kind != "agent_message")
else:
    for path in (root / "claude-state/projects").rglob("*.jsonl"):
        for line in path.read_text().splitlines():
            event = json.loads(line)
            if event.get("type") == "assistant":
                value = event.get("message", {}).get("model")
                if value and value != "<synthetic>":
                    models.add(value)
    events = [
        json.loads(line) for line in (case / "events.jsonl").read_text().splitlines()
    ]
    task_turn = next(turn for turn in turns if turn["phase"] == "task")
    tool_calls = task_turn.get("tool_calls_so_far")
    kinds = {}
fixture = json.loads((case / "fixture.json").read_text())
result = {
    "model": model,
    "models_in_native_transcript": sorted(models),
    "phases": normalized,
    "builtin_tool_calls": tool_calls,
    "builtin_event_types": dict(kinds),
    "binary_matches_fixture": hashlib.sha256(
        (root / "bin/foilfern-mcp").read_bytes()
    ).hexdigest()
    == fixture["sha256"],
    "fixture": fixture,
}
if (case / "acceptance.json").exists():
    result["acceptance"] = json.loads((case / "acceptance.json").read_text())
(case / "measurements.json").write_text(json.dumps(result, indent=2) + "\n")
print(
    json.dumps(
        {
            key: result[key]
            for key in [
                "model",
                "models_in_native_transcript",
                "phases",
                "builtin_tool_calls",
                "binary_matches_fixture",
            ]
        }
    )
)
if models != {model}:
    raise SystemExit("Unexpected native model")
