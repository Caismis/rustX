// One standalone public-protocol smoke example. Requires Node.js LTS.
import { readFile } from 'node:fs/promises';

const [endpoint, tokenFile] = process.argv.slice(2);
if (!endpoint || !tokenFile) throw Error('Usage: node examples/app-server/connect.mjs ws://127.0.0.1:8787/ /absolute/token-file');
const token = (await readFile(tokenFile, 'utf8')).replace(/\n$/, '');
const socket = new WebSocket(endpoint, ['rustx.app-server.v78', `rustx-token.${Buffer.from(token).toString('base64url')}`]);
const timeout = setTimeout(() => { socket.close(); process.exitCode = 1; }, 10_000);
await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = () => reject(Error('WebSocket admission failed')); });
const pending = new Map();
socket.onmessage = event => {
  const reply = JSON.parse(event.data);
  if (reply.method || !pending.has(reply.id)) return;
  const { resolve, reject } = pending.get(reply.id); pending.delete(reply.id);
  if (reply.error) reject(Error(reply.error.message)); else resolve(reply.result);
};
let nextId = 0;
const call = (method, params) => new Promise((resolve, reject) => {
  const id = ++nextId; pending.set(id, { resolve, reject });
  socket.send(JSON.stringify({ jsonrpc:'2.0', id, method, params }));
});
try {
  await call('initialize', { protocol_version:78, client:{name:'standalone-example',version:'1'}, presentation:{images:false,questionnaires:false,reviews:false} });
  console.log(JSON.stringify(await call('server/info', {}), null, 2));
} finally { clearTimeout(timeout); socket.close(); }
