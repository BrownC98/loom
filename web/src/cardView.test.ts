import { describe, expect, it } from 'vitest';
import { cardViewOf, menuSwitch } from './cardView';
import type { CardView } from './gen/CardView';
import type { NodeView } from './gen/NodeView';

const view = (parent: number | null, cardView: CardView | null = null): NodeView => ({
  id: 2,
  name: 'n',
  agent: null,
  parent,
  state: { kind: 'running' },
  unread: 0,
  model: null,
  effort: null,
  place: { repo: 1, worktree: '/a' },
  bookmark: null,
  cardView,
  reply: null,
  waits: [],
});

describe('cardViewOf', () => {
  it('shows a terminal card only for nodes without a parent under rootTerminal', () => {
    expect(cardViewOf(view(null), 'rootTerminal')).toBe('standard');
    expect(cardViewOf(view(1), 'rootTerminal')).toBe('simple');
  });

  it('follows the default view without a node choice', () => {
    expect(cardViewOf(view(null), 'simple')).toBe('simple');
    expect(cardViewOf(view(1), 'standard')).toBe('standard');
  });

  it('puts the node choice before the default view', () => {
    expect(cardViewOf(view(1, 'standard'), 'rootTerminal')).toBe('standard');
    expect(cardViewOf(view(null, 'simple'), 'standard')).toBe('simple');
  });
});

describe('menuSwitch', () => {
  it('stores the other view when it differs from the default', () => {
    expect(menuSwitch(view(1), 'rootTerminal')).toEqual({ next: 'standard', clear: false });
    expect(menuSwitch(view(null), 'standard')).toEqual({ next: 'simple', clear: false });
  });

  it('clears the node choice when the other view is the default', () => {
    expect(menuSwitch(view(1, 'standard'), 'rootTerminal')).toEqual({ next: 'simple', clear: true });
    expect(menuSwitch(view(null, 'simple'), 'rootTerminal')).toEqual({ next: 'standard', clear: true });
  });

  it('keeps a choice made under an older default and stores the switch back', () => {
    expect(menuSwitch(view(1, 'simple'), 'simple')).toEqual({ next: 'standard', clear: false });
  });
});
