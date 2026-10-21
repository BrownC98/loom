import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import 'pretendard/dist/web/variable/pretendardvariable-dynamic-subset.css';
import '@xyflow/react/dist/base.css';
import '@xterm/xterm/css/xterm.css';
import './styles.css';
import './i18n';
import { App } from './App';
import { showClippedText } from './clippedText';

showClippedText(document);
// 터미널 밖에 파일을 놓으면 브라우저가 그 파일로 이 화면을 바꾼다. 터미널(Terminal.tsx)만 놓기를 받는다.
for (const type of ['dragover', 'drop'] as const)
  window.addEventListener(type, (e) => {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'none';
  });

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
