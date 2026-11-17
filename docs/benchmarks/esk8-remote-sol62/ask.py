"""Ask one follow-up question in the finished trial session (read-only)."""
import json, os, subprocess, sys, time
from pathlib import Path
sys.argv = [sys.argv[0], "sol", "gpt-6.2-sol"] + sys.argv[1:]
src = (Path(__file__).parent / "run-codex.py").read_text()
prefix = src[: src.index("feedback = ")]  # sandbox args, CLI flags and env
exec(prefix)
name, question = sys.argv[3], sys.argv[4]
session = json.loads((case / "metrics.json").read_text())["turns"][-1]["session_id"]
command = args + cli + ["resume", session, "-o", str(case / f"{name}-answer.md"), "-"]
start = time.monotonic()
with (case / f"{name}-events.jsonl").open("w") as out, (case / f"{name}-stderr.log").open("w") as err:
    p = subprocess.run(command, input=question, stdout=out, stderr=err, env=env, text=True, timeout=300)
usage = [json.loads(l).get("usage") for l in (case / f"{name}-events.jsonl").read_text().splitlines() if '"turn.completed"' in l]
print(json.dumps({"exit": p.returncode, "wall_s": round(time.monotonic() - start, 1), "usage": usage}))
print((case / f"{name}-answer.md").read_text())
