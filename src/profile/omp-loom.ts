import { connect, lastText, type Message } from "./loom-hook.ts";

type Context = {
  isIdle(): boolean;
  abort(): void;
  hasUI: boolean;
  ui: { notify(text: string, type: "error"): void };
  agent?: { kind: "main" | "sub" };
  model?: { provider: string; id: string };
  sessionManager: { getSessionFile(): string | undefined };
};
type AgentEnd = { messages: (Message & { stopReason?: string })[]; willContinue?: boolean; awaitingAsyncWork?: boolean };
type Api = Parameters<typeof connect<Context>>[0] & { getThinkingLevel(): string | undefined };

const main = (ctx: Context) => ctx.agent?.kind !== "sub";

export default function (pi: Api) {
  const loom = connect(pi, {
    state: (ctx) => ({
      transcript: ctx.sessionManager.getSessionFile(),
      model: ctx.model && `${ctx.model.provider}/${ctx.model.id}`,
      effort: pi.getThinkingLevel(),
    }),
    ours: (_event, ctx) => main(ctx),
    background: "async",
  });

  pi.on("session_start", (_event, ctx) => main(ctx) && loom.start(ctx, false));

  pi.on("tool_execution_start", (_event, ctx) => main(ctx) && void loom.running(ctx, () => ctx.abort()));

  pi.on("tool_execution_end", (_event, ctx) => main(ctx) && loom.finished());

  pi.on("agent_end", (event: AgentEnd, ctx) => {
    if (!main(ctx) || (event.willContinue && !event.awaitingAsyncWork)) return;
    const last = event.messages.findLast((message) => message.role === "assistant");
    if (last?.stopReason === "aborted") return;
    loom.settled(ctx, lastText(event.messages));
  });
}
