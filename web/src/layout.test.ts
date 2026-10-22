import { describe, expect, it } from 'vitest';
import { arrangeNested, centerSide, crossed, edgeSides, followCard, freeSpot, GROUP_GAP, overlaps, push, settle, stepAt, swap, TREE_SIDES, unpile, type ArrangeDirection, type Box, type NestGroup, type Size, type Swap } from './layout';

const box = (x: number, y: number, width = 100, height = 50): Box => ({ at: { x, y }, size: { width, height } });

describe('edgeSides', () => {
  const parent = box(0, 0);
  const directions: ArrangeDirection[] = ['down', 'up', 'right', 'left'];
  const ahead: Record<ArrangeDirection, Box> = {
    down: box(0, 200),
    up: box(0, -200),
    right: box(300, 0),
    left: box(-300, 0),
  };

  it.each(directions)('child ahead of the parent uses TREE_SIDES[%s]', (direction) => {
    expect(edgeSides(parent, ahead[direction], direction)).toEqual(TREE_SIDES[direction]);
  });

  it('child touching the parent edge still counts as ahead', () => {
    expect(edgeSides(parent, box(0, 50), 'down')).toEqual(TREE_SIDES.down);
  });

  it('child not ahead: the axis with the larger empty gap wins', () => {
    // direction down, but the child sits far to the right with a small vertical gap.
    expect(edgeSides(parent, box(400, 20), 'down')).toEqual(TREE_SIDES.right);
    // far to the left.
    expect(edgeSides(parent, box(-400, 20), 'down')).toEqual(TREE_SIDES.left);
    // direction right, but the child sits far above.
    expect(edgeSides(parent, box(20, -400), 'right')).toEqual(TREE_SIDES.up);
    // far below.
    expect(edgeSides(parent, box(20, 400), 'right')).toEqual(TREE_SIDES.down);
  });

  it('horizontally overlapping cards use the vertical axis, not the center distance', () => {
    // centers are 300 apart horizontally and 70 vertically, but the cards overlap in x: the only empty gap is vertical.
    expect(edgeSides(box(0, 0, 400, 50), box(300, 70, 400, 50), 'right')).toEqual(TREE_SIDES.down);
  });
});

type N = { id: number; parent: number | null; group: number; size?: Size };
const n = (id: number, parent: number | null, group: number, size?: Size): N => ({ id, parent, group, size });
const CARD: Size = { width: 600, height: 466 };
const EMPTY: Size = { width: 320, height: 100 };

/**
 * 그래프 자동 배치(Graph.tsx)와 같은 입력으로 `arrangeNested`를 돌린다. 카드 600×466, 묶음 머리 60, 옆 20.
 * 묶음의 부모는 그 묶음 노드 가운데 부모가 다른 묶음에 있는 첫 노드의 부모 카드다(repos.ts `groupParents`·`attachCards`). `groups`는 묶음 수다.
 */
function family(nodes: readonly N[], groups: number, direction: ArrangeDirection = 'down') {
  const groupOf = new Map(nodes.map((x) => [x.id, x.group]));
  const input = Array.from({ length: groups }, (_, g): NestGroup => {
    const hit = nodes.find((x) => x.group === g && x.parent !== null && groupOf.get(x.parent) !== g);
    return { parent: hit ? groupOf.get(hit.parent!)! : null, card: hit ? hit.parent : null, members: nodes.filter((x) => x.group === g).map((x) => x.id), empty: EMPTY };
  });
  const size = (id: number) => nodes.find((x) => x.id === id)!.size ?? CARD;
  const laid = arrangeNested(input, (id) => nodes.find((x) => x.id === id)!.parent, size, direction, { top: 60, side: 20 });
  const cards = new Map(nodes.map((x): [number, Box] => [x.id, { at: laid.cards.get(x.id)!, size: size(x.id) }]));
  return { cards, boxes: laid.boxes, input };
}

const contains = (outer: Box, inner: Box) =>
  inner.at.x >= outer.at.x && inner.at.y >= outer.at.y && inner.at.x + inner.size.width <= outer.at.x + outer.size.width && inner.at.y + inner.size.height <= outer.at.y + outer.size.height;
