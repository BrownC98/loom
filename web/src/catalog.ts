import type { AgentId } from './gen/AgentId';
import type { AgentView } from './gen/AgentView';
import type { NodeView } from './gen/NodeView';
import i18next from './i18n';

export function canBypass(agent: AgentView, bypassOff: readonly AgentId[]): boolean {
  return agent.canBypass && !bypassOff.includes(agent.id);
}

export function recentAgent(views: Iterable<NodeView>): AgentId | null | undefined {
  let last: NodeView | undefined;
  for (const v of views) if (v.parent === null && (!last || v.id > last.id)) last = v;
  return last?.agent;
}

export function agentLabel(agents: AgentView[], id: AgentId | null): string {
  if (id === null) return i18next.t('node.terminal');
  return agents.find((a) => a.id === id)?.label ?? id;
}
