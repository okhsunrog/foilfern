import { client, ndJsonStream } from "@agentclientprotocol/sdk";
import { spawn } from "node:child_process";
import { Readable, Writable } from "node:stream";
import {
  appendFileSync,
  closeSync,
  openSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { resolve } from "node:path";

const adapterRoot = process.cwd();
const trialRoot = resolve(process.env.FOILFERN_TRIAL_ROOT);
const userHome = process.env.HOME;
const hiddenSource = resolve(process.env.FOILFERN_SOURCE_ROOT);
const caseRoot = resolve(process.argv[2]);
const feedbackOnly = process.argv.includes("--feedback-only");
const oldMetrics = feedbackOnly
  ? JSON.parse(readFileSync(resolve(caseRoot, "metrics.json"), "utf8"))
  : null;
const model = "deepseek/deepseek-v4.1-flash-1017";
const eventPath = resolve(
  caseRoot,
  feedbackOnly ? "feedback-events.jsonl" : "events.jsonl",
);
writeFileSync(eventPath, "");
const record = (event) =>
  appendFileSync(
    eventPath,
    JSON.stringify({ at: new Date().toISOString(), ...event }) + "\n",
  );
const errors = openSync(
  resolve(
    caseRoot,
    feedbackOnly ? "feedback-adapter.stderr" : "adapter.stderr",
  ),
  "w",
);
const started = performance.now();
const jailArgs = [
  "--ro-bind",
  "/",
  "/",
  "--tmpfs",
  hiddenSource,
  "--tmpfs",
  resolve(userHome, "tmp_zfs"),
  "--bind",
  trialRoot,
  trialRoot,
  "--ro-bind",
  adapterRoot,
  adapterRoot,
  "--tmpfs",
  resolve(userHome, ".codex"),
  "--bind",
  resolve(trialRoot, "claude-state"),
  resolve(userHome, ".claude"),
  "--bind",
  resolve(trialRoot, "claude-global.json"),
  resolve(userHome, ".claude.json"),
  "--bind",
  resolve(userHome, ".ori"),
  resolve(userHome, ".ori"),
  "--tmpfs",
  "/tmp",
  "--unshare-pid",
  "--proc",
  "/proc",
  "--dev",
  "/dev",
  "--chdir",
  adapterRoot,
  "--",
  "node",
  "node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js",
];
const child = spawn(
  "taskset",
  ["-c", "0,1", "nice", "-n", "10", "bwrap", ...jailArgs],
  {
    cwd: adapterRoot,
    env: {
      ...process.env,
      UV_CACHE_DIR: resolve(trialRoot, "uv-cache"),
      CLAUDE_CODE_EXECUTABLE: resolve(adapterRoot, "claude-via-ori"),
      CLAUDE_AGENT_ACP_PROTOCOL: "v1",
      CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
      CLAUDE_CODE_DISABLE_TERMINAL_TITLE: "1",
      ENABLE_TOOL_SEARCH: "false",
      TMPDIR: resolve(trialRoot, "tmp"),
    },
    stdio: ["pipe", "pipe", errors],
    detached: true,
  },
);
let currentSession;
let agentContext;
const timeout = setTimeout(async () => {
  record({ type: "timeout" });
  if (currentSession && agentContext)
    await agentContext
      .notify("session/cancel", { sessionId: currentSession.sessionId })
      .catch(() => {});
  setTimeout(() => {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {}
  }, 2000);
  process.exitCode = 1;
}, 360_000);
const toolIds = new Set();
const app = client()
  .onNotification("session/update", (ctx) => {
    record({ type: "update", ...ctx.params });
    if (ctx.params.update.sessionUpdate === "tool_call")
      toolIds.add(ctx.params.update.toolCallId);
  })
  .onRequest("session/request_permission", (ctx) => {
    const option = ctx.params.options.find(
      (option) => option.kind === "allow_once",
    );
    record({
      type: "permission",
      tool: ctx.params.toolCall,
      selected: option?.optionId,
    });
    return {
      outcome: option
        ? { outcome: "selected", optionId: option.optionId }
        : { outcome: "cancelled" },
    };
  });
const metrics = {
  model,
  adapter: "0.107.0",
  sdk: "2.7.0",
  catalog_tools: 101,
  builtin_tools: ["Bash", "Read", "Write"],
  transport: "Foilfern CLI over ACP; MCP server blocked by local Claude policy",
  source_access: false,
  turns: [],
};
try {
  await app.connectWith(
    ndJsonStream(Writable.toWeb(child.stdin), Readable.toWeb(child.stdout)),
    async (agent) => {
      agentContext = agent;
      const initialized = await agent.request("initialize", {
        protocolVersion: 1,
        clientCapabilities: {},
        clientInfo: { name: "foilfern-schematic-trial", version: "0.1" },
      });
      record({ type: "initialize", result: initialized });
      const session = await agent
        .buildSession({
          cwd: caseRoot,
          mcpServers: [],
          _meta: {
            claudeCode: {
              options: {
                model,
                ...(feedbackOnly ? { resume: oldMetrics.session_id } : {}),
                tools: { type: "preset", preset: "claude_code" },
                disallowedTools: [
                  "Edit",
                  "Glob",
                  "Grep",
                  "NotebookEdit",
                  "Agent",
                  "Task",
                  "WebFetch",
                  "WebSearch",
                  "ToolSearch",
                  "AskUserQuestion",
                  "Skill",
                ],
                maxTurns: feedbackOnly ? 3 : 40,
                maxBudgetUsd: feedbackOnly ? 1.0 : 0.3,
                promptSuggestions: false,
                env: { ENABLE_TOOL_SEARCH: "false" },
              },
            },
          },
        })
        .start();
      currentSession = session;
      metrics.session_id = session.sessionId;
      metrics.startup_s = (performance.now() - started) / 1000;
      await agent.request("session/set_mode", {
        sessionId: session.sessionId,
        modeId: "default",
      });
      record({
        type: "model_selection",
        result: await agent.request("session/set_config_option", {
          sessionId: session.sessionId,
          configId: "model",
          value: model,
        }),
      });
      await agent.request("session/set_title", {
        sessionId: session.sessionId,
        title: "Foilfern schematic trial",
      });
      const prompts = [
        readFileSync(resolve(caseRoot, "prompt.txt"), "utf8"),
        "Give detailed feedback about this task and the tools. Identify unclear descriptions, errors, workarounds, layout/wiring limitations, visual-review problems and unnecessary calls. Explain what should improve beyond MCP itself. Distinguish observed behavior from hypotheses. Limit feedback to 300 words. Do not edit the project in this feedback turn.",
      ];
      if (feedbackOnly) prompts.shift();
      for (let i = 0; i < prompts.length; i++) {
        const turnStarted = performance.now();
        record({
          type: "prompt",
          phase: i === 0 && !feedbackOnly ? "task" : "feedback",
          text: prompts[i],
        });
        const responsePromise = session.prompt(prompts[i]);
        let text, response;
        try {
          [text, response] = await Promise.all([
            session.readText(),
            responsePromise,
          ]);
        } catch (error) {
          metrics.turns.push({
            phase: i === 0 && !feedbackOnly ? "task" : "feedback",
            wall_s: (performance.now() - turnStarted) / 1000,
            tool_calls_so_far: toolIds.size,
            error: String(error),
          });
          record({ type: "phase_error", error: String(error) });
          throw error;
        }
        const measurement = {
          phase: i === 0 && !feedbackOnly ? "task" : "feedback",
          wall_s: (performance.now() - turnStarted) / 1000,
          tool_calls_so_far: toolIds.size,
          response,
        };
        metrics.turns.push(measurement);
        const actualModels =
          response._meta?.quota?.model_usage?.map((item) => item.model) ?? [];
        if (!actualModels.length || actualModels.some((actual) => actual !== model))
          throw new Error(
            "Unexpected model in measured turn: " + actualModels.join(", "),
          );
        writeFileSync(
          resolve(
            caseRoot,
            i === 0 && !feedbackOnly ? "answer.md" : "feedback.md",
          ),
          text,
        );
        writeFileSync(
          resolve(
            caseRoot,
            feedbackOnly ? "feedback-metrics.json" : "metrics.json",
          ),
          JSON.stringify(metrics, null, 2) + "\n",
        );
        console.log(JSON.stringify(measurement));
        if (i === 0 && !feedbackOnly && toolIds.size === 0)
          throw new Error(
            "No actual tool calls; harness or tool availability failed",
          );
        if (response.stopReason !== "end_turn") break;
      }
      session.dispose();
    },
  );
} finally {
  metrics.total_wall_s = (performance.now() - started) / 1000;
  writeFileSync(
    resolve(caseRoot, feedbackOnly ? "feedback-metrics.json" : "metrics.json"),
    JSON.stringify(metrics, null, 2) + "\n",
  );
  clearTimeout(timeout);
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch {}
  closeSync(errors);
}
