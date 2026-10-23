import type { AgentView } from './gen/AgentView';
import type { Profile } from './gen/Profile';
import type { Role } from './gen/Role';

const MODEL_PROFILES: readonly Profile[] = ['claude', 'codex', 'cursor'];

export const canChooseModel = (agent: AgentView | undefined): boolean => agent === undefined || (agent.profile !== null && MODEL_PROFILES.includes(agent.profile));

export const blankToNull = (text: string): string | null => (text.trim() === '' ? null : text.trim());

export const nameTaken = (name: string, others: readonly Role[]): boolean => name.trim() === '' || others.some((r) => r.name === name.trim());

export function runnerText(role: Role, agents: readonly AgentView[], text: { none: string; cliDefault: string }): string {
  if (role.agent === null) return text.none;
  const label = agents.find((a) => a.id === role.agent)?.label ?? role.agent;
  return [label, role.model ?? text.cliDefault, role.effort ?? text.cliDefault].join(' · ');
}

export const cursorEffortWithoutModel = (agent: AgentView | undefined, model: string, effort: string): boolean => agent?.profile === 'cursor' && blankToNull(effort) !== null && blankToNull(model) === null;

export const rolesChanged = (before: readonly Role[], after: readonly Role[]): boolean => JSON.stringify(before) !== JSON.stringify(after);
