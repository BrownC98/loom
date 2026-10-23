// 노드 카드 머리의 「⋯」 메뉴: 드물거나 되돌리기 어려운 일을 모은다(목업 docs/mockups/relaunch.html v3, 2026-10-22 사람 승인 Q362).
// 머리에는 자주 쓰고 안전한 버튼(북마크, 확대)만 둔다. 사람 지적(Q360): "헤더에 버튼이 점점 많아지는것 같고 실수하기 쉬워질 것 같음".
import { Ellipsis, PanelTop, RefreshCw, RotateCcw, SquareTerminal, Trash2 } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { clearCardView, relaunchNode, relaunchQueued, setCardView } from './api';
import { menuSwitch } from './cardView';
import { relaunchableState } from './catalog';
import type { DefaultCardView } from './gen/DefaultCardView';
import type { NodeView } from './gen/NodeView';
import { Menu } from './Menu';

/**
 * `resumable`은 `canResume`이다. `onResetSize`는 크기를 바꾼 표준 카드만 준다. `onRefusal`은 거절 문장을 보일 곳이다.
 * 다시 띄우기는 실행 중인 에이전트 노드(프로필이 있는 에이전트)만 보인다. 턴 중이면 「턴이 끝나면 다시 띄웁니다」로 꺼 둔다. 메뉴를 열 때 데몬에 묻는다.
 * 지우기는 확인 없이 바로 지운다(docs/design.md 「노드 제거와 입양」). 메뉴가 한 단계를 더한다.
 * 첫 줄은 이 노드만 다른 카드로 본다(목업 docs/mockups/node-card-view.html ⑤). 고른 값이 기본 보기(`defaultView`)와 같으면 노드의 값을 지워 기본을 따르게 한다.
 */
export function NodeMenu(props: { view: NodeView; defaultView: DefaultCardView; resumable: boolean; onResetSize?: () => void; onRemove: () => void; onRefusal: (text: string) => void }) {
  const { t } = useTranslation();
  const { view } = props;
  const button = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  // 열 때 데몬에 물은 값. 답이 오기 전에는 없다.
  const [queued, setQueued] = useState<boolean | null>(null);
  const resumable = props.resumable && relaunchableState(view);
  const { next, clear } = menuSwitch(view, props.defaultView);
  const switchView = () => (clear ? clearCardView(view.id) : setCardView(view.id, next)).catch((err: Error) => props.onRefusal(err.message));
  const toggle = () => {
    if (open) return setOpen(false);
    setQueued(null);
    setOpen(true);
    if (resumable) relaunchQueued(view.id).then(setQueued, () => setQueued(false));
  };
  const run = (act: () => void) => {
    setOpen(false);
    act();
  };
  return (
    <>
      <button ref={button} type="button" className="btn ghost sm icon nodrag" aria-haspopup="menu" aria-expanded={open} aria-label={t('node.menu')} title={t('node.menu')} onClick={toggle}>
        <Ellipsis />
      </button>
      {open && (
        <Menu anchor={button} label={t('node.menu')} onClose={() => setOpen(false)}>
          <button role="menuitem" onClick={() => run(switchView)}>
            {next === 'standard' ? <SquareTerminal /> : <PanelTop />}
            {next === 'standard' ? t('node.toStandard') : t('node.toSimple')}
          </button>
          {props.onResetSize && (
            <button role="menuitem" onClick={() => run(props.onResetSize!)}>
              <RotateCcw />
              {t('node.resetSize')}
            </button>
          )}
          {resumable && (
            <button role="menuitem" disabled={queued !== false} onClick={() => run(() => relaunchNode(view.id).catch((err: Error) => props.onRefusal(err.message)))}>
              <RefreshCw />
              {queued ? t('node.relaunchQueued') : t('node.relaunch')}
            </button>
          )}
          {(resumable || props.onResetSize) && <hr />}
          <button role="menuitem" className="danger" onClick={() => run(props.onRemove)}>
            <Trash2 />
            {t('node.remove')}
          </button>
        </Menu>
      )}
    </>
  );
}
