import { startDogfood } from '../test/e2e/dogfood-server.ts';

const server = await startDogfood(process.argv[2] ?? 'web_console_dogfood', !process.argv.includes('--untrusted'));
// The Vite carrier will own this Host configuration. Do not leave a second
// metadata writer running alongside it.
await server.workspaceHost.stop();
console.log(JSON.stringify({
  endpoint: server.endpoint, tokenFile: server.tokenFile,
  hostConfigFile: server.hostConfigFile,
  workspaceA: server.workspaceA, workspaceB: server.workspaceB,
  settings: server.settings, providerControl: `${server.providerUrl}/__control`,
}, null, 2));
console.log('Start Web with RUSTX_WORKSPACE_HOST_CONFIG set to hostConfigFile. Follow DOGFOODING.md. Ctrl+C checks the selected provider script.');
await new Promise<void>(resolve => { process.once('SIGINT', resolve); process.once('SIGTERM', resolve); });
try { console.log(JSON.stringify(await server.stop(), null, 2)); }
catch (error) { console.error(String(error)); process.exitCode = 1; }
