import { useRef, useState } from 'react';
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Check, ChevronDown, RotateCcw, Workflow, type LucideIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { ARRANGE_DIRECTIONS, type ArrangeDirection } from './layout';
import { Menu } from './Menu';

const ARROW: Record<ArrangeDirection, LucideIcon> = { down: ArrowDown, right: ArrowRight, up: ArrowUp, left: ArrowLeft };

function Arrow(props: { direction: ArrangeDirection }) {
  const Icon = ARROW[props.direction];
  return <Icon />;
}

/**
 * [정렬]은 지금 방향으로 다시 놓고, 옆 버튼은 방향 메뉴를 연다. 메뉴에서 고르면 그 방향으로 바로 정렬한다.
 * 오른쪽 사이드바 「보기」에 하나다(목업 toolbar.html E안).
 * [고정]은 상시 정렬을 켜고 끈다(목업 docs/mockups/always-arrange.html v4, 2026-10-22 사람 승인).
 * 메뉴 끝의 [모든 카드 기본 크기로]는 보이는 그래프의 카드 크기만 되돌린다(목업 docs/mockups/card-resize.html ④). `hasSizes`는 메뉴를 열 때 한 번 묻는다.
 */
export function ArrangeButton(props: { direction: ArrangeDirection; onArrange: (direction: ArrangeDirection) => void; alwaysArrange: boolean; onAlwaysArrange: (on: boolean) => void; hasSizes: () => boolean; onResetSizes: () => void }) {
  const { t } = useTranslation();
  // 메뉴를 열 때 한 번 묻는다. 닫혀 있으면 없다.
  const [resettable, setResettable] = useState<boolean | null>(null);
  const open = resettable !== null;
  const caret = useRef<HTMLButtonElement>(null);
  const close = () => setResettable(null);
  const choose = (direction: ArrangeDirection) => {
    close();
    props.onArrange(direction);
  };
  return (
    <span className="split">
      <button className="btn" onClick={() => props.onArrange(props.direction)}>
        <Workflow />
        {t('graph.arrange')}
      </button>
      <button className="btn pin" aria-pressed={props.alwaysArrange} title={t('graph.keepArrangedHint')} onClick={() => props.onAlwaysArrange(!props.alwaysArrange)}>
        {t('graph.keepArranged')}
      </button>
      <button ref={caret} className="btn caret" aria-haspopup="menu" aria-expanded={open} aria-label={t('graph.direction')} title={t('graph.direction')} onClick={() => setResettable(open ? null : props.hasSizes())}>
        <Arrow direction={props.direction} />
        <ChevronDown />
      </button>
      {open && (
        <Menu anchor={caret} label={t('graph.direction')} onClose={close}>
          {ARRANGE_DIRECTIONS.map((direction) => (
            <button key={direction} role="menuitemradio" aria-checked={direction === props.direction} autoFocus={direction === props.direction} onClick={() => choose(direction)}>
              <Arrow direction={direction} />
              {t(`graph.directions.${direction}`)}
              {direction === props.direction && <Check className="check" />}
            </button>
          ))}
          <hr />
          <button
            role="menuitem"
            disabled={!resettable}
            onClick={() => {
              close();
              props.onResetSizes();
            }}
          >
            <RotateCcw />
            {t('graph.resetSizes')}
          </button>
        </Menu>
      )}
    </span>
  );
}
