import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import type { NodeView } from './gen/NodeView';

vi.stubGlobal('document', { documentElement: { dataset: { language: 'en' } }, addEventListener: () => {} });
vi.stubGlobal('navigator', { language: 'en' });
vi.mock('./Terminal', () => ({ navDirection: () => null }));
const { AgentLine } = await import('./NodeCard');
const { NodeTitle } = await import('./Phone');

const node = (over: Partial<NodeView>): NodeView => ({
  id: 1,
  name: 'worker',
  agent: 'claude',
  parent: null,
  state: { kind: 'running' },
  unread: 0,
  model: null,
  effort: null,
  place: { repo: 1, worktree: '/repo/wt' },
  bookmark: null,
  cardView: null,
  reply: null,
  waits: [],
  ...over,
});

describe('phone terminal header subtitle', () => {
  const cases: [string, NodeView, string, string][] = [
    ['claude', node({ model: 'claude-opus-5-5', effort: 'medium' }), 'Claude Code', 'claude-opus-5-5 · medium'],
    ['codex without a model', node({ agent: 'codex' }), 'Codex', 'Codex'],
    ['shell', node({ agent: null }), 'Terminal', 'Terminal'],
  ];
  it.each(cases)('is the node list line 2 for %s', (_, view, label, text) => {
    const out = renderToStaticMarkup(<NodeTitle view={view} agentLabel={label} />);
    const line = renderToStaticMarkup(<AgentLine view={view} label={label} />);
    expect(out).toBe(`<span class="phone-title"><span class="phone-name"><b>worker</b></span><span class="node-line">${line}</span></span>`);
    expect(line).toContain(`<span class="sub" translate="no">${text}</span>`);
  });

  it('puts notice chips and the exit state on the name row, not beside the subtitle', () => {
    const view = node({ state: { kind: 'exited', code: 1 }, waits: [{ id: 1, kind: 'auth', text: '', since: 0, answerable: false, doc: null, images: [] }] });
    const out = renderToStaticMarkup(<NodeTitle view={view} agentLabel="Claude Code" />);
    const [name, line] = out.split('<span class="node-line">');
    expect(name).toContain('class="chips');
    expect(name).toContain('class="state exited"');
    expect(line).not.toContain('chips');
  });
});
