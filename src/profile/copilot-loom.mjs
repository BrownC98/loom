import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { joinSession } from "@github/copilot-sdk/extension";

const child = spawn("loom", ["hook", "copilot"], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
const pending = new Map();
let next = 0;
let state = {};
let session;
let waiting = false;
let reply;
let unacked = [];
let tools = 0;
let interrupt;
const fail = (error) => {
  console.error(`loom: ${error}`);
  for (const resolve of pending.values()) resolve(undefined);
  pending.clear();
};
child.on("error", fail);
child.on("exit", (code) => fail(`hook exited with code ${code}`));
child.stdin.on("error", fail);
child.stderr.on("data", (chunk) => console.error(chunk.toString()));
createInterface({ input: child.stdout }).on("line", (line) => {
  try {
    const answer = JSON.parse(line);
    const request = pending.get(answer.id);
    if (!request) return;
    pending.delete(answer.id);
    if (answer.error) console.error(`loom: ${answer.error}`);
    request(answer.error ? undefined : answer);
  } catch (error) {
    fail(error);
  }
});

function send(event) {
  const id = ++next;
  const answer = new Promise((resolve) => {
    pending.set(id, resolve);
    child.stdin.write(`${JSON.stringify({ protocol: 4, id, ...state, ...event })}\n`);
  });
  return { id, answer };
}

const call = (event) => send(event).answer;

function watch() {
  if (interrupt !== undefined) return;
  const request = send({ event: "interrupt" });
  interrupt = request.id;
  void request.answer.then(async (answer) => {
    if (interrupt === request.id && answer?.interrupted) await session.abort();
  }).catch(fail);
}

function unwatch() {
  if (interrupt === undefined) return;
  const of = interrupt;
  interrupt = undefined;
  void call({ event: "close", of }).catch(fail);
}

function finished() {
  tools = Math.max(0, tools - 1);
  if (!tools) unwatch();
}

async function acknowledge(ids) {
  if (ids.length) await call({ event: "ack", ids });
}

async function context(event) {
  const answer = await call(event);
  if (answer?.ids?.length) unacked.push(answer);
  return answer?.text;
}

async function admit(answer) {
  await session.send({ prompt: answer.text, mode: "immediate", source: "system" });
  await acknowledge(answer.ids ?? []);
}

async function idle(event = "settled") {
  if (waiting) return;
  waiting = true;
  try {
    if (unacked.length) {
      const answers = unacked;
      unacked = [];
      try {
        await admit({ text: answers.map((answer) => answer.text).join("\n\n"), ids: answers.flatMap((answer) => answer.ids) });
      } catch (error) {
        unacked.unshift(...answers);
        throw error;
      }
      return;
    }
    const answer = await call({ event, reply });
    if (answer?.text) await admit(answer);
  } catch (error) {
    console.error(`loom idle: ${error}`);
  } finally {
    waiting = false;
  }
}

session = await joinSession({
  systemMessage: { mode: "append", content: process.env.LOOM_COPILOT_RULES ?? "" },
  hooks: {
    async onUserPromptSubmitted(input) {
      if (input.agentId) return;
      const text = await context({ event: "prompt", prompt: input.prompt });
      if (text) return { additionalContext: text };
    },
    async onPreToolUse(input) {
      if (input.agentId) return;
      const text = await context({ event: "tool_call" });
      if (text) return { permissionDecision: "deny", permissionDecisionReason: text };
      tools++;
      watch();
    },
    async onPostToolUse(input) {
      if (input.agentId) return;
      finished();
      const text = await context({ event: "tool_result" });
      if (text) return { additionalContext: text };
    },
    async onPostToolUseFailure(input) {
      if (input.agentId) return;
      finished();
      const text = await context({ event: "tool_result" });
      if (text) return { additionalContext: text };
    },
  },
});
const model = await session.rpc.model.getCurrent();
state = { transcript: session.sessionId, model: model.modelId, effort: model.reasoningEffort };
session.on((event) => {
  if (event.agentId) return;
  switch (event.type) {
    case "assistant.turn_start": {
      const ids = unacked.flatMap((answer) => answer.ids);
      unacked = [];
      void acknowledge(ids).catch(fail);
      break;
    }
    case "assistant.message":
      reply = event.data.content;
      break;
    case "session.model_change":
      state = { ...state, model: event.data.newModel, effort: event.data.reasoningEffort };
      break;
    case "session.idle":
      tools = 0;
      unwatch();
      void idle();
      break;
    case "session.shutdown":
      child.stdin.end();
      child.kill();
      break;
  }
});
await call({ event: "session_start" });
if (process.env.LOOM_COPILOT_IDLE === "1") void idle("idle");
