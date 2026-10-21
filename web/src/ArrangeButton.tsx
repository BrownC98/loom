import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Check, ChevronDown, RotateCcw, Workflow, type LucideIcon } from 'lucide-react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { ARRANGE_DIRECTIONS, type ArrangeDirection } from './layout';

/** 정렬 방향은 보는 사람마다 하나다. 끈 카드 위치처럼 화면 상태라 이 브라우저에만 둔다. */
const STORAGE_KEY = 'loom.arrangeDirection';

export function loadDirection(): ArrangeDirection {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    return ARRANGE_DIRECTIONS.find((d) => d === saved) ?? 'down';
  } catch {
    return 'down';
  }
}

export function saveDirection(direction: ArrangeDirection) {
  try {
    localStorage.setItem(STORAGE_KEY, direction);
  } catch {
    // 저장할 수 없으면 이 화면에서만 쓴다.
  }
}

const ARROW: Record<ArrangeDirection, LucideIcon> = { down: ArrowDown, right: ArrowRight, up: ArrowUp, left: ArrowLeft };

function Arrow(props: { direction: ArrangeDirection }) {
  const Icon = ARROW[props.direction];
  return <Icon />;
}

/** 메뉴 폭. styles.css `.menu`의 폭이다. 메뉴 오른쪽을 버튼 오른쪽에 맞춘다. */
const MENU_WIDTH = 180;

/**
 * [정렬]은 지금 방향으로 다시 놓고, 옆 버튼은 방향 메뉴를 연다. 메뉴에서 고르면 그 방향으로 바로 정렬한다.
 * 오른쪽 사이드바 「보기」에 하나다(목업 toolbar.html E안). 메뉴는 그래프 층에 덮이지 않게 body에 버튼 자리 기준으로 그린다.
 * 메뉴 끝의 [모든 카드 기본 크기로]는 보이는 그래프의 카드 크기만 되돌린다(목업 docs/mockups/card-resize.html ④). `hasSizes`는 메뉴를 열 때 한 번 묻는다.
 */
export function ArrangeButton(props: { direction: ArrangeDirection; onArrange: (direction: ArrangeDirection) => void; hasSizes: () => boolean; onResetSizes: () => void }) {
  const { t } = useTranslation();
  const [at, setAt] = useState<{ top: number; left: number; resettable: boolean } | null>(null);
  const open = at !== null;
  const caret = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const items = useRef<(HTMLButtonElement | null)[]>([]);
  const setOpen = (next: boolean) => {
    const r = caret.current?.getBoundingClientRect();
    setAt(next && r ? { top: r.bottom + 4, left: r.right - MENU_WIDTH, resettable: props.hasSizes() } : null);
  };
  // 메뉴와 버튼 밖을 누르면 닫는다. 그래프를 옮기기 시작해도 닫힌다.
  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => {
      const target = e.target as Node;
      if (!menu.current?.contains(target) && !caret.current?.contains(target)) setAt(null);
    };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  }, [open]);
  const choose = (direction: ArrangeDirection) => {
    setOpen(false);
    props.onArrange(direction);
  };
  const onMenuKey = (e: KeyboardEvent) => {
    // 꺼진 [모든 카드 기본 크기로]는 건너뛴다.
    const live = items.current.filter((el) => el !== null && !el.disabled);
    const at = live.indexOf(document.activeElement as HTMLButtonElement);
    const step = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0;
    if (step) {
      e.preventDefault();
      live[(at + step + live.length) % live.length]?.focus();
    } else if (e.key === 'Escape') {
      setOpen(false);
      caret.current?.focus();
    }
  };
  return (
    <span className="split">
      <button className="btn" onClick={() => props.onArrange(props.direction)}>
        <Workflow />
        {t('graph.arrange')}
      </button>
      <button ref={caret} className="btn caret" aria-haspopup="menu" aria-expanded={open} aria-label={t('graph.direction')} title={t('graph.direction')} onClick={() => setOpen(!open)}>
        <Arrow direction={props.direction} />
        <ChevronDown />
      </button>
      {at &&
        createPortal(
          <div ref={menu} className="menu" role="menu" aria-label={t('graph.direction')} style={{ top: at.top, left: at.left }} onKeyDown={onMenuKey}>
            {ARRANGE_DIRECTIONS.map((direction, i) => (
              <button
                key={direction}
                role="menuitemradio"
                aria-checked={direction === props.direction}
                autoFocus={direction === props.direction}
                ref={(el) => {
                  items.current[i] = el;
                }}
                onClick={() => choose(direction)}
              >
                <Arrow direction={direction} />
                {t(`graph.directions.${direction}`)}
                {direction === props.direction && (
                  <Check className="check" />
                )}
              </button>
            ))}
            <hr />
            <button
              role="menuitem"
              disabled={!at.resettable}
              ref={(el) => {
                items.current[ARRANGE_DIRECTIONS.length] = el;
              }}
              onClick={() => {
                setOpen(false);
                props.onResetSizes();
              }}
            >
              <RotateCcw />
              {t('graph.resetSizes')}
            </button>
          </div>,
          document.body,
        )}
    </span>
  );
}
