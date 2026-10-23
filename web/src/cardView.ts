// 노드 카드 보기를 정하는 규칙 한 곳(목업 docs/mockups/node-card-view.html). 그래프, 오른쪽 칸, 「⋯」 메뉴가 함께 쓴다.
import type { CardView } from './gen/CardView';
import type { DefaultCardView } from './gen/DefaultCardView';
import type { NodeView } from './gen/NodeView';

/** 노드에 고른 값이 없을 때의 보기. 「총괄만 터미널」이면 부모 없는 노드만 표준이다. */
export function baseCardView(view: NodeView, setting: DefaultCardView): CardView {
  return setting === 'rootTerminal' ? (view.parent === null ? 'standard' : 'simple') : setting;
}

/** 이 노드가 보이는 카드. 「⋯」 메뉴로 고른 값이 기본 보기보다 앞선다. */
export function cardViewOf(view: NodeView, setting: DefaultCardView): CardView {
  return view.cardView ?? baseCardView(view, setting);
}

/** 「⋯」 메뉴 첫 줄이 고르는 보기. `next`가 기본 보기와 같으면 노드의 값을 지워(`clear`) 다시 기본을 따르게 한다. */
export function menuSwitch(view: NodeView, setting: DefaultCardView): { next: CardView; clear: boolean } {
  const next = cardViewOf(view, setting) === 'simple' ? 'standard' : 'simple';
  return { next, clear: next === baseCardView(view, setting) };
}
