import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import css from './styles.css?raw';
import type { NodeView } from './gen/NodeView';

vi.stubGlobal('document', { documentElement: { dataset: { language: 'en' } }, addEventListener: () => {} });
vi.stubGlobal('navigator', { language: 'en' });
vi.mock('./Terminal', () => ({ navDirection: () => null }));
const { DockSummary, NodeSummary } = await import('./NodeCard');

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
const props = { view, agentLabel: 'Claude Code', rename, actions: <button type="button">menu</button> };

describe('side terminal header in simple mode', () => {
  it('puts the model on the name row before the buttons and hides the summary', () => {
    const out = renderToStaticMarkup(<DockSummary {...props} />);
    expect(out).not.toContain('the report summary');
    expect(out).not.toContain('summary-model');
    const row = out.match(/<div class="summary-row one-row">(.*)<\/div><\/div>$/)?.[1] ?? '';
    expect(row.indexOf('worker')).toBeLessThan(row.indexOf('claude-opus-5-5 · medium'));
    expect(row.indexOf('claude-opus-5-5 · medium')).toBeLessThan(row.indexOf('menu'));
  });

  it('keeps the card summary with its model row and summary text', () => {
    const out = renderToStaticMarkup(<NodeSummary {...props} />);
    expect(out).toContain('<div class="summary-model">');
    expect(out).toContain('the report summary');
  });

  it('keeps the buttons in a 320 px dock while the name editor and two badges are open', () => {
    const rule = (selector: string) => css.split('\n').find((line) => line.startsWith(`${selector} {`)) ?? '';
    const px = (selector: string, prop: string) => Number(rule(selector).match(new RegExp(`[ ;{]${prop}: (\\d+)px`))?.[1] ?? NaN);
    const pad = rule('.dock-head').match(/padding: \d+px (\d+)px \d+px (\d+)px/)!;
    const badge = ':is(.summary-row, .card-head):has(> .rename) > :is(.unread, .chips, .state)';
    expect(rule(badge)).toContain('flex-shrink: 1');
    expect(rule('.summary-row > :not(.name, .rename, .model, .spacer, .unread, .chips, .state)')).toContain('flex-shrink: 0');
    expect(rule('.summary-row.one-row .rename + .model')).toContain('display: none');
    const editing = { ...props, view: { ...view, unread: 1, state: { kind: 'exited', code: 1 } } as NodeView, rename: { ...rename, editing: true } };
    const icon = <button type="button" className="btn icon" />;
    const out = renderToStaticMarkup(<DockSummary {...editing} actions={<>{icon}{icon}</>} />);
    const items = { 'agent-logo': px('.agent-logo', 'width'), rename: px(':is(.summary-row, .card-head) > .rename', 'min-width'), spacer: 0, unread: px(badge, 'min-width'), state: px(badge, 'min-width'), 'btn icon': px('.btn.icon', 'width') };
    let need = 0;
    let count = 0;
    for (const [cls, min] of Object.entries(items)) {
      const n = out.split(`class="${cls}`).length - 1;
      expect(n, cls).toBeGreaterThan(0);
      expect(min, cls).not.toBeNaN();
      need += n * min;
      count += n;
    }
    need += (count - 1) * px('.summary-row', 'gap');
    expect(need).toBeLessThanOrEqual(320 - Number(pad[1]) - Number(pad[2]));
  });
});
