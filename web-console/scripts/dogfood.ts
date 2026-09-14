import { startDogfood } from '../test/e2e/dogfood-server.ts';

const server = await startDogfood();
console.log(JSON.stringify({
  endpoint: server.endpoint, tokenFile: server.tokenFile,
  workspaceA: server.workspaceA, workspaceB: server.workspaceB,
  settings: server.settings, providerControl: `${server.providerUrl}/__control`,
}, null, 2));
console.log('Follow README.md in the browser. Ctrl+C shuts down and checks all eight provider steps.');
await new Promise<void>(resolve => { process.once('SIGINT', resolve); process.once('SIGTERM', resolve); });
try { console.log(JSON.stringify(await server.stop(), null, 2)); }
catch (error) { console.error(String(error)); process.exitCode = 1; }
