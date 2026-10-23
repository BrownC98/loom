import type { AgentView } from './gen/AgentView';
import type { Profile } from './gen/Profile';
import type { Role } from './gen/Role';

/** 모델·강도를 CLI 인자로 받는 에이전트의 프로필. OpenCode 2는 프로필이 있어도 받지 않는다(목업 role-settings.html ②). */
const MODEL_PROFILES: readonly Profile[] = ['claude', 'codex', 'cursor'];

/** 카탈로그에 없는 에이전트(목록을 받기 전, 카탈로그에서 빠진 id)는 칸을 켜 둔다. 끄면 저장할 때 모델·강도가 지워진다. */
export const canChooseModel = (agent: AgentView | undefined): boolean => agent === undefined || (agent.profile !== null && MODEL_PROFILES.includes(agent.profile));

/** 칸의 글 → 저장값. 비면 `null`(CLI 기본). */
export const blankToNull = (text: string): string | null => (text.trim() === '' ? null : text.trim());

/** 이름이 비었거나 `others`(편집 중인 역할을 뺀 목록)와 겹치면 `true`. */
export const nameTaken = (name: string, others: readonly Role[]): boolean => name.trim() === '' || others.some((r) => r.name === name.trim());

/** 줄의 「실행자 · 모델 · 강도」. `agent`가 `null`이면 `none`이다. */
export function runnerText(role: Role, agents: readonly AgentView[], text: { none: string; cliDefault: string }): string {
  if (role.agent === null) return text.none;
  const label = agents.find((a) => a.id === role.agent)?.label ?? role.agent;
  return [label, role.model ?? text.cliDefault, role.effort ?? text.cliDefault].join(' · ');
}

/** Cursor는 모델 없이 추론 강도만 받지 못한다(`src/profile/cursor.rs`). 칸의 글 기준이다. */
export const cursorEffortWithoutModel = (agent: AgentView | undefined, model: string, effort: string): boolean => agent?.profile === 'cursor' && blankToNull(effort) !== null && blankToNull(model) === null;

/** 설정에서 온 역할 목록이 바뀌었는지(내용 기준). 같은 내용의 새 배열은 바뀐 것이 아니다. */
export const rolesChanged = (before: readonly Role[], after: readonly Role[]): boolean => JSON.stringify(before) !== JSON.stringify(after);
