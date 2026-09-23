import { readFileSync } from 'node:fs';
import { LocalWorkspaceHost } from './host/workspaces.ts';
import { workspaceHandler } from './host/http.ts';
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'node:url';
const host = process.env.RUSTX_WORKSPACE_HOST_CONFIG ? new LocalWorkspaceHost(JSON.parse(readFileSync(process.env.RUSTX_WORKSPACE_HOST_CONFIG, 'utf8'))) : undefined;
export default defineConfig({
  plugins: [react(), { name: 'product-host-workspaces', configureServer(server) { server.middlewares.use(workspaceHandler(host)); }, configurePreviewServer(server) { server.middlewares.use(workspaceHandler(host)); } }],
  // TanStack Form core always constructs a devtools event client that
  // broadcasts and queues complete form state, secret field values included.
  // It is replaced by an inert module in the bundle and in tests alike; see
  // src/app/settings/forms/inert-devtools-event-client.ts.
  resolve: { alias: [{ find: /^@tanstack\/devtools-event-client$/, replacement: fileURLToPath(new URL('./src/app/settings/forms/inert-devtools-event-client.ts', import.meta.url)) }] },
  server: { host: '127.0.0.1', strictPort: true },
  test: {
    setupFiles: ['./test/setup.ts'], include: ['test/**/*.test.ts', 'test/**/*.test.tsx'], environment: 'jsdom', restoreMocks: true,
    // The whole form chain is transformed by Vite rather than loaded by Node
    // directly, so the replacement above applies to the tested code exactly as
    // it does to the production bundle. Inlining only form core is not enough:
    // an externalized @tanstack/react-form, once cached natively by a worker,
    // would import form core — and the real client — outside Vite.
    server: { deps: { inline: ['@tanstack/react-form', '@tanstack/form-core', '@tanstack/react-store', '@tanstack/store'] } },
  },
});
