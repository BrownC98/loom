import { type ChildProcess, spawn } from "node:child_process";

export type State = { transcript?: string; model?: string; effort?: string };
export type Harness<Context> = {
  state(ctx: Context): State;
  ours(event: Event, ctx: Context): boolean;
  background: string;
};
type Reply = { id?: number; text?: string; error?: string };
type Part = { type: string; text?: string };
export type Message = { role: string; content: string | Part[] };
type Hook = { child: ChildProcess; pending: Map<number, (reply: Reply | undefined) => void>; open: boolean };
type Event = Record<string, unknown>;
type Context = { isIdle(): boolean; signal?: AbortSignal; hasUI: boolean; ui: { notify(text: string, type: "error"): void } };
type Api<C> = {
  registerFlag(name: string, options: { type: "boolean"; default: boolean; description: string }): void;
  getFlag(name: string): unknown;
  on(event: string, handler: (event: any, ctx: C) => unknown): void;
  sendMessage(
    message: { customType: string; content: string; display: boolean },
    options: { triggerTurn: boolean; deliverAs: "steer" },
  ): void;
};

const GUARD = "loom guard";
const MESSAGE = "loom";
const IDLE_FLAG = "loom-idle";
const PROTOCOL = 2;
const RETRY_MS = 250;
const REOPEN_MS = 1000;
const INTERRUPT_POLL_MS = 1000;

function parse(line: string): Reply | undefined {
  try {
    return JSON.parse(line) as Reply;
  } catch {
    return undefined;
  }
}

function open(ctx: Context, signal: AbortSignal): Hook {
  const child = spawn("loom", ["pi-hook"], { stdio: ["pipe", "pipe", "pipe"], signal, windowsHide: true });
  const hook: Hook = { child, pending: new Map(), open: true };
  let out = "";
  let err = "";
  const notify = (text: string) => {
    if (!signal.aborted && ctx.hasUI) ctx.ui.notify(text, "error");
  };
  const end = () => {
    hook.open = false;
    for (const done of hook.pending.values()) done(undefined);
    hook.pending.clear();
  };
  child.stdout?.setEncoding("utf8");
  child.stderr?.setEncoding("utf8");
  child.stdout?.on("data", (chunk: string) => {
    out += chunk;
    for (let at = out.indexOf("\n"); at >= 0; at = out.indexOf("\n")) {
      const reply = parse(out.slice(0, at));
      out = out.slice(at + 1);
      if (reply?.id === undefined) continue;
      if (reply.error) notify(`loom pi-hook: ${reply.error}`);
      hook.pending.get(reply.id)?.(reply.error ? undefined : reply);
      hook.pending.delete(reply.id);
    }
  });
  child.stderr?.on("data", (chunk: string) => (err += chunk));
  child.on("error", (e) => {
    notify(`loom pi-hook: ${e.message}`);
    end();
  });
  child.on("close", (code) => {
    if (code !== 0 && err.trim()) notify(err.trim());
    end();
  });
  child.stdin?.on("error", () => {});
  return hook;
}

export function lastText(messages: Message[]): string | undefined {
  for (const message of [...messages].reverse()) {
    if (message.role !== "assistant") continue;
    const text =
      typeof message.content === "string"
        ? message.content
        : message.content
            .filter((part) => part.type === "text")
            .map((part) => part.text ?? "")
            .join("\n");
    if (text.trim()) return text;
  }
  return undefined;
}

export function connect<C extends Context>(pi: Api<C>, harness: Harness<C>) {
  const shutdown = new AbortController();
  let hook: Hook | undefined;
  let next = 0;
  let waits = 0;
  let running = 0;
  let polling = false;

  pi.registerFlag(IDLE_FLAG, { type: "boolean", default: false, description: "Wait for loom mail when the session starts without a prompt" });

  const call = (input: Record<string, unknown>, ctx: C): Promise<Reply | undefined> => {
    if (shutdown.signal.aborted) return Promise.resolve(undefined);
    if (!hook?.open) {
      hook = open(ctx, shutdown.signal);
    }
    const { child, pending } = hook;
    const id = ++next;
    return new Promise((resolve) => {
      pending.set(id, resolve);
      child.stdin?.write(`${JSON.stringify({ protocol: PROTOCOL, id, ...input, ...harness.state(ctx) })}\n`);
    });
  };

  const deliver = (text: string, ctx: C) => {
    if (shutdown.signal.aborted) return;
    if (ctx.isIdle() || ctx.signal) {
      pi.sendMessage({ customType: MESSAGE, content: text, display: true }, { triggerTurn: true, deliverAs: "steer" });
    } else {
      setTimeout(() => deliver(text, ctx), RETRY_MS);
    }
  };

  const wait = async (event: string, ctx: C, reply?: string) => {
    const id = ++waits;
    const current = () => !shutdown.signal.aborted && id === waits && ctx.isIdle();
    for (;;) {
      const answer = await call({ event, reply }, ctx);
      if (answer?.text) return deliver(answer.text, ctx);
      if (answer || !current()) return;
      await new Promise((resolve) => setTimeout(resolve, REOPEN_MS));
      if (!current()) return;
    }
  };

  pi.on("before_agent_start", async (event: Event & { prompt: string }, ctx) => {
    if (!harness.ours(event, ctx)) return;
    const { text } = (await call({ event: "prompt", prompt: event.prompt }, ctx)) ?? {};
    if (text) return { message: { customType: MESSAGE, content: text, display: true } };
  });

  pi.on("tool_call", async (event: Event & { toolName: string; input: Record<string, unknown> }, ctx) => {
    if (!harness.ours(event, ctx)) return;
    const { text } = (await call({ event: "tool_call" }, ctx)) ?? {};
    if (text) return { block: true, reason: text };
    const { input } = event;
    const command = input.command;
    if (event.toolName === "bash" && typeof command === "string" && !input[harness.background] && !command.startsWith(`${GUARD}\n`)) {
      input.command = `${GUARD}\n${command}`;
      return { input };
    }
  });

  pi.on("tool_result", async (event: Event & { content: Part[]; structuredContent?: unknown }, ctx) => {
    if (!harness.ours(event, ctx)) return;
    const { text } = (await call({ event: "tool_result" }, ctx)) ?? {};
    if (text) return { content: [...event.content, { type: "text", text }], structuredContent: event.structuredContent };
  });

  pi.on("session_shutdown", () => shutdown.abort());

  return {
    async start(ctx: C, idle: boolean) {
      await call({ event: "session_start" }, ctx);
      if (idle || pi.getFlag(IDLE_FLAG) === true) void wait("idle", ctx);
    },
    settled(ctx: C, reply: string | undefined) {
      void wait("settled", ctx, reply);
    },
    async running(ctx: C, stop: () => void) {
      running++;
      if (polling) return;
      polling = true;
      try {
        while (running > 0 && !shutdown.signal.aborted) {
          await new Promise((resolve) => setTimeout(resolve, INTERRUPT_POLL_MS));
          if (running === 0) return;
          if (ctx.isIdle()) {
            running = 0;
            return;
          }
          const { text } = (await call({ event: "tool_call" }, ctx)) ?? {};
          if (text) {
            stop();
            return deliver(text, ctx);
          }
        }
      } finally {
        polling = false;
      }
    },
    finished() {
      running = Math.max(0, running - 1);
    },
  };
}
