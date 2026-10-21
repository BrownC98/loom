/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  // 화면은 데몬 실행 파일에 들어 있고 루프백으로 받는다. 기본 경고선(500kB)은 인터넷으로 받는 앱의 기준이다.
  // 지금 번들은 약 1MB(xterm·React Flow)다. 나누지 않고, 지금보다 크게 늘 때만 경고하게 1.5MB로 둔다.
  build: { chunkSizeWarningLimit: 1500 },
  // uiColors.test.ts가 styles.css 글자(`?raw`)를 읽는다. 기본값(css 끔)이면 빈 글자가 온다.
  test: { css: { include: [/styles\.css/] } },
});
