/** Normal Vite carrier, with an owner IPC handshake instead of log scraping. */
import { createServer } from 'vite';
import { readFileSync } from 'node:fs';
import { browserAuth } from '../host/browser-auth.ts';

if (!process.send) throw new Error('Use pnpm --dir dev web to start the owned carrier');
let stopping = false;
let requestStop!: () => void;
const stopped = new Promise<void>(resolve => { requestStop = () => { stopping = true; resolve(); }; });
process.on('message', message => {
  if (typeof message === 'object' && message !== null && 'stop' in message && message.stop === true) requestStop();
});
process.on('disconnect', requestStop);
process.on('SIGINT', requestStop);
process.on('SIGTERM', requestStop);
const auth = browserAuth(JSON.parse(readFileSync(process.env.RUSTX_WEB_BOOTSTRAP_CONFIG!, 'utf8')));
const server = await createServer({ plugins: [{ name: 'local-browser-auth', enforce: 'pre', configureServer(server) { server.middlewares.use(auth); } }], server: { host: '127.0.0.1', port: 0, strictPort: true } });
try {
  if (!stopping) {
    await server.listen();
    if (!stopping && process.connected) process.send({ ready: server.resolvedUrls?.local[0] });
  }
  await stopped;
} finally {
  await server.close();
  if (process.connected) process.disconnect();
}
