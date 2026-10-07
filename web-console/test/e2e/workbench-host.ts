/** Browser fixture uses real Host filesystem and PTY; only native cwd lookup is controlled. */
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Plugin } from 'vite';
import { LocalWorkspaceHost } from '../../host/workspaces.ts';
import { workspaceHandler } from '../../host/http.ts';
export function workbenchFixture(): Plugin {
  return { name: 'workbench-fixture', configureServer(server) {
    const root = mkdtempSync(join(tmpdir(), 'rustx-workbench-browser-')); mkdirSync(join(root, 'src')); writeFileSync(join(root, 'src/hello.txt'), 'Workspace preview works.');
    writeFileSync(join(root, 'src/test.py'), 'def greet(name):\n    return "Hello, " + name\n\n' + '# long line ' + 'source '.repeat(120) + '\n');
    writeFileSync(join(root, 'src/file10.txt'), 'ten'); writeFileSync(join(root, 'src/file2.txt'), 'two');
    const host = new LocalWorkspaceHost({ roots: [{ id: 'A', cwd: root, displayName: 'Workspace A' }], endpoint: 'ws://localhost:8080', picker: false, metadataFile: join(root, 'registry.json'), nativeFilesystem: 'shared', transportToken: 'fixture' }, undefined, async (_endpoint, _token, target) => { if (target.session_id !== 'A' || target.active_node !== 'node-A') throw new Error('Wrong native Session'); return root; });
    const handler = workspaceHandler(host);
    server.middlewares.use('/product-host/workbench-fixture', (request, response, next) => { request.url = '/product-host' + request.url; void handler(request, response, next); });
    server.httpServer?.on('close', () => { host.close(); rmSync(root, { recursive: true, force: true }); });
  } };
}
