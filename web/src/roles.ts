import type { AgentView } from './gen/AgentView';
import type { Role } from './gen/Role';

export const canChooseModel = (agent: AgentView | undefined): boolean => agent === undefined || agent.modelRule !== 'fixed';

export const blankToNull = (text: string): string | null => (text.trim() === '' ? null : text.trim());

export const nameTaken = (name: string, others: readonly Role[]): boolean => name.trim() === '' || others.some((r) => r.name === name.trim());

export function runnerText(role: Role, agents: readonly AgentView[], text: { none: string; cliDefault: string }): string {
  if (role.agent === null) return text.none;
  const label = agents.find((a) => a.id === role.agent)?.label ?? role.agent;
  return [label, role.model ?? text.cliDefault, role.effort ?? text.cliDefault].join(' · ');
}

export const effortWithoutModel = (agent: AgentView | undefined, model: string, effort: string): boolean => agent?.modelRule === 'effortNeedsModel' && blankToNull(effort) !== null && blankToNull(model) === null;

export const rolesChanged = (before: readonly Role[], after: readonly Role[]): boolean => JSON.stringify(before) !== JSON.stringify(after);
