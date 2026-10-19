// 카탈로그와 설정을 엮는 규칙. 설정 창과 새 노드 창이 같은 답을 쓴다.
import type { AgentId } from './gen/AgentId';
import type { AgentView } from './gen/AgentView';
import type { SettingsView } from './gen/SettingsView';
import i18next from './i18n';

/** 설치했고 끄지 않은 에이전트. 카탈로그 순서다. */
export function enabledAgents(agents: AgentView[], settings: SettingsView): AgentView[] {
  return agents.filter((a) => a.path !== null && !settings.disabledAgents.includes(a.id));
}

/** 고른 기본 에이전트. 고르지 않았거나 꺼졌으면 켠 것 가운데 첫째다. */
export function defaultAgent(agents: AgentView[], settings: SettingsView): AgentView | undefined {
  const enabled = enabledAgents(agents, settings);
  return enabled.find((a) => a.id === settings.defaultAgent) ?? enabled[0];
}

/** 노드 카드 머리의 글자. 직접 입력한 명령이면 「직접 입력」이다. */
export function agentLabel(agents: AgentView[], id: AgentId | null): string {
  if (id === null) return i18next.t('node.customCommand');
  return agents.find((a) => a.id === id)?.label ?? id;
}
