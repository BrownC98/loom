import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";

export const name = "loom";

const PROTOCOL = 4;
const ENTRY = "dsh";
const IDLE_ENV = "LOOM_DSH_IDLE";
const MODEL_ENV = "LOOM_DSH_MODEL";
const EFFORT_ENV = "LOOM_DSH_EFFORT";
const SHELL_LOOM = "BASH_FUNC_loom%%";
const MODEL_NOTICE = "model-selection";
const LAST = Number.MAX_SAFE_INTEGER;
const INTERRUPT = { kind: "hook", reason: "loom interrupt" };
const APPROVAL_OPTIONS = Object.freeze([
  Object.freeze({ label: "Yes, allow once", reply: "allowed-once" }),
  Object.freeze({ label: "No", reply: "rejected" }),
]);

const parse = (line) => {
  try {
    return JSON.parse(line);
  } catch {
    return undefined;
  }
};

const textOf = (message) =>
  message.content
    .filter((part) => part.type === "text")
    .map((part) => part.text)
    .join("\n");

const loomMessage = (text) =>
  Object.freeze({
    id: randomUUID(),
    role: "user",
    content: Object.freeze([Object.freeze({ type: "text", text })]),
    source: Object.freeze({ kind: "user" }),
  });

const chosenRoute = (model, effort) => {
  const at = model?.indexOf("/") ?? -1;
  return Object.freeze({
    ...(at >= 0 ? { provider: model.slice(0, at), model: model.slice(at + 1) } : model ? { model } : {}),
    ...(effort ? { reasoningEffort: effort } : {}),
  });
};

const routeKey = (route) => `${route.provider}/${route.model}/${route.reasoningEffort}`;

const commandOf = (args) => {
  const parsed = typeof args === "string" ? parse(args) : args;
  return typeof parsed?.command === "string" ? parsed.command : undefined;
};

const top = (agent) => !agent.session.header.origin && !agent.session.header.delegationDepth;

