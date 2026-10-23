import { describe, expect, it } from 'vitest';
import type { AgentView } from './gen/AgentView';
import type { Role } from './gen/Role';
import { blankToNull, canChooseModel, nameTaken, runnerText } from './roles';

const agent = (id: string, profile: AgentView['profile']) => ({ id, label: id.toUpperCase(), profile }) as AgentView;
const role = (name: string, over: Partial<Role> = {}): Role => ({ name, duty: '', agent: 'claude', model: null, effort: null, ...over });
const text = { none: 'none', cliDefault: 'CLI default' };

describe('roles', () => {
  it('only claude, codex and cursor profiles take a model choice', () => {
    expect(['claude', 'codex', 'cursor'].map((p) => canChooseModel(agent(p, p as AgentView['profile'])))).toEqual([true, true, true]);
    expect(canChooseModel(agent('opencode2', 'opencode2'))).toBe(false);
    expect(canChooseModel(agent('gemini', null))).toBe(false);
    expect(canChooseModel(undefined)).toBe(true);
  });

  it('empty text becomes null and text is trimmed', () => {
    expect(blankToNull('  ')).toBeNull();
    expect(blankToNull(' high ')).toBe('high');
  });

  it('empty or duplicate names are taken', () => {
    const others = [role('cody')];
    expect(nameTaken('', others)).toBe(true);
    expect(nameTaken(' cody ', others)).toBe(true);
    expect(nameTaken('worker', others)).toBe(false);
  });

  it('runner line shows CLI default for empty model and effort, none for no agent', () => {
    const agents = [agent('claude', 'claude')];
    expect(runnerText(role('a', { model: 'm', effort: 'high' }), agents, text)).toBe('CLAUDE · m · high');
    expect(runnerText(role('a'), agents, text)).toBe('CLAUDE · CLI default · CLI default');
    expect(runnerText(role('a', { agent: null }), agents, text)).toBe('none');
  });
});