const center = (b: Box) => b.at.x + b.size.width / 2;
/** 목업 docs/mockups/arrange-family.html v4 ①에 자식 없는 뿌리 규칙(docs/mockups/arrange-root.html v2 ④)을 더한 값. 카드 1은 첫 줄에서 카드 2 왼쪽에 선다. */
const PERSON_CARDS = [{ x: 92, y: 60 }, { x: 712, y: 60 }, { x: 376, y: 638 }, { x: 1028, y: 638 }, { x: 40, y: 1216 }, { x: 712, y: 1216 }];
const PERSON_BOXES = [[0, 0, 1668, 1722], [1008, 578, 640, 546], [20, 1156, 640, 546], [692, 1156, 640, 546]];

/** 나무 간선(계단)의 꼭짓점. 부모 변 가운데 → 꺾는 자리 → 자식 변 가운데. */
function stepPath(parent: Box, child: Box, direction: ArrangeDirection, bend: number) {
  const step = stepAt(parent, child, direction, bend);
  if (step === null) return null;
  const c = (b: Box) => ({ x: b.at.x + b.size.width / 2, y: b.at.y + b.size.height / 2 });
  const [p, q] = [c(parent), c(child)];
  if (direction === 'down' || direction === 'up') {
    const sign = direction === 'down' ? 1 : -1;
    const a = p.y + (sign * parent.size.height) / 2;
    const b = q.y - (sign * child.size.height) / 2;
    const at = a + (b - a) * step;
    return [{ x: p.x, y: a }, { x: p.x, y: at }, { x: q.x, y: at }, { x: q.x, y: b }];
  }
  const sign = direction === 'right' ? 1 : -1;
  const a = p.x + (sign * parent.size.width) / 2;
  const b = q.x - (sign * child.size.width) / 2;
  const at = a + (b - a) * step;
  return [{ x: a, y: p.y }, { x: at, y: p.y }, { x: at, y: q.y }, { x: b, y: q.y }];
}

