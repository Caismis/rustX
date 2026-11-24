/** Serve the built Web console with the same owned Product Host and browser proof. */
import { createReadStream, readdirSync, readFileSync, statSync } from 'node:fs';
import { createServer } from 'node:http';
import { extname, join } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { fileURLToPath } from 'node:url';
import { browserAuth } from '../host/browser-auth.ts';
import { workspaceHandler } from '../host/http.ts';
import { LocalWorkspaceHost } from '../host/workspaces.ts';

const portHost = '127.0.0.1';
const contentTypes: Record<string, string> = {
  '.css': 'text/css; charset=utf-8', '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8', '.mjs': 'text/javascript; charset=utf-8', '.json': 'application/json',
  '.png': 'image/png', '.svg': 'image/svg+xml', '.ico': 'image/x-icon', '.webp': 'image/webp',
  '.txt': 'text/plain; charset=utf-8', '.wasm': 'application/wasm', '.woff': 'font/woff', '.woff2': 'font/woff2',
};
const dist = fileURLToPath(new URL('../dist/', import.meta.url));
// Only regular files in the installed dist are served; a request path never reaches an open call.
const assets = new Map(readdirSync(dist, { recursive: true, withFileTypes: true }).filter(entry => entry.isFile()).map(entry => {
  const path = join(entry.parentPath, entry.name);
  const url = '/' + path.slice(dist.length).split('/').map(encodeURIComponent).join('/');
  return [url, { path, length: statSync(path).size, type: contentTypes[extname(entry.name)] ?? 'application/octet-stream' }];
}));
const index = assets.get('/index.html');
if (!index || !process.send) throw new Error('Start the installed Web console with rustx web');
assets.set('/', index);
const auth = browserAuth(JSON.parse(readFileSync(process.env.RUSTX_WEB_BOOTSTRAP_CONFIG!, 'utf8')));
const host = new LocalWorkspaceHost(JSON.parse(readFileSync(process.env.RUSTX_WORKSPACE_HOST_CONFIG!, 'utf8')));
const workspaces = workspaceHandler(host);
const server = createServer((request, response) => {
  auth(request, response, () => {
    void workspaces(request, response, () => {
      if (request.method !== 'GET' && request.method !== 'HEAD') {
        response.writeHead(405, { Allow: 'GET, HEAD' }).end();
        return;
      }
      const path = new URL(request.url ?? '/', `http://${portHost}`).pathname;
      const asset = assets.get(path);
      if (!asset) {
        response.writeHead(404).end();
        return;
      }
      response.writeHead(200, { 'Content-Type': asset.type, 'Content-Length': asset.length, 'X-Content-Type-Options': 'nosniff' });
      if (request.method === 'HEAD') response.end();
      else void pipeline(createReadStream(asset.path), response).catch(() => response.destroy());
    }).catch(() => response.destroy());
  });
});
const controller = new AbortController();
const stopped = new Promise<void>(resolve => controller.signal.addEventListener('abort', () => resolve(), { once: true }));
const stop = () => controller.abort();
process.on('message', message => {
  if (typeof message === 'object' && message !== null && 'stop' in message && message.stop === true) stop();
});
process.on('disconnect', stop);
process.on('SIGINT', stop);
process.on('SIGTERM', stop);
try {
  const listening = new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen({ host: portHost, port: 0, signal: controller.signal }, resolve);
  });
  await Promise.race([listening, stopped]);
  const address = server.address();
  if (address && typeof address !== 'string' && process.connected) process.send({ ready: `http://${portHost}:${address.port}/` });
  await stopped;
} finally {
  server.closeAllConnections();
  await host.close();
  if (process.connected) process.disconnect();
}
