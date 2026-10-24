import { spawn } from "node:child_process";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

type Reply = { text?: string };
type Part = { type: string; text?: string };
type Message = { role: string; content: string | Part[] };

const GUARD = "loom guard";
const MESSAGE = "loom";
const IDLE_FLAG = "loom-idle";
const RETRY_MS = 250;

function parse(out: string): Reply {
  try {
    return JSON.parse(out) as Reply;
  } catch {
    return {};
  }
}

function call(input: Record<string, unknown>, ctx: ExtensionContext, signal?: AbortSignal): Promise<Reply> {
  return new Promise((resolve) => {
    const child = spawn("loom", ["pi-hook"], { stdio: ["pipe", "pipe", "pipe"], signal, windowsHide: true });
    let out = "";
    let err = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      out += chunk;
      const end = out.indexOf("\n");
      if (end >= 0) resolve(parse(out.slice(0, end)));
    });
    child.stderr.on("data", (chunk: string) => (err += chunk));
    child.on("error", (e) => {
      if (!signal?.aborted && ctx.hasUI) ctx.ui.notify(`loom pi-hook: ${e.message}`, "error");
      resolve({});
    });
    child.on("close", (code) => {
      if (code !== 0 && err.trim() && ctx.hasUI) ctx.ui.notify(err.trim(), "error");
      resolve(parse(out));
    });
    child.stdin.on("error", () => {});
    child.stdin.end(JSON.stringify(input));
  });
}

function state(ctx: ExtensionContext) {
  return {
    transcript: ctx.sessionManager.getSessionFile(),
    model: ctx.model && `${ctx.model.provider}/${ctx.model.id}`,
    effort: ctx.thinkingLevel,
  };
}

function lastText(messages: Message[]): string | undefined {
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

export default function (pi: ExtensionAPI) {
  let lastReply: string | undefined;
  const shutdown = new AbortController();

  pi.registerFlag(IDLE_FLAG, { type: "boolean", default: false, description: "Wait for loom mail when the session starts without a prompt" });

  const deliver = (text: string, ctx: ExtensionContext) => {
    if (shutdown.signal.aborted) return;
    if (ctx.isIdle() || ctx.signal) {
      pi.sendMessage({ customType: MESSAGE, content: text, display: true }, { triggerTurn: true, deliverAs: "steer" });
    } else {
      setTimeout(() => deliver(text, ctx), RETRY_MS);
    }
  };

  const wait = async (event: string, ctx: ExtensionContext, reply?: string) => {
    const { text } = await call({ event, reply, ...state(ctx) }, ctx, shutdown.signal);
    if (text) deliver(text, ctx);
  };

  pi.on("session_start", async (event, ctx) => {
    await call({ event: "session_start", ...state(ctx) }, ctx);
    if (event.reason === "startup" && pi.getFlag(IDLE_FLAG) === true) void wait("idle", ctx);
  });

  pi.on("before_agent_start", async (event, ctx) => {
    if (event.messageType !== "user") return;
    const { text } = await call({ event: "prompt", prompt: event.prompt, ...state(ctx) }, ctx);
    if (text) return { message: { customType: MESSAGE, content: text, display: true } };
  });

  pi.on("tool_call", async (event, ctx) => {
    if (event.parentToolCallId) return;
    const { text } = await call({ event: "tool_call", ...state(ctx) }, ctx);
    if (text) return { block: true, reason: text };
    if (event.toolName === "bash" && typeof event.input.command === "string") {
      event.input.command = `${GUARD}\n${event.input.command}`;
    }
  });

  pi.on("tool_result", async (event, ctx) => {
    if (event.parentToolCallId) return;
    const { text } = await call({ event: "tool_result", ...state(ctx) }, ctx);
    if (text) return { content: [...event.content, { type: "text", text }], structuredContent: event.structuredContent };
  });

  pi.on("agent_end", (event) => {
    lastReply = lastText(event.messages as Message[]);
  });

  pi.on("agent_settled", (event, ctx) => {
    if (event.aborted) return;
    const reply = lastReply;
    lastReply = undefined;
    void wait("settled", ctx, reply);
  });

  pi.on("session_shutdown", () => shutdown.abort());
}
