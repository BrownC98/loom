import type { CardView } from './gen/CardView';
import type { DefaultCardView } from './gen/DefaultCardView';
import type { NodeView } from './gen/NodeView';

export function baseCardView(view: NodeView, setting: DefaultCardView): CardView {
  return setting === 'rootTerminal' ? (view.parent === null ? 'standard' : 'simple') : setting;
}

export function cardViewOf(view: NodeView, setting: DefaultCardView): CardView {
  return view.cardView ?? baseCardView(view, setting);
}

export function menuSwitch(view: NodeView, setting: DefaultCardView): { next: CardView; clear: boolean } {
  const next = cardViewOf(view, setting) === 'simple' ? 'standard' : 'simple';
  return { next, clear: next === baseCardView(view, setting) };
}