describe('arrangeNested catalogue (docs/mockups/arrange-family.html v4)', () => {
  const base = [n(1, null, 0), n(2, null, 0), n(3, null, 0), n(4, 2, 0), n(5, 2, 1)];
  const person = [n(1, null, 0), n(2, null, 0), n(3, 2, 0), n(4, 2, 1), n(5, 3, 2), n(6, 3, 3)];
  const tree = [n(1, null, 1), n(2, 1, 2), n(3, 1, 3), n(4, 1, 4)];
  /** 사람 화면 캡처(docs/mockups/arrange-root.html v2): 자식 없는 뿌리 1, 자식 묶음 넷을 둔 나무 2. */
  const screen = [n(1, null, 0), n(2, null, 0), n(3, 2, 0), n(4, 3, 1), n(5, 3, 2), n(6, 2, 3), n(7, 6, 3), n(8, 2, 4)];
  const cases: [string, N[], number, ArrangeDirection][] = [
    ['① person picture', person, 4, 'down'],
    ['② mixed', [n(1, null, 0), n(2, null, 0), n(3, 2, 0), n(4, 2, 1), n(5, 4, 1)], 2, 'down'],
    ['③ in-group children only', [n(1, null, 0), n(2, 1, 0), n(3, 1, 0), n(4, 1, 0)], 1, 'down'],
    ['④ one child group', [n(1, null, 0), n(2, 1, 1)], 2, 'down'],
    ['⑤ many child groups', [n(1, null, 0), n(2, 1, 1), n(3, 1, 2), n(4, 1, 3)], 4, 'down'],
    ['⑥ parent in the middle', base, 2, 'down'],
    ['⑥a A gets an in-group child', [...base, n(6, 1, 0)], 2, 'down'],
    ['⑥b B gets an in-group child', [...base, n(6, 3, 0)], 2, 'down'],
    ['⑥c A gets a child group', [...base, n(6, 1, 2)], 3, 'down'],
    ['⑥d B gets a child group', [...base, n(6, 3, 2)], 3, 'down'],
    ['⑦ deep chain', [n(1, null, 0), n(2, 1, 1), n(3, 2, 1), n(4, 2, 2), n(5, 2, 3), n(6, 5, 3)], 4, 'down'],
    ['⑧ several root groups', [n(1, null, 0), n(2, null, 1), n(3, 2, 1), n(4, 1, 2)], 3, 'down'],
    ['⑨ wide and tall cards', [n(1, null, 0), n(2, 1, 0, { width: 1000, height: 300 }), n(3, 1, 0, { width: 400, height: 800 }), n(4, 1, 1)], 2, 'down'],
    ['⑩ M8', [n(1, null, 0), n(2, 1, 0), n(3, 1, 0), n(4, 1, 0), n(5, null, 0), n(6, 5, 1), n(7, 5, 2), n(8, 5, 3), n(9, 5, 4)], 5, 'down'],
    ['⑪ person, right', person, 4, 'right'],
    ['⑪ person, up', person, 4, 'up'],
    ['⑪ person, left', person, 4, 'left'],
    ['⑫a one empty worktree', tree, 5, 'down'],
    ['⑫b two empty worktrees', tree, 6, 'down'],
    ['M19 ① childless root', screen, 5, 'down'],
    ['M19 ② root gets an in-group child', [...screen, n(9, 1, 0)], 5, 'down'],
    ['M19 ③ root gets a child group', [...screen, n(9, 1, 5)], 6, 'down'],
  ];
  const inside = (pt: { x: number; y: number }, b: Box) => pt.x > b.at.x && pt.x < b.at.x + b.size.width && pt.y > b.at.y && pt.y < b.at.y + b.size.height;

  it.each(cases)('%s: no overlap, boxes nest, edges cross no card, parents centred', (_, nodes, groups, direction) => {
    const { cards, boxes, input } = family(nodes, groups, direction);
    for (const [a, x] of cards) for (const [b, y] of cards) if (a !== b) expect(overlaps(x, y), `cards ${a} and ${b}`).toBe(false);
    // 상자끼리는 떨어져 있거나 하나가 다른 하나를 담는다. 자식 묶음은 부모 카드의 묶음 안이다.
    for (const [i, x] of boxes.entries()) for (const [k, y] of boxes.entries()) if (i !== k && overlaps(x, y)) expect(contains(x, y) || contains(y, x), `boxes ${i} and ${k}`).toBe(true);
    for (const [i, g] of input.entries()) {
      if (g.parent !== null) expect(contains(boxes[g.parent], boxes[i]), `group ${i} in ${g.parent}`).toBe(true);
      for (const id of g.members) expect(contains(boxes[i], cards.get(id)!), `card ${id} in group ${i}`).toBe(true);
    }
    for (const child of nodes) {
      if (child.parent === null) continue;
      const path = stepPath(cards.get(child.parent)!, cards.get(child.id)!, direction, 36);
      expect(path).not.toBeNull();
      for (let s = 1; s < path!.length; s++) {
        const [p, q] = [path![s - 1], path![s]];
        for (let t = 0; t <= 200; t++) {
          const pt = { x: p.x + ((q.x - p.x) * t) / 200, y: p.y + ((q.y - p.y) * t) / 200 };
          for (const [id, b] of cards) if (id !== child.id && id !== child.parent) expect(inside(pt, b), `edge ${child.parent}→${child.id} crosses ${id}`).toBe(false);
        }
      }
    }
    // 부모 카드는 다음 줄 자식(같은 묶음 카드와 자식 묶음 상자) 전체의 가운데다.
    if (direction !== 'down') return;
    for (const parent of nodes) {
      const kids = [
        ...nodes.filter((x) => x.parent === parent.id && x.group === parent.group).map((x) => cards.get(x.id)!),
        ...input.flatMap((g, i) => (g.card === parent.id ? [boxes[i]] : [])),
      ];
      if (kids.length === 0) continue;
      const lo = Math.min(...kids.map((b) => b.at.x));
      const hi = Math.max(...kids.map((b) => b.at.x + b.size.width));
      expect(center(cards.get(parent.id)!), `parent ${parent.id}`).toBe((lo + hi) / 2);
    }
  });

  it('① matches the approved picture (arrange-root.html v2 ④)', () => {
    const { cards, boxes } = family(person, 4);
    expect([1, 2, 3, 4, 5, 6].map((id) => cards.get(id)!.at)).toEqual(PERSON_CARDS);
    expect(boxes.map((b) => [b.at.x, b.at.y, b.size.width, b.size.height])).toEqual(PERSON_BOXES);
  });

  it('M19 ①: a childless root stands on the first row left of the tree and takes no own column', () => {
    const { cards, boxes } = family(screen, 5);
    const without = family(screen.slice(1), 5);
    expect(boxes[0].size).toEqual(without.boxes[0].size);
    expect(cards.get(1)!.at.y).toBe(cards.get(2)!.at.y);
    expect(cards.get(1)!.at.x + CARD.width + 20).toBe(cards.get(2)!.at.x);
    expect(cards.get(3)!.at.x).toBeLessThan(cards.get(1)!.at.x + CARD.width);
  });

  it.each([
    ['② in-group child', [...screen, n(9, 1, 0)], 5],
    ['③ child group', [...screen, n(9, 1, 5)], 6],
  ] as [string, N[], number][])('M19 %s: the grown root keeps its place left of the other tree; the other tree shifts right as one piece', (_, nodes, groups) => {
    const before = family(screen, 5);
    const after = family(nodes, groups);
    const dx = after.cards.get(2)!.at.x - before.cards.get(2)!.at.x;
    expect(dx).toBeGreaterThan(0);
    for (const id of [3, 4, 5, 6, 7, 8]) expect(after.cards.get(id)!.at, `card ${id}`).toEqual({ x: before.cards.get(id)!.at.x + dx, y: before.cards.get(id)!.at.y });
    const others = [2, 3, 4, 5, 6, 7, 8].map((id) => after.cards.get(id)!.at.x);
    expect(after.cards.get(1)!.at.x + CARD.width).toBeLessThan(Math.min(...others));
    expect(center(after.cards.get(1)!)).toBe(center(after.cards.get(9)!));
  });

  it('⑥c and ⑥d: a child group stands inside the parent card\'s group, under the parent card', () => {
    for (const card of [1, 3]) {
      const { cards, boxes } = family([...base, n(6, card, 2)], 3);
      expect(contains(boxes[0], boxes[2])).toBe(true);
      expect(center(cards.get(card)!)).toBe(center(boxes[2]));
      expect(boxes[2].at.y).toBeGreaterThan(cards.get(card)!.at.y + CARD.height);
    }
  });

  it('⑫: a top group with no child group (an empty one too) stands after the trees', () => {
    const { boxes } = family(tree, 6);
    // 0(계정전환-화면)과 5(빈-작업)는 빈 묶음이다. 나무(1) 뒤에 선다.
    expect(boxes[1].at.x).toBe(0);
    expect(boxes[0].at.x).toBe(boxes[1].at.x + boxes[1].size.width + GROUP_GAP);
    expect(boxes[5].at.x).toBe(boxes[0].at.x + EMPTY.width + GROUP_GAP);
    expect(boxes[0].at.y).toBe(0);
    expect(boxes[0].size).toEqual(EMPTY);
  });

  it('removing the parent worktree: the child group moves up to a top group on the first row', () => {
    // 부모 묶음 1의 노드 2를 지우면 3은 뿌리가 된다(노드 제거와 입양: 부모가 없으면 뿌리).
    const before = family([n(1, null, 0), n(2, 1, 1), n(3, 2, 2)], 3);
    expect(contains(before.boxes[1], before.boxes[2])).toBe(true);
    const after = family([n(1, null, 0), n(3, null, 2)], 3);
    expect(after.input[2].parent).toBeNull();
    expect(after.cards.get(3)!.at.y).toBe(after.cards.get(1)!.at.y);
    expect(overlaps(after.boxes[0], after.boxes[2])).toBe(false);
    // 손자 묶음이 조부모 카드에 입양되면 조부모 묶음 안으로 올라온다.
    const adopted = family([n(1, null, 0), n(3, 1, 2)], 3);
    expect(adopted.input[2]).toMatchObject({ parent: 0, card: 1 });
    expect(contains(adopted.boxes[0], adopted.boxes[2])).toBe(true);
  });

  it('gaps: card to card 20, any pair with a box 32; the row gap holds the closing feet', () => {
    const { cards, boxes } = family([n(1, null, 0), n(2, 1, 0), n(3, 1, 1)], 2);
    // 2(카드)와 3의 묶음(상자) 사이는 32다.
    expect(boxes[1].at.x - (cards.get(2)!.at.x + CARD.width)).toBe(GROUP_GAP);
    // 1 아래 줄: 카드 아래 20(먼 쪽) 없이, 틈 32와 자식 묶음 머리 60. 머리 줄은 자식 묶음 상자 위다.
    expect(boxes[1].at.y - (cards.get(1)!.at.y + CARD.height)).toBe(20 + GROUP_GAP);
    expect(cards.get(3)!.at.y - boxes[1].at.y).toBe(60);
    // 바깥 상자 아래는 안쪽 상자 아래 + 20이다.
    expect(boxes[0].at.y + boxes[0].size.height).toBe(boxes[1].at.y + boxes[1].size.height + 20);
  });
});

