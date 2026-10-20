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

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
