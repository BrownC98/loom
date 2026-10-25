/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig(({ mode }) => ({
  plugins: [react()],
  server: mode === 'test' ? { fs: { deny: ['.env', '.env.*', '*.{crt,pem}'] } } : undefined,
  build: { chunkSizeWarningLimit: 1500 },
  test: {
    css: { include: [/styles\.css/] },
    projects: [
      { extends: true, test: { name: 'node', include: ['src/**/*.test.{ts,tsx}'], exclude: ['src/{Tree,Graph,Inbox,Phone}.test.tsx'] } },
      { extends: true, test: { name: 'dom', environment: 'jsdom', include: ['src/{Tree,Graph,Inbox,Phone}.test.tsx'] } },
    ],
  },
}));