describe('stepAt', () => {
  it('bends `bend` past the parent, as a share of the gap', () => {
    expect(stepAt(box(0, 0), box(0, 162), 'down', 36)).toBe(36 / 112);
    expect(stepAt(box(0, 162), box(0, 0), 'up', 36)).toBe(36 / 112);
  });
  it('a gap narrower than two bends bends in the middle; a child behind gets a curve (null)', () => {
    expect(stepAt(box(0, 0), box(0, 100), 'down', 36)).toBe(0.5);
    expect(stepAt(box(0, 0), box(300, 0), 'down', 36)).toBeNull();
  });
});

describe('push (people decision 2026-10-21, mockup v4 ②)', () => {
  const sib = (key: string, x: number, y: number, width = 100, height = 50) => ({ key, ...box(x, y, width, height) });

  it('the overlapped sibling moves the shortest way out, with the gap', () => {
    // mover 0..100 × 0..50. a는 80..180 × 0..50: 오른쪽으로 20 + 32, 왼쪽 180 - (-32) = 212.
    expect(push([sib('a', 80, 0)], box(0, 0), ['a'], 32)).toEqual(new Map([['a', { x: 132, y: 0 }]]));
    // a가 아래로 조금만 겹치면 아래로 비킨다: 50 + 32 - 40 = 42.
    expect(push([sib('a', 0, 40, 400)], box(0, 0), ['a'], 32)).toEqual(new Map([['a', { x: 0, y: 82 }]]));
  });

  it('ties pick right, down, left, up in that order', () => {
    // 같은 자리, 같은 크기: 네 쪽 모두 132. 오른쪽이다.
    expect(push([sib('a', 0, 0, 100, 100)], box(0, 0, 100, 100), ['a'], 32)).toEqual(new Map([['a', { x: 132, y: 0 }]]));
  });

  it('a pushed sibling pushes the next one the same way (cascade)', () => {
    const got = push([sib('a', 80, 0), sib('b', 200, 0, 100, 300)], box(0, 0), ['a'], 32);
    // a → 132..232. b(200..)와 겹친다 → 같은 쪽(오른쪽)으로 232 + 32 = 264.
    expect(got).toEqual(new Map([['a', { x: 132, y: 0 }], ['b', { x: 264, y: 0 }]]));
  });

  it('only targets move first; other overlaps stay until a cascade reaches them', () => {
    expect(push([sib('a', 80, 0), sib('c', -50, 0)], box(0, 0), ['a'], 32)).toEqual(new Map([['a', { x: 132, y: 0 }]]));
  });
});

