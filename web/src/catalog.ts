// 카탈로그와 설정을 엮는 규칙. 설정 창과 새 노드 창이 같은 답을 쓴다.
import type { AgentId } from './gen/AgentId';
import type { AgentView } from './gen/AgentView';
import type { NodeView } from './gen/NodeView';
import type { SettingsView } from './gen/SettingsView';
import i18next from './i18n';

/** 모든 권한 허용으로 뜬다: 카탈로그에 인자가 있고 설정에서 끄지 않았다. */
export function canBypass(agent: AgentView, settings: SettingsView): boolean {
  return agent.canBypass && !settings.bypassOff.includes(agent.id);
}

/** 사람이 마지막으로 띄운 노드(부모 없는 노드 가운데 id가 가장 큰 것)의 에이전트. 노드가 없으면 `undefined`다. */
export function recentAgent(views: Iterable<NodeView>): AgentId | null | undefined {
  let last: NodeView | undefined;
  for (const v of views) if (v.parent === null && (!last || v.id > last.id)) last = v;
  return last?.agent;
}

/** 노드 카드 머리의 글자. 에이전트 없이 띄운 셸·명령이면 「터미널」이다. */
export function agentLabel(agents: AgentView[], id: AgentId | null): string {
  if (id === null) return i18next.t('node.terminal');
  return agents.find((a) => a.id === id)?.label ?? id;
}
