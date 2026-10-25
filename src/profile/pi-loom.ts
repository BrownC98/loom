import { type ChildProcess, spawn } from "node:child_process";
import { closeSync, mkdirSync, openSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  createBashToolDefinition,
  DefaultPackageManager,
  getShellConfig,
  type ExtensionAPI,
  type ExtensionContext,
  getAgentDir,
  loadSkills,
  SettingsManager,
} from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

type Reply = { id?: number; text?: string; error?: string };
type Part = { type: string; text?: string };
type Message = { role: string; content: string | Part[] };
type Hook = { child: ChildProcess; pending: Map<number, (reply: Reply | undefined) => void>; open: boolean };

const GUARD = "loom guard";
const MESSAGE = "loom";
const IDLE_FLAG = "loom-idle";
const PROTOCOL = 2;
const RETRY_MS = 250;
const REOPEN_MS = 1000;
const BASH_TIMEOUT_S = 1200;
let started = 0;
const SKILLS = join(dirname(fileURLToPath(import.meta.url)), "skills");

function parse(line: string): Reply | undefined {
  try {
    return JSON.parse(line) as Reply;
  } catch {
    return undefined;
  }
}

function open(ctx: ExtensionContext, signal: AbortSignal): Hook {
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
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => {
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
  child.stderr.on("data", (chunk: string) => (err += chunk));
  child.on("error", (e) => {
    notify(`loom pi-hook: ${e.message}`);
    end();
  });
  child.on("close", (code) => {
    if (code !== 0 && err.trim()) notify(err.trim());
    end();
  });
  child.stdin.on("error", () => {});
  return hook;
}

async function background(command: string, cwd: string, shell: { shell: string; args: string[]; commandTransport?: string }) {
  const dir = join(process.env.LOOM_TASK_DIR || tmpdir(), "logs");
  mkdirSync(dir, { recursive: true });
  const log = join(dir, `background-${Date.now()}-${++started}.log`);
  const fd = openSync(log, "a");
  const stdin = shell.commandTransport === "stdin";
  const child = spawn(shell.shell, stdin ? shell.args : [...shell.args, command], {
    cwd,
    detached: true,
    stdio: [stdin ? "pipe" : "ignore", fd, fd],
    windowsHide: true,
  });
  closeSync(fd);
  if (stdin) {
    child.stdin?.on("error", () => {});
    child.stdin?.end(command);
  }
  await new Promise((resolve, reject) => {
    child.once("spawn", resolve);
    child.once("error", (e) => reject(new Error(`Could not start the background command: ${e.message}`)));
  });
  child.unref();
  const text = `Started in the background: pid ${child.pid}, output in ${log}. Stop it with: kill -- -${child.pid}`;
  return { content: [{ type: "text" as const, text }], details: undefined };
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

async function otherSkills(cwd: string, projectTrusted: boolean): Promise<string[]> {
  const agentDir = getAgentDir();
  const settingsManager = SettingsManager.create(cwd, agentDir, { projectTrusted });
  const { skills } = await new DefaultPackageManager({ cwd, agentDir, settingsManager }).resolve();
  const found = (skillPaths: string[]) => loadSkills({ cwd, agentDir, skillPaths, includeDefaults: false }).skills;
  const loom = new Set(found([SKILLS]).map((skill) => skill.name));
  const paths = skills.filter((skill) => skill.enabled).map((skill) => skill.path);
  return found(paths)
    .filter((skill) => !loom.has(skill.name))
    .map((skill) => skill.filePath);
}

export default function (pi: ExtensionAPI) {
  let lastReply: string | undefined;
  const shutdown = new AbortController();
  let hook: Hook | undefined;
  let next = 0;
  let waits = 0;

  pi.registerFlag(IDLE_FLAG, { type: "boolean", default: false, description: "Wait for loom mail when the session starts without a prompt" });

  const bash = createBashToolDefinition(process.cwd());
  pi.registerTool({
    ...bash,
    description: `${bash.description} Set run_in_background to true for a process that keeps running, such as a server: it starts detached, writes its output to a log file and returns at once. A foreground command without timeout stops after ${BASH_TIMEOUT_S} seconds.`,
    parameters: Type.Object({
      ...bash.parameters.properties,
      run_in_background: Type.Optional(Type.Boolean({ description: "Start the command detached and return at once" })),
    }),
    async execute(id, params, signal, onUpdate, ctx) {
      const settings = SettingsManager.create(ctx.cwd, getAgentDir(), { projectTrusted: ctx.isProjectTrusted() });
      const commandPrefix = settings.getShellCommandPrefix();
      const shellPath = settings.getShellPath();
      if (params.run_in_background) {
        const command = commandPrefix ? `${commandPrefix}\n${params.command}` : params.command;
        return background(command, ctx.cwd, getShellConfig(shellPath));
      }
      try {
        const shell = createBashToolDefinition(ctx.cwd, { commandPrefix, shellPath });
        return await shell.execute(id, { command: params.command, timeout: params.timeout ?? BASH_TIMEOUT_S }, signal, onUpdate, ctx);
      } catch (e) {
        if (e instanceof Error && e.message.includes("timed out")) {
          throw new Error(`${e.message}. Start a process that keeps running with run_in_background: true.`);
        }
        throw e;
      }
    },
  });

  const call = (input: Record<string, unknown>, ctx: ExtensionContext): Promise<Reply | undefined> => {
    if (shutdown.signal.aborted) return Promise.resolve(undefined);
    if (!hook?.open) {
      hook = open(ctx, shutdown.signal);
    }
    const { child, pending } = hook;
    const id = ++next;
    return new Promise((resolve) => {
      pending.set(id, resolve);
      child.stdin.write(`${JSON.stringify({ protocol: PROTOCOL, id, ...input })}\n`);
    });
  };

  const deliver = (text: string, ctx: ExtensionContext) => {
    if (shutdown.signal.aborted) return;
    if (ctx.isIdle() || ctx.signal) {
      pi.sendMessage({ customType: MESSAGE, content: text, display: true }, { triggerTurn: true, deliverAs: "steer" });
    } else {
      setTimeout(() => deliver(text, ctx), RETRY_MS);
    }
  };

  const wait = async (event: string, ctx: ExtensionContext, reply?: string) => {
    const id = ++waits;
    const current = () => !shutdown.signal.aborted && id === waits && ctx.isIdle();
    for (;;) {
      const answer = await call({ event, reply, ...state(ctx) }, ctx);
      if (answer?.text) return deliver(answer.text, ctx);
      if (answer || !current()) return;
      await new Promise((resolve) => setTimeout(resolve, REOPEN_MS));
      if (!current()) return;
    }
  };

  pi.on("resources_discover", async (event, ctx) => ({ skillPaths: await otherSkills(event.cwd, ctx.isProjectTrusted()) }));

  pi.on("session_start", async (event, ctx) => {
    await call({ event: "session_start", ...state(ctx) }, ctx);
    if (event.reason !== "startup" || pi.getFlag(IDLE_FLAG) === true) void wait("idle", ctx);
  });

  pi.on("before_agent_start", async (event, ctx) => {
    if (event.messageType !== "user") return;
    const { text } = (await call({ event: "prompt", prompt: event.prompt, ...state(ctx) }, ctx)) ?? {};
    if (text) return { message: { customType: MESSAGE, content: text, display: true } };
  });

  pi.on("tool_call", async (event, ctx) => {
    if (event.parentToolCallId) return;
    const { text } = (await call({ event: "tool_call", ...state(ctx) }, ctx)) ?? {};
    if (text) return { block: true, reason: text };
    const input = event.input as { command?: unknown; run_in_background?: unknown };
    if (event.toolName === "bash" && typeof input.command === "string" && !input.run_in_background) {
      input.command = `${GUARD}\n${input.command}`;
    }
  });

  pi.on("tool_result", async (event, ctx) => {
    if (event.parentToolCallId) return;
    const { text } = (await call({ event: "tool_result", ...state(ctx) }, ctx)) ?? {};
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
