# Binary-only schematic trials through ACP

The three tasks in `scenarios.json` cover a vertical divider, an RC filter and
two separate RC channels sharing ground. Each task fixes component identities,
nominals and net memberships. The request supplies no placement coordinates.
The agent receives every tool description, access to every schema and the
binary's starting instructions. See [results.md](results.md) for measured runs.

`run.mjs` controls Claude Code through ACP with the DeepSeek model
`deepseek/deepseek-v4.1-flash-1017`. Each ordinary invocation starts a fresh
session, runs the task and requests a separate workflow-feedback turn. It
records ACP events, responses, task/feedback durations and usage independently.
The script verifies the reported model and fails on an unexpected model.
`collect_metrics.py` recovers per-message usage from the native transcript,
deduplicating assistant message IDs and excluding the feedback turn. Cached
input and uncached input remain separate. Context occupancy is a different
measurement.

## Local prerequisites

Use Linux with `bwrap`, `taskset`, `ori`, Claude Code, Node, `vp` and `uv`.
Configure Ori for the requested model. The measured adapter and SDK versions
were 0.107.0 and 2.7.0. Install those packages with `vp add` in a temporary
adapter directory. Copy `run.mjs` and the executable `claude-via-ori` into that
directory so the runner resolves its local Node dependencies.

Set `FOILFERN_TRIAL_ROOT` to a new directory under
`/home/okhsunrog/tmp_zfs/codex_tmp/`. Set `FOILFERN_SOURCE_ROOT` to the directory
containing the source checkouts to hide. On this host that is
`/home/okhsunrog/code`. The runner hides that tree, the sibling `tmp_zfs` data
and the Codex state, then mounts the trial root and adapter installation. The
remaining host filesystem is read-only. Native project and library `.lp` data
remain available in the fixture.

Create private copies of Claude's settings and global state at
`$FOILFERN_TRIAL_ROOT/claude-state/settings.json` and
`$FOILFERN_TRIAL_ROOT/claude-global.json`. Preserve permission restrictions.
Set the copied settings' `model` to the DeepSeek identifier. The runner also
selects that model explicitly through ACP, chooses manual approval mode and
sets a title before the first prompt. The Ori state directory remains mounted
for its per-launch routing helper. Keep all private state outside Git.

The measured local Claude configuration blocked MCP server registration. The
agent therefore used Bash, Read and Write to execute Foilfern `call` and
`batch`, with an empty ACP MCP server list. The complete Foilfern catalog was
still supplied. These measurements cover that transport.

Prepare an empty native seed with a `Main` sheet, no board and a complete
project library containing the parts in `parts.json`, including all variants'
symbol dependencies. The recorded seed is retained with the local artifacts
listed in the results. The benchmark does not vendor library data.

## Prepare and run

Build the MCP binary in release mode using the repository's normal cache.
Copy the binary into the trial root so the agent can execute it while the
source tree is hidden. The runner restricts itself and descendants to CPU
cores 0 and 1 with nice level 10. The task has a 40-model-turn ceiling, a
360-second session deadline and a USD 0.30 session budget. Feedback-only recovery
uses a separate budget and is excluded from task totals.

Create a Python environment in the trial root with `uv init --bare` and
`uv add mcp`. Use that environment for preparation and independent inspection.
Run preparation from the repository root with those environment variables set.

```sh
uv run --project "$FOILFERN_TRIAL_ROOT" python \
  docs/benchmarks/schematic-acp/prepare.py \
  --root "$FOILFERN_TRIAL_ROOT" \
  --binary "$FOILFERN_TRIAL_ROOT/bin/foilfern-mcp" \
  --seed "$FOILFERN_TRIAL_ROOT/seed" \
  --commit "$(git rev-parse HEAD)" --scenario rc-filter --case rc-filter-1
```

From the temporary adapter directory:

```sh
node run.mjs "$FOILFERN_TRIAL_ROOT/rc-filter-1"
```

From the repository root, use an independent binary to reopen the saved native
project. The measurements used the original release binary for this check.

```sh
export FOILFERN_CHECK_BINARY="$FOILFERN_TRIAL_ROOT/bin/foilfern-check"
uv run --project "$FOILFERN_TRIAL_ROOT" python \
  docs/benchmarks/schematic-acp/inspect_case.py \
  "$FOILFERN_TRIAL_ROOT/rc-filter-1" rc-filter
uv run --project "$FOILFERN_TRIAL_ROOT" python \
  docs/benchmarks/schematic-acp/collect_metrics.py \
  "$FOILFERN_TRIAL_ROOT/rc-filter-1"
```

Inspect `schematic.png` yourself after the native checks. Confirm signal flow,
local physical wiring, readable values and association between captions and
symbols. For the two-channel task, confirm separate named blocks and local
ground connections. Record failures and visual caveats. Repeat in fresh case
directories without editing the agent's project or giving corrective hints.
