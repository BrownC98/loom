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

/** 다시 띄워 같은 세션을 이을 수 있는 노드의 에이전트. 프로필이 있는 에이전트만 데몬이 잇는다. 셸·명령은 `noResume`으로 거절한다. */
/**
 * 프로세스가 살아 있어 다시 띄울 수 있는 상태. 데몬은 프로세스가 사는 노드(`Graph::active`)면 완료한 노드도 다시 띄운다.
 * 완료 노드에서 단추가 빠졌다(2026-10-23 사람 "ktasearch-총괄 터미널에 다시 띄우기 버튼이 안보임").
 */
export const relaunchableState = (view: NodeView) => view.state.kind === 'running' || view.state.kind === 'done';

export function canResume(agents: AgentView[], id: AgentId | null): boolean {
  return agents.some((a) => a.id === id && a.profile !== null);
}
