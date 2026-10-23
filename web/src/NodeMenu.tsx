import { Ellipsis, PanelTop, RefreshCw, RotateCcw, SquareTerminal, Trash2 } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { clearCardView, relaunchNode, relaunchQueued, setCardView } from './api';
import { menuSwitch } from './cardView';
import { relaunchableState } from './catalog';
import type { DefaultCardView } from './gen/DefaultCardView';
import type { NodeView } from './gen/NodeView';
import { Menu } from './Menu';

export function NodeMenu(props: { view: NodeView; defaultView: DefaultCardView; resumable: boolean; onResetSize?: () => void; onRemove: () => void; onRefusal: (text: string) => void }) {
  const { t } = useTranslation();
  const { view } = props;
  const button = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
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
