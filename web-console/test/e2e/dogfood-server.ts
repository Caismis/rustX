import { startWorkspaceHost } from './workspace-host.ts';
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../../', import.meta.url));

function readiness(child: ChildProcess, output: 'stdout' | 'stderr', match: (line: string) => string | undefined): Promise<string> {
  return new Promise((resolveReady, reject) => {
    const lines = createInterface({ input: child[output]! });
    const timer = setTimeout(() => { lines.close(); reject(new Error('Process readiness deadline exceeded.')); }, 60_000);
    child.once('error', error => { clearTimeout(timer); lines.close(); reject(error); });
    child.once('exit', code => { clearTimeout(timer); lines.close(); reject(new Error(`Process exited before readiness: ${code}`)); });
    lines.on('line', line => { const found = match(line); if (found) { clearTimeout(timer); lines.removeAllListeners('line'); lines.on('line', () => {}); resolveReady(found); } });
  });
}
function exited(child: ChildProcess): Promise<number | null> {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve(child.exitCode);
  return new Promise((resolveExit, reject) => {
    const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error('Child failed to exit after shutdown.')); }, 10_000);
    child.once('exit', code => { clearTimeout(timer); resolveExit(code); });
  });
}
export async function startDogfood(scenario = 'web_console_dogfood', trusted = true) {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-web-console-'));
  const taskConfig = join(directory, 'config');
  const settings = join(taskConfig, 'rustx/settings.toml');
  mkdirSync(join(taskConfig, 'rustx'), { recursive: true });
  const workspaceA = join(directory, 'A'), workspaceB = join(directory, 'B');
  mkdirSync(workspaceA); mkdirSync(workspaceB);
  const binary = process.env.RUSTX_BINARY ?? resolve(root, 'target/debug/rustx');
  const fixtureHome = join(directory, 'home'); mkdirSync(fixtureHome);
  const env = { ...process.env, HOME: fixtureHome, XDG_CONFIG_HOME: taskConfig, XDG_STATE_HOME: join(directory, 'state'), RUSTX_CONSOLE_FIXTURE_KEY: 'fake-provider-only' };
  const provider = spawn('uv', ['run', '--project', resolve(root, 'test-support/fake-provider'), '--frozen', 'fake-provider', '--scenario', scenario, '--port', '0'], { stdio: ['pipe', 'pipe', 'pipe'] });
  let providerErrors = ''; provider.stderr.on('data', chunk => { providerErrors = (providerErrors + String(chunk)).slice(-16_384); });
  let app: ChildProcess | undefined;
  try {
    const providerUrl = await readiness(provider, 'stdout', line => {
      const message = JSON.parse(line); return message.ready ? `http://${message.host}:${message.port}` : undefined;
    });
    writeFileSync(join(taskConfig, 'rustx/models.toml'), `[providers.fixture]\nbase_url = "${providerUrl}/v1"\napi_key = "$RUSTX_CONSOLE_FIXTURE_KEY"\n` +
      ['console-model', 'second-model'].map(id => `\n[[providers.fixture.models]]\nid = "${id}"\nprotocol = "openai_chat_completions"\ncontext_window = 128000\nmax_output_tokens = 4096\ncapabilities = { input_modalities = ["text"], output_modalities = ["text"], tool_calls = true, reasoning = false }\ncompat = { chat_reasoning_replay = "omit" }\n`).join(''));
    const imageSource = scenario === 'web_chat_history' ? `
[mcp_servers.image_fixture]
enabled = true
type = "stdio"
command = "python3"
args = [${JSON.stringify(resolve(root, 'web-console/test/e2e/image-mcp.py'))}]
[mcp_tool_policies.image_fixture]
approval = "never"
[agent.tools.sources]
image_fixture = ["render_image"]
` : scenario === 'web_composer_context' ? `
[agent.extensions.todo]
enabled = true
[agent.extensions.goal]
enabled = true
` : '';
    const writeSettings = (model = 'console-model') => writeFileSync(settings, `[model_timeout_policy]\nresponse_start_timeout_ms = 600000\nstream_idle_timeout_ms = 600000\n[native_tools.bash]\napproval = "always"\n[agent.model]\nmodel = "fixture/${model}"\n` + imageSource);
    writeSettings();
    for (const workspace of trusted ? [workspaceA, workspaceB] : []) {
      const trusted = spawnSync(binary, ['--workspace', workspace, '--trust', 'grant'], { env, encoding: 'utf8' });
      if (trusted.status !== 0) throw new Error(`Trust setup failed: ${trusted.stderr}`);
    }
    const token = randomBytes(32).toString('base64url');
    const tokenFile = join(directory, 'socket-token'); writeFileSync(tokenFile, token, { mode: 0o600 });
    app = spawn(binary, ['app-server', '--user-settings', settings, '--runtime-root', join(directory, 'runtime'), '--listen', 'ws://127.0.0.1:0', '--token-file', tokenFile], { env, stdio: ['pipe', 'pipe', 'pipe'] });
    let appErrors = ''; app.stderr!.on('data', chunk => { appErrors = (appErrors + String(chunk)).slice(-16_384); });
    const endpoint = await readiness(app, 'stderr', line => /listening (ws:\/\/\S+)/.exec(line)?.[1]);
    const control = async (path: string, method = 'GET') => {
      const response = await fetch(`${providerUrl}/__control/${path}`, { method });
      if (!response.ok) throw new Error(`Provider barrier failed: ${await response.text()}`);
      return response.json();
    };
    const workspaceHost = await startWorkspaceHost({ endpoint: new URL(endpoint).href, picker: true, metadataFile: join(directory, 'workspaces.json'), roots: [
      { id: 'root-a', cwd: workspaceA, displayName: 'Workspace A' }, { id: 'root-b', cwd: workspaceB, displayName: 'Workspace B' },
    ] });
    return { workspaceHost, workspaceHostUrl: workspaceHost.url, directory, endpoint, token, tokenFile, workspaceA, workspaceB, providerUrl, settings, writeSettings, control,
      gate: (name: string) => control(`observations/await?kind=gate_reached&name=${name}&timeoutMs=30000`),
      release: (name: string) => control(`gates/${name}/release`, 'POST'),
      diagnostics: () => ({ providerErrors, appErrors }),
      async stop(check = true) {
        try {
          const report = await control('shutdown', 'POST');
          app!.kill('SIGTERM'); provider.stdin.end();
          const [providerCode] = await Promise.all([exited(provider), exited(app!)]);
          if (check && (!report.ok || providerCode !== 0)) throw new Error(`Provider scenario not satisfied (exit ${providerCode}): ${JSON.stringify(report)}`);
          return report;
        } finally {
          await workspaceHost.stop();
          app?.kill('SIGTERM'); provider.kill('SIGTERM');
          rmSync(directory, { recursive: true, force: true });
        }
      },
    };
  } catch (error) { app?.kill(); provider.kill(); rmSync(directory, { recursive: true, force: true }); throw new Error(`${String(error)}\n${providerErrors}`); }
}
