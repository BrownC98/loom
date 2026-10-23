import type { AgentId } from './gen/AgentId';
import type { AgentView } from './gen/AgentView';
import type { NodeView } from './gen/NodeView';
import type { SettingsView } from './gen/SettingsView';
import i18next from './i18n';

export function canBypass(agent: AgentView, settings: SettingsView): boolean {
  return agent.canBypass && !settings.bypassOff.includes(agent.id);
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

export const relaunchableState = (view: NodeView) => view.state.kind === 'running' || view.state.kind === 'done';

export function canResume(agents: AgentView[], id: AgentId | null): boolean {
  return agents.some((a) => a.id === id && a.profile !== null);
}
