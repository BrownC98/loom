import { spawn } from "node:child_process";
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
import { connect, lastText, type Message } from "./loom-hook.ts";

const BASH_TIMEOUT_S = 1200;
let started = 0;
const SKILLS = join(dirname(fileURLToPath(import.meta.url)), "skills");

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
  const loom = connect(pi, {
    state: (ctx: ExtensionContext) => ({
      transcript: ctx.sessionManager.getSessionFile(),
      model: ctx.model && `${ctx.model.provider}/${ctx.model.id}`,
      effort: ctx.thinkingLevel,
    }),
    ours: (event) => !event.parentToolCallId && (event.messageType ?? "user") === "user",
    background: "run_in_background",
  });

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

  pi.on("resources_discover", async (event, ctx) => ({ skillPaths: await otherSkills(event.cwd, ctx.isProjectTrusted()) }));

  pi.on("session_start", (event, ctx) => loom.start(ctx, event.reason !== "startup"));

  pi.on("agent_end", (event) => {
    lastReply = lastText(event.messages as Message[]);
  });

  pi.on("agent_settled", (event, ctx) => {
    if (event.aborted) return;
    const reply = lastReply;
    lastReply = undefined;
    loom.settled(ctx, reply);
  });
}
