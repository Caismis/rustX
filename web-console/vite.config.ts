import { readFileSync } from 'node:fs';
import { LocalWorkspaceHost } from './host/workspaces.ts';
import { workspaceHandler } from './host/http.ts';
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
const host = process.env.RUSTX_WORKSPACE_HOST_CONFIG ? new LocalWorkspaceHost(JSON.parse(readFileSync(process.env.RUSTX_WORKSPACE_HOST_CONFIG, 'utf8'))) : undefined;
export default defineConfig({
  plugins: [react(), { name: 'product-host-workspaces', configureServer(server) { server.middlewares.use(workspaceHandler(host)); }, configurePreviewServer(server) { server.middlewares.use(workspaceHandler(host)); } }],
  server: { host: '127.0.0.1', strictPort: true },
  test: { setupFiles: ['./test/setup.ts'], include: ['test/**/*.test.ts', 'test/**/*.test.tsx'], environment: 'jsdom', restoreMocks: true },
});
