/** Browser fixture uses real Host filesystem and PTY; only native cwd lookup is controlled. */
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, copyFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
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
    mkdirSync(join(root, 'docs'));
    writeFileSync(join(root, 'docs/モルガン_解説.md'), '# モルガン 解説\n\n**Rendered file reference**');
    writeFileSync(join(root, 'lines.py'), Array.from({length: 150}, (_, i) => `# line ${i + 1}`).join('\n'));
    writeFileSync(join(root, 'docs/guide.md'), '---\ntitle: Sidebar documentation\nauthor: Example\n---\n# Rendered document\n\n| Feature | Status |\n| --- | --- |\n| Markdown | Ready |\n\n**Bold text** and $x^2$.\n\n![diagram](diagram.svg)\n\n[Read source](../src/test.py)\n');
    writeFileSync(join(root, 'docs/diagram.svg'), '<svg xmlns="http://www.w3.org/2000/svg" width="800" height="300"><rect width="800" height="300" fill="#4285f4"/></svg>');
    writeFileSync(join(root, 'docs/page.html'), '<!doctype html><style>h1{color:rgb(200,0,0)}</style><h1>HTML document</h1><script>parent.postMessage("unexpected-script", "*")</script>');
    writeFileSync(join(root, 'docs/interactive.html'), '<!doctype html><link rel="stylesheet" href="counter.css?v=1"><button id="counter">Count 0</button><output id="boundary"></output><script src="counter.js?v=1"></script>');
    writeFileSync(join(root, 'docs/counter.css'), '#counter{color:rgb(12,34,56)}');
    writeFileSync(join(root, 'docs/counter.js'), 'let n=0;counter.onclick=()=>counter.textContent="Count "+(++n);try{parent.localStorage.getItem("secret");boundary.textContent="exposed"}catch{boundary.textContent="isolated"}');
    writeFileSync(join(root, 'docs/table.csv'), 'Name,Amount\nApples,12\nPears,8\n');
    for (const name of ['sample.pdf','sample.xlsx']) copyFileSync(new URL(`../fixtures/documents/${name}`,import.meta.url),join(root,'docs',name));
    const host = new LocalWorkspaceHost({ roots: [{ id: 'A', cwd: root, displayName: 'Workspace A' }], endpoint: 'ws://localhost:8080', picker: false, metadataFile: join(root, 'registry.json'), nativeFilesystem: 'shared', terminalSupervisor: fileURLToPath(new URL('../../../target/debug/interactive-supervisor', import.meta.url)), transportToken: 'fixture' }, undefined, async (_endpoint, _token, target) => { if (target.session_id !== 'A' || target.active_node !== 'node-A') throw new Error('Wrong native Session'); return root; }, async () => async () => {});
    const handler = workspaceHandler(host);
    server.middlewares.use('/product-host/workbench-fixture', (request, response, next) => { request.url = '/product-host' + request.url; void handler(request, response, next); });
    server.httpServer?.on('close', () => { void host.close().then(() => rmSync(root, { recursive: true, force: true })); });
  } };
}
