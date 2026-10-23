import { describe, expect, it, vi } from 'vitest';

// i18n.ts가 첫 언어를 문서에서 읽는다. 시험은 DOM 없이 돈다.
vi.stubGlobal('document', { documentElement: { dataset: { language: 'en' } } });
vi.stubGlobal('navigator', { language: 'en' });
const { problemText } = await import('./problem');

describe('problemText', () => {
  it('joins a list value', () => {
    const text = problemText({ code: 'refused', value: { code: 'missingHeads', value: ['Goal', 'Facts'] } });
    expect(text).toBe('The brief has no Goal, Facts head. The heads are: Goal, Facts, Scope, Done when, Checks, Time.');
  });
  it('joins a list field', () => {
    const text = problemText({ code: 'refused', value: { code: 'unknownRole', value: { role: 'x', names: ['lead', 'worker'] } } });
    expect(text).toBe("The settings have no role 'x'. The roles are: lead, worker.");
  });
});
