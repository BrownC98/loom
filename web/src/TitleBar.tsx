import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Copy, Minus, Square, X } from 'lucide-react';
import { MAC } from './platform';

const WEAVE = [0, 1, 2].flatMap((row) => [0, 1, 2].map((col) => ({ x: 0.5 + col * 5, y: 0.5 + row * 5, across: (row + col) % 2 === 0 })));

export function TitleBar() {
  const { t } = useTranslation();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    const read = () => getCurrentWindow().isMaximized().then(setMaximized);
    read();
    addEventListener('resize', read);
    return () => removeEventListener('resize', read);
  }, []);

  if (!isTauri()) return null;
  const win = getCurrentWindow();
  const toggle = maximized ? t('titlebar.restore') : t('titlebar.maximize');
  return (
    <header className={MAC ? 'titlebar mac' : 'titlebar'} data-tauri-drag-region="deep">
      <svg className="titlebar-mark" viewBox="0 0 16 16" aria-hidden>
        {WEAVE.map(({ x, y, across }) => (
          <rect key={`${x},${y}`} x={across ? x : x + 1} y={across ? y + 1 : y} width={across ? 4 : 2} height={across ? 2 : 4} />
        ))}
      </svg>
      <span className="spacer" />
      {!MAC && (
        <>
          <button type="button" className="caption" aria-label={t('titlebar.minimize')} title={t('titlebar.minimize')} onClick={() => win.minimize()}>
            <Minus />
          </button>
          <button type="button" className="caption" aria-label={toggle} title={toggle} onClick={() => win.toggleMaximize()}>
            {maximized ? <Copy /> : <Square />}
          </button>
          <button type="button" className="caption close" aria-label={t('common.close')} title={t('common.close')} onClick={() => win.close()}>
            <X />
          </button>
        </>
      )}
    </header>
  );
}
