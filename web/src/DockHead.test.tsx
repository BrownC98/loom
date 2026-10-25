import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import type { NodeView } from './gen/NodeView';

vi.stubGlobal('document', { documentElement: { dataset: { language: 'en' } }, addEventListener: () => {} });
vi.stubGlobal('navigator', { language: 'en' });
vi.mock('./Terminal', () => ({ navDirection: () => null }));
const { NodeSummary } = await import('./NodeCard');

const view: NodeView = {
  id: 1,
  name: 'worker',
  agent: 'claude',
  parent: 2,
  state: { kind: 'running' },
  unread: 0,
  model: 'claude-opus-5-5',
  effort: 'medium',
  place: { repo: 1, worktree: '/repo/wt' },
  bookmark: null,
  cardView: null,
  reply: 'done: the report summary',
  waits: [],
};
const rename = { editing: false, open: () => {}, close: () => {}, refused: () => {} };
const render = (oneRow: boolean) => renderToStaticMarkup(<NodeSummary view={view} agentLabel="Claude Code" rename={rename} oneRow={oneRow} actions={<button type="button">menu</button>} />);

describe('side terminal header in simple mode', () => {
  it('puts the model on the name row before the buttons and hides the summary', () => {
    const out = render(true);
    expect(out).not.toContain('the report summary');
    expect(out).not.toContain('summary-model');
    const row = out.match(/<div class="summary-row one-row">(.*)<\/div><\/div>$/)?.[1] ?? '';
    expect(row.indexOf('worker')).toBeLessThan(row.indexOf('claude-opus-5-5 · medium'));
    expect(row.indexOf('claude-opus-5-5 · medium')).toBeLessThan(row.indexOf('menu'));
  });

  it('keeps the card summary with its model row and summary text', () => {
    const out = render(false);
    expect(out).toContain('<div class="summary-model">');
    expect(out).toContain('the report summary');
  });
});
