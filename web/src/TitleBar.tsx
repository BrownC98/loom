import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Copy, Minus, Square, X } from 'lucide-react';

/** 앱 아이콘(`app/icons/icon.ico`)의 짜임 무늬. 칸마다 가로·세로 막대가 번갈아 놓인다. */
const WEAVE = [0, 1, 2].flatMap((row) => [0, 1, 2].map((col) => ({ x: 0.5 + col * 5, y: 0.5 + row * 5, across: (row + col) % 2 === 0 })));

/**
 * Windows 앱 창의 맨 위 줄(docs/mockups/titlebar.html). 앱은 OS 제목 줄을 끄고(`app/src/main.rs`) 이 줄을 그린다.
 * 빈 곳을 끌면 창이 옮겨지고, 두 번 누르면 최대화가 바뀐다(Tauri `data-tauri-drag-region`). 브라우저에서 열면 그리지 않는다.
 */
export function TitleBar() {
  const { t } = useTranslation();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    // 최대화·이전 크기로 바뀌면 창 크기도 바뀐다. 그때 상태를 다시 읽는다.
    const read = () => getCurrentWindow().isMaximized().then(setMaximized);
    read();
    addEventListener('resize', read);
    return () => removeEventListener('resize', read);
  }, []);

  if (!isTauri()) return null;
  const win = getCurrentWindow();
  const toggle = maximized ? t('titlebar.restore') : t('titlebar.maximize');
  return (
    <header className="titlebar" data-tauri-drag-region="deep">
      <svg className="titlebar-mark" viewBox="0 0 16 16" aria-hidden>
        {WEAVE.map(({ x, y, across }) => (
          <rect key={`${x},${y}`} x={across ? x : x + 1} y={across ? y + 1 : y} width={across ? 4 : 2} height={across ? 2 : 4} />
        ))}
      </svg>
      <span className="spacer" />
      <button type="button" className="caption" aria-label={t('titlebar.minimize')} title={t('titlebar.minimize')} onClick={() => win.minimize()}>
        <Minus />
      </button>
      <button type="button" className="caption" aria-label={toggle} title={toggle} onClick={() => win.toggleMaximize()}>
        {maximized ? <Copy /> : <Square />}
      </button>
      <button type="button" className="caption close" aria-label={t('common.close')} title={t('common.close')} onClick={() => win.close()}>
        <X />
      </button>
    </header>
  );
}
