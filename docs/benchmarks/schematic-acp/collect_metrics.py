import json
import os
import sys
from pathlib import Path

root = Path(os.environ["FOILFERN_TRIAL_ROOT"]).resolve()
case = Path(sys.argv[1]).resolve()
metrics = json.loads((case / "metrics.json").read_text())
events = [json.loads(line) for line in (case / "events.jsonl").read_text().splitlines()]
feedback = next(
    (
        e["at"]
        for e in events
        if e.get("type") == "prompt" and e.get("phase") == "feedback"
    ),
    events[-1]["at"],
)
messages = {}
models = set()
for path in (root / "claude-state/projects").glob(
    "*/" + metrics["session_id"] + ".jsonl"
):
    for line in path.read_text().splitlines():
        event = json.loads(line)
        if event.get("type") != "assistant" or (
            feedback and event.get("timestamp", "") >= feedback
        ):
            continue
        message = event["message"]
        models.add(message.get("model") or "unknown")
        if message.get("model") != "deepseek/deepseek-v4.1-flash-1017":
            continue
        messages[message["id"]] = message
usage = {
    key: sum(m.get("usage", {}).get(key, 0) for m in messages.values())
    for key in [
        "input_tokens",
        "output_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ]
}
metrics["task_native_usage"] = usage
metrics["task_api_messages"] = len(messages)
metrics["task_models"] = sorted(models)
(case / "metrics.json").write_text(json.dumps(metrics, indent=2) + "\n")
print(
    json.dumps(
        {
            "case": str(case.relative_to(root)),
            "task": metrics["turns"][:1],
            "usage": usage,
            "messages": len(messages),
        }
    )
)
if models != {"deepseek/deepseek-v4.1-flash-1017"}:
    raise SystemExit(f"Unexpected task models: {models}")
