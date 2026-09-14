import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
export default defineConfig({
  plugins: [react()],
  server: { host: '127.0.0.1', strictPort: true },
  test: { include: ['test/**/*.test.ts', 'test/**/*.test.tsx'], environment: 'jsdom', restoreMocks: true },
});