export function apply(ctx, config) {
  const log = ctx.logger("loom");
  const shutdown = new AbortController();
  ctx.effect(() => () => shutdown.abort());
  const token = process.env.LOOM_TOKEN;
  if (token) process.env[SHELL_LOOM] = `() { LOOM_TOKEN=${token} command loom "$@"; }`;
  const idleAtStart = process.env[IDLE_ENV] === "1";
  const chosen = chosenRoute(process.env[MODEL_ENV], process.env[EFFORT_ENV]);
  for (const name of [IDLE_ENV, MODEL_ENV, EFFORT_ENV]) delete process.env[name];
  let steered;
  const ours = new Set();
  const unacked = new Map();
  let hook;
  let next = 0;
  let main;
  let state = {};
  let started = false;
  let reply;
  let ended;
  let watching;
  const commands = new Map();
  const relayed = new WeakSet();
  let asking = Promise.resolve();

  ctx.inject(["systemPrompt"], (inner) =>
    inner.systemPrompt.section({ name: "loom", order: LAST, text: config.usage, interpolate: false }),
  );

  const open = () => {
    const child = spawn("loom", ["hook", ENTRY], { stdio: ["pipe", "pipe", "pipe"], signal: shutdown.signal, windowsHide: true });
    const opened = { child, pending: new Map(), open: true };
    let out = "";
    let err = "";
    const end = () => {
      opened.open = false;
      for (const done of opened.pending.values()) done(undefined);
      opened.pending.clear();
    };
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      out += chunk;
      for (let at = out.indexOf("\n"); at >= 0; at = out.indexOf("\n")) {
        const answer = parse(out.slice(0, at));
        out = out.slice(at + 1);
        if (answer?.id === undefined) continue;
        if (answer.error) log.warn(`loom hook ${ENTRY}: ${answer.error}`);
        opened.pending.get(answer.id)?.(answer.error ? undefined : answer);
        opened.pending.delete(answer.id);
      }
    });
    child.stderr.on("data", (chunk) => (err += chunk));
    child.on("error", (e) => {
      if (!shutdown.signal.aborted) log.warn(`loom hook ${ENTRY}: ${e.message}`);
      end();
    });
    child.on("close", (code) => {
      if (code !== 0 && err.trim()) log.warn(err.trim());
      end();
    });
    child.stdin.on("error", () => {});
    return opened;
  };

  const send = (input) => {
    const id = ++next;
    if (shutdown.signal.aborted) return { id, answer: Promise.resolve(undefined) };
    if (!hook?.open) hook = open();
    const { child, pending } = hook;
    const answer = new Promise((resolve) => {
      pending.set(id, resolve);
      child.stdin.write(`${JSON.stringify({ protocol: PROTOCOL, id, ...input, ...state })}\n`);
    });
    return { id, answer };
  };

  const call = (input) => send(input).answer;

  const handed = (answer) => {
    if (!answer?.text) return undefined;
    const message = loomMessage(answer.text);
    if (answer.ids?.length) unacked.set(message.id, answer.ids);
    return message;
  };

  const marked = (answer) => {
    const message = handed(answer);
    if (message) ours.add(message.id);
    return message;
  };

  const admitted = (message) => {
    const ids = unacked.get(message.id);
    if (!ids) return;
    unacked.delete(message.id);
    void call({ event: "ack", ids });
  };

  const wait = async (event, last) => {
    const answer = await call({ event, reply: last });
    if (!main || shutdown.signal.aborted) return;
    const message = marked(answer);
    if (message) main.followup(message);
  };

  const watch = async () => {
    const { id, answer } = send({ event: "interrupt" });
    watching = id;
    const got = await answer;
    if (watching === id) watching = undefined;
    if (!got?.interrupted || main?.status !== "running") return;
    main.cancel(INTERRUPT, { keepInbox: true });
  };

  const unwatch = () => {
    if (watching === undefined) return;
    const of = watching;
    watching = undefined;
    void call({ event: "close", of });
  };

  const steer = (agent) => {
    let first;
    let steering = true;
    let notices;
    const request = agent.ctx.on(
      "agent/request",
      async (_payload, next) => {
        const resolved = await next();
        first ??= routeKey(resolved);
        steering = first === routeKey(resolved);
        return steering ? { ...resolved, ...chosen } : resolved;
      },
      { prepend: true },
    );
    const quiet = () => {
      notices ??= agent.ctx.on(
        "agent/pre-step",
        async (_payload, next) => {
          const decision = await next();
          if (!steering || decision.kind !== "enter") return decision;
          return { ...decision, messages: decision.messages.filter((m) => m.source?.kind !== MODEL_NOTICE) };
        },
        { prepend: true },
      );
    };
    const end = () => {
      request();
      notices?.();
    };
    return { quiet, end };
  };

  const approval = (req) => {
    const command = req.callId === undefined ? undefined : commands.get(String(req.callId));
    return {
      event: "approval",
      summary: command ?? req.toolName,
      body: [command, req.reason].filter(Boolean),
      options: APPROVAL_OPTIONS,
    };
  };

  ctx.on(
    "approval/request",
    async function (req, next) {
      if (relayed.has(req) || req.agent !== main) return next();
      const local = new AbortController();
      const copy = { ...req, signal: local.signal };
      relayed.add(copy);
      const withdraw = () => local.abort();
      req.signal?.addEventListener("abort", withdraw, { once: true });
      const terminal = ctx.waterfall(this, "approval/request", copy, () => Promise.resolve("unavailable"));
      let ask;
      const inbox = asking.then(() => {
        if (local.signal.aborted) return undefined;
        ask = send(approval(req));
        return ask.answer.then((answer) => answer?.choice);
      });
      asking = inbox.then(
        () => {},
        () => {},
      );
      const outcome = await new Promise((resolve) => {
        terminal.then(resolve, () => resolve("unavailable"));
        inbox.then((choice) => choice && resolve(choice));
      });
      req.signal?.removeEventListener("abort", withdraw);
      local.abort();
      if (ask) void call({ event: "close", of: ask.id });
      return outcome;
    },
    { prepend: true },
  );

  ctx.on("agent/created", async ({ agent }) => {
    if (!top(agent)) return;
    unwatch();
    steered?.end();
    main = agent;
    steered = Object.keys(chosen).length ? steer(agent) : undefined;
    const { provider, model, reasoningEffort } = { ...agent.options, ...chosen };
    state = {
      transcript: String(agent.session.id),
      model: `${provider}/${model}`,
      effort: reasoningEffort,
    };
    await call({ event: "session_start" });
    if (started) return;
    started = true;
    if (idleAtStart) void wait("idle");
  });

  ctx.on("agent/disposed", ({ agent }) => {
    if (agent !== main) return;
    unwatch();
    steered?.end();
    steered = undefined;
    main = undefined;
  });

  ctx.on("session/event", (session, event) => {
    if (String(session.id) !== state.transcript) return;
    switch (event.type) {
      case "request/header": {
        const { provider, model, reasoningEffort } = event.data.header.config;
        state = { ...state, model: `${provider}/${model}`, effort: reasoningEffort };
        break;
      }
      case "user/message":
        admitted(event.data);
        break;
      case "turn/start":
        reply = undefined;
        commands.clear();
        break;
      case "tool/call":
        commands.set(String(event.data.callId), commandOf(event.data.arguments));
        break;
      case "tool/ptc-dispatch-start":
        commands.set(String(event.data.subCallId), commandOf(event.data.arguments));
        break;
      case "assistant/message": {
        const text = event.data.interrupted ? "" : textOf(event.data.message);
        if (text.trim()) reply = text;
        break;
      }
      case "turn/end":
        ended = event.data.reason;
        break;
    }
  });

  ctx.on("agent/status", ({ agent, status }) => {
    if (agent !== main) return;
    if (status === "running") {
      steered?.quiet();
      return void watch();
    }
    unwatch();
    const reason = ended;
    ended = undefined;
    if (!reason) return;
    if (reason.kind !== "aborted") return void wait("settled", reply);
    void wait("idle");
  });

  ctx.on("agent/pre-step", async (payload, proceed) => {
    const decision = await proceed();
    if (payload.agent !== main || decision.kind !== "enter") return decision;
    const human = decision.messages.filter((message) => message.source.kind === "user" && !ours.delete(message.id));
    if (!human.length) return decision;
    const message = handed(await call({ event: "prompt", prompt: human.map(textOf).join("\n\n") }));
    if (!message) return decision;
    return { ...decision, messages: [...decision.messages, message] };
  });

  ctx.on("tools/post-execute", async (exec, _result, proceed) => {
    const decision = await proceed();
    if (exec.agent !== main || exec.parent !== undefined) return decision;
    const message = marked(await call({ event: "tool_result" }));
    if (!message) return decision;
    return { ...decision, additionalContexts: [...(decision.additionalContexts ?? []), message] };
  });
}
