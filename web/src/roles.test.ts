import { describe, expect, it } from 'vitest';
import type { AgentView } from './gen/AgentView';
import type { Role } from './gen/Role';
import { blankToNull, canChooseModel, cursorEffortWithoutModel, nameTaken, rolesChanged, runnerText } from './roles';

const agent = (id: string, profile: AgentView['profile']) => ({ id, label: id.toUpperCase(), profile }) as AgentView;
const role = (name: string, over: Partial<Role> = {}): Role => ({ name, duty: '', agent: 'claude', model: null, effort: null, ...over });
const text = { none: 'none', cliDefault: 'CLI default' };

describe('roles', () => {
  it('only claude, codex, cursor, pi and omp profiles take a model choice', () => {
    expect(['claude', 'codex', 'cursor', 'pi', 'omp'].map((p) => canChooseModel(agent(p, p as AgentView['profile'])))).toEqual([true, true, true, true, true]);
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

  it('cursor refuses an effort without a model only', () => {
    const cursor = agent('cursor', 'cursor');
    expect([['', ''], ['m', ''], ['m', 'high'], ['', 'high'], [' ', ' high ']].map(([m, e]) => cursorEffortWithoutModel(cursor, m, e))).toEqual([false, false, false, true, true]);
    expect(cursorEffortWithoutModel(agent('claude', 'claude'), '', 'high')).toBe(false);
  });

  it('a reset, a replaced list or a shorter list closes an open edit; an equal new array does not', () => {
    const defaults = [role('a'), role('b')];
    const custom = [role('x', { duty: 'd' }), ...defaults];
    const seventh = [...defaults, role('c'), role('d'), role('e'), role('f'), role('g')];
    expect(rolesChanged(custom, defaults)).toBe(true);
    expect(rolesChanged(seventh, defaults)).toBe(true);
    expect(rolesChanged(defaults, defaults.map((r) => ({ ...r })))).toBe(false);
  });
});