describe('crossed', () => {
  it('changes when the leading edge passes the center line it came toward', () => {
    const other = box(150, 0);
    const before = centerSide(box(0, 0), other);
    expect(crossed(before, box(99, 0), other)).toBe(false);
    expect(crossed(before, box(100, 0), other)).toBe(true);
  });

  it('coming from the right counts the left edge', () => {
    const other = box(0, 0);
    const before = centerSide(box(150, 0), other);
    expect(crossed(before, box(51, 0), other)).toBe(false);
    expect(crossed(before, box(50, 0), other)).toBe(true);
  });

  it('a big group swaps when it covers half of a small sibling, not only when its center passes (repro: a 640 wide group over a 279 wide one)', () => {
    // 사람 "묶음을 드래그할때는 터미널처럼 서로 포지션이 바뀌는 동작이 안일어남". 중심으로 세면 x 520까지 끌어야 했다.
    const other = box(700, 0, 279, 238);
    const before = centerSide(box(0, 0, 640, 517), other);
    expect(crossed(before, box(199, 0, 640, 517), other)).toBe(false);
    expect(crossed(before, box(200, 0, 640, 517), other)).toBe(true);
  });

  it('a small group over a big sibling follows the same rule', () => {
    const other = box(300, 0, 640, 517);
    const before = centerSide(box(0, 0, 279, 238), other);
    expect(crossed(before, box(340, 0, 279, 238), other)).toBe(false);
    expect(crossed(before, box(341, 0, 279, 238), other)).toBe(true);
  });

  it('a 1px jitter across the other center line does not count (repro: two same-height groups in one row)', () => {
    // 같은 줄, 같은 높이: 세로 중심이 같다. 옆에서 다가와 3 겹친 채 1 아래로 흔들려도 비키지 않는다.
    const other = box(688, 64, 640, 517);
    const before = centerSide(box(16, 64, 640, 517), other);
    expect(crossed(before, box(51, 65, 640, 517), other)).toBe(false);
    expect(crossed(before, box(51, 63, 640, 517), other)).toBe(false);
    expect(crossed(before, box(700, 65, 640, 517), other)).toBe(true);
  });

  it('coming from a corner counts both lines', () => {
    const other = box(100, 100);
    const before = centerSide(box(0, 0), other);
    expect(crossed(before, box(40, 60), other)).toBe(false);
    expect(crossed(before, box(40, 80), other)).toBe(true);
  });
});

describe('swap while dragging (people decision 2026-10-21: swap at the center line, push only on drop)', () => {
  // 사람 화면(ktasearch main 묶음): 카드 「피드백 반영」 위, 자식 묶음 「매뉴얼그림」 아래.
  const card = { key: 'card', ...box(0, 0, 505, 392) };
  const group = { key: 'group', ...box(49, 424, 539, 505) };
  /** `key` 상자를 `path` 자리로 차례로 끈다. 지나간 단계마다 바꾼 형제 자리를 돌려준다. */
  const drag = (moving: typeof card, others: (typeof card)[], path: [number, number][]) => {
    let state: Swap = { slot: moving, sides: new Map(), at: new Map() };
    return path.map(([x, y]) => (state = swap(state, others, box(x, y, moving.size.width, moving.size.height))).at);
  };

  it('nothing moves before the leading edge reaches the center line, even while overlapping (no push during drag)', () => {
    // 묶음 위 변이 카드 중심선(196)에 닿기 전: 카드와 300 넘게 겹쳐도 움직이지 않는다.
    const steps = drag(group, [card], [[49, 424], [49, 300], [49, 197]]);
    expect(steps.every((at) => at.size === 0)).toBe(true);
  });

  it('a big group dragged up swaps with a small card; the card takes the bottom of the group slot', () => {
    const [, at] = drag(group, [card], [[49, 424], [49, 196]]);
    // 카드 아래 변이 묶음의 끌기 전 아래 변(929)에 선다: 929 - 392 = 537. 둘이 차지하던 0..929 안에서 순서만 바뀐다. 넘지 않은 가로축은 제 자리(0)다.
    expect(at).toEqual(new Map([['card', { x: 0, y: 537 }]]));
  });

  it('dragging back past the moved card swaps again and the card returns home', () => {
    // 카드 중심은 537 + 196 = 733. 묶음 아래 변(y + 505)이 그 선에 닿으면(y 228) 돌아간다.
    const steps = drag(group, [card], [[49, 424], [49, 196], [49, 227], [49, 228]]);
    expect(steps[2]).toEqual(new Map([['card', { x: 0, y: 537 }]]));
    expect(steps[3]).toEqual(new Map([['card', { x: 0, y: 0 }]]));
  });

  it('a small card dragged down swaps with a big group; the group takes the top of the card slot', () => {
    // 묶음 중심 676.5. 카드 아래 변(y + 392)이 닿으면(y 284.5) 바꾼다.
    const steps = drag(card, [group], [[0, 0], [0, 284], [0, 285]]);
    expect(steps[1].size).toBe(0);
    expect(steps[2]).toEqual(new Map([['group', { x: 49, y: 0 }]]));
  });

  it('only crossed siblings move; others stay even when the dragged box overlaps them', () => {
    const side = { key: 'side', ...box(600, -150, 505, 392) };
    // 묶음(100..639 × 196..701)이 오른쪽 위 카드와 39 × 46 겹친다. 앞 변은 그 카드의 중심선(852, 46)에 닿지 않는다.
    const [, at] = drag(group, [card, side], [[49, 424], [100, 196]]);
    expect([...at.keys()]).toEqual(['card']);
  });
});

describe('freeSpot', () => {
  it('moves right past overlapped boxes with the gap', () => {
    expect(freeSpot(box(0, 0), [box(50, 0), box(182, 0)], 32)).toEqual({ x: 314, y: 0 });
    expect(freeSpot(box(0, 100), [box(50, 0)], 32)).toEqual({ x: 0, y: 100 });
  });
});

describe('unpile (render-time push; the pushed spot is not saved, 2026-10-22 "state must be one")', () => {
  it('a group that grows down pushes the saved group under it the shortest way, with the gap', () => {
    // main 0..1000 × 0..400 grows to 840 high (a card in row 2). 판정근거-범위 at 480..980 × 440..840.
    // right 1000 + 32 - 480 = 552, down 840 + 32 - 440 = 432, left 1012, up 872: down.
    const at = unpile([{ key: 'main', ...box(0, 0, 1000, 840) }, { key: 'g', ...box(480, 440, 500, 400) }], 32);
    expect(at).toEqual(new Map([['main', { x: 0, y: 0 }], ['g', { x: 480, y: 872 }]]));
  });

  it('a new group whose auto spot is on a grown saved group goes to its right', () => {
    expect(freeSpot(box(480, 440, 500, 400), [box(0, 0, 1000, 840)], 32)).toEqual({ x: 1032, y: 440 });
  });

  it('repo loom grew over ktasearch: the front box stays and the box behind moves (repro 2026-10-22)', () => {
    // right 5028 + 32 - 4388 = 672, down and up 595 + 32 = 627: down (shortest, then the tie order).
    const at = unpile([{ key: 'repo:loom', ...box(0, 0, 5028, 595) }, { key: 'repo:ktasearch', ...box(4388, 0, 672, 595) }], 32);
    expect(at.get('repo:ktasearch')).toEqual({ x: 4388, y: 627 });
  });

  it('boxes that only touch or are apart do not move', () => {
    const boxes = [{ key: 'a', ...box(0, 0) }, { key: 'b', ...box(100, 0) }, { key: 'c', ...box(300, 0) }];
    expect(unpile(boxes, 32)).toEqual(new Map(boxes.map((b) => [b.key, b.at])));
  });

  it('push → shrink → return: B is pushed while A is wide and is back on its saved spot when A shrinks, with nothing remembered', () => {
    const b = { key: 'B', ...box(432, 0, 400, 600) };
    expect(unpile([{ key: 'A', ...box(0, 0, 600, 600) }, b], 32).get('B')).toEqual({ x: 632, y: 0 });
    expect(unpile([{ key: 'A', ...box(0, 0, 400, 600) }, b], 32).get('B')).toEqual({ x: 432, y: 0 });
  });
});

describe('settle (an arranged repo is derived from sizes, not saved; 2026-10-22 two tabs flipped its saved layout)', () => {
  const item = (key: string, y: number, height: number) => ({ key, size: { width: 600, height }, lead: { x: 0, y: 0 }, auto: { x: 20, y } });
  const nothingSaved = () => undefined;

  it('without placed spots each tab gets the auto layout from its own sizes, so a grown card needs no save', () => {
    expect(settle([item('main', 0, 437), item('kid', 549, 517)], nothingSaved, 32).get('kid')).toEqual({ x: 20, y: 549 });
    expect(settle([item('main', 0, 502), item('kid', 614, 517)], nothingSaved, 32).get('kid')).toEqual({ x: 20, y: 614 });
  });

  it('a placed spot (a box the person dropped) wins over the auto layout, and a new box goes to the free spot beside it', () => {
    const placed = (key: string) => (key === 'main' ? { x: 20, y: 549 } : undefined);
    const at = settle([item('main', 0, 437), item('kid', 549, 517)], placed, 32);
    expect(at.get('main')).toEqual({ x: 20, y: 549 });
    expect(at.get('kid')).toEqual({ x: 652, y: 549 });
  });
});

describe('followCard', () => {
  it('keeps the moved card at the same screen spot', () => {
    const camera = { x: 40, y: -10, zoom: 2 };
    const old = { x: 100, y: 50 };
    const now = { x: 160, y: 20 };
    const next = followCard(old, now, camera);
    expect(next.zoom).toBe(2);
    expect({ x: now.x * next.zoom + next.x, y: now.y * next.zoom + next.y }).toEqual({ x: old.x * camera.zoom + camera.x, y: old.y * camera.zoom + camera.y });
  });

  it('does not move the camera when the card did not move', () => {
    const camera = { x: 5, y: 7, zoom: 0.5 };
    expect(followCard({ x: 3, y: 4 }, { x: 3, y: 4 }, camera)).toEqual(camera);
  });
});
