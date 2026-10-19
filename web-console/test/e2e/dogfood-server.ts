import { appServerEndpoint } from '../../../dev/src/app-server-readiness.ts';
import { startWorkspaceHost } from './workspace-host.ts';
import { spawn, type ChildProcess } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, readFileSync, opendirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../../../', import.meta.url));

/** Kernel counters for this fixture's original native child, never process discovery. */
function nativeIdentity(pid: number | undefined) {
  if (process.platform !== 'linux' || pid === undefined) return undefined;
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, 'utf8');
    const fields = stat.slice(stat.lastIndexOf(')') + 2).trim().split(/\s+/);
    return { pid, startToken: fields[19]!, state: fields[0]!, cpuTicks: Number(fields[11]) + Number(fields[12]) };
  } catch { return undefined; }
}
function nativeDiagnostics(identity: ReturnType<typeof nativeIdentity>) {
  if (!identity) return undefined;
  const current = nativeIdentity(identity.pid);
  if (!current || current.startToken !== identity.startToken) return { pid: identity.pid, unavailable: 'Original native process exited.' };
  try {
    const io = readFileSync(`/proc/${identity.pid}/io`, 'utf8').slice(0, 4096);
    const tasks = opendirSync(`/proc/${identity.pid}/task`);
    const waits: { tid: string; wait: string }[] = [];
    let truncated = false;
    try {
      for (let count = 0; count < 64; count++) {
        const task = tasks.readSync();
        if (!task) break;
        if (!/^\d+$/.test(task.name)) continue;
        try { waits.push({ tid: task.name, wait: readFileSync(`/proc/${identity.pid}/task/${task.name}/wchan`, 'utf8').slice(0, 128) }); }
        catch { /* A native worker may have exited during this diagnostic. */ }
      }
      truncated = tasks.readSync() !== null;
    } finally { tasks.closeSync(); }
    const after = nativeIdentity(identity.pid);
    if (!after || after.startToken !== identity.startToken) return { pid: identity.pid, unavailable: 'Original native process exited during diagnostics.' };
    return { ...current, io, waits, truncated };
  } catch { return { pid: identity.pid, unavailable: 'Native process counters unavailable.' }; }
}

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
/** Local opt-in scalar telemetry only; never retain arbitrary stderr lines. */
function observeMeasurementLines(child: ChildProcess, receive: (line: string) => void) {
  let partial = '', oversized = false;
  child.stderr!.setEncoding('utf8');
  child.stderr!.on('data', (chunk: string) => {
    let at = 0;
    while (at < chunk.length) {
      const newline = chunk.indexOf('\n', at), end = newline < 0 ? chunk.length : newline;
      if (!oversized) {
        if (partial.length + end - at > 4096) { partial = ''; oversized = true; }
        else partial += chunk.slice(at, end);
      }
      if (newline < 0) break;
      if (!oversized && partial.startsWith('{')) {
        try {
          const record = JSON.parse(partial) as Record<string, unknown>;
          if (record.adapter_profile === 1 || record.kind === 'source_auth_profile' || record.kind === 'provider_measurement' || record.kind === 'provider_request_measurement') receive(partial);
        } catch { /* Ordinary non-JSON diagnostics do not enter the measurement. */ }
      }
      partial = ''; oversized = false; at = newline + 1;
    }
  });
}
export async function startDogfood(scenario = 'web_console_dogfood', measurement?: {
  line: (line: string) => void;
  process?: (process: { role: 'native' | 'provider_launcher'; pid: number }) => void;
  /** Runs after both original subprocesses exit, before fixture deletion. */
  afterExit?: () => void | Promise<void>;
}) {
  const directory = mkdtempSync(join(tmpdir(), 'rustx-web-console-'));
  const settings = join(directory, 'rustx.toml');
  const workspaceA = join(directory, 'A'), workspaceB = join(directory, 'B');
  mkdirSync(workspaceA); mkdirSync(workspaceB);
  const binary = process.env.RUSTX_BINARY ?? resolve(root, 'target/debug/rustx');
  const fixtureHome = join(directory, 'home'); mkdirSync(fixtureHome);
  const env = { ...process.env, HOME: fixtureHome, RUSTX_CONSOLE_FIXTURE_KEY: 'fake-provider-only' };
  const provider = spawn('uv', ['run', '--project', resolve(root, 'test-support/fake-provider'), '--frozen', 'fake-provider', '--scenario', scenario, '--port', '0'], { stdio: ['pipe', 'pipe', 'pipe'] });
  if (provider.pid !== undefined) measurement?.process?.({ role: 'provider_launcher', pid: provider.pid });
  if (measurement) observeMeasurementLines(provider, measurement.line);
  let providerErrors = ''; provider.stderr.on('data', chunk => { providerErrors = (providerErrors + String(chunk)).slice(-16_384); });
  let app: ChildProcess | undefined;
  let appIdentity: ReturnType<typeof nativeIdentity>;
  try {
    const providerUrl = await readiness(provider, 'stdout', line => {
      const message = JSON.parse(line); return message.ready ? `http://${message.host}:${message.port}` : undefined;
    });
    const primaryModel = scenario === 'tui_multi_session' ? 'integration-model' : 'console-model';
    const profiles = scenario === 'tui_multi_session' ? '\ndefault_profile = "high"\n[models."fixture/integration-model".profiles.high]\nreasoning_enabled = true\nrequest_params = \'{"reasoning_effort":"high"}\'\n[models."fixture/integration-model".profiles.low]\nreasoning_enabled = false\nmax_output_tokens = 1024\n' : '';
    const catalog = `[providers.fixture]\nbase_url = "${providerUrl}/v1"\napi_key = "$RUSTX_CONSOLE_FIXTURE_KEY"\n` +
      [primaryModel, 'second-model'].map(id => `\n[models."fixture/${id}"]\nprovider = "fixture"\nid = "${id}"\nprotocol = "openai_chat_completions"\ncontext_window = 128000\nmax_output_tokens = 4096\ncapabilities = { input_modalities = ["text"], output_modalities = ["text"], tool_calls = true, reasoning = ${scenario === 'tui_multi_session'} }\ncompat = { chat_reasoning_replay = "omit" }\n${id === primaryModel ? profiles : ''}`).join('');
    if (['web_chat_history', 'web_child_capabilities'].includes(scenario)) {
      const agents = join(fixtureHome, 'rustx/.agents');
      mkdirSync(agents, { recursive: true });
      writeFileSync(join(agents, 'mcp.toml'), `[mcp_servers.image_fixture]\ntype = "stdio"\ncommand = "python3"\nargs = [${JSON.stringify(resolve(root, 'web-console/test/e2e/image-mcp.py'))}]\n`);
    }
    const imageSource = ['web_chat_history', 'web_child_capabilities'].includes(scenario) ? `
[mcp_tool_policies.image_fixture]
approval = "never"
[agent.tools.sources]
image_fixture = ["render_image"]
` : ['web_composer_context', 'web_harness_convergence'].includes(scenario) ? `
[agent.plugins.todo]
enabled = true
[agent.plugins.goal]
enabled = true
[agent.plugins.agent_status]
enabled = true
[agent.plugins.agent_status.time]
enabled = true
timezone = "UTC"
` : '';
    const writeSettings = (model = primaryModel, modelFields = '') => writeFileSync(settings, (scenario === 'web_compaction' ? '[context]\nreserve_tokens = 0\nkeep_recent_tokens = 0\n' : '') + catalog + `[model_timeout_policy]\nresponse_start_timeout_ms = 600000\nstream_idle_timeout_ms = 600000\n[native_tools.bash]\napproval = "always"\n[agent.tools]\nbuiltin = ["read", "write", "edit", "glob", "grep", "bash", "present", "ask_user", "job_list", "job_status", "job_wait", "job_cancel"]\n[agent.model]\nmodel = "fixture/${model}"\n${modelFields}` + imageSource);
    writeSettings();
    if (['web_child_capabilities', 'web_large_write'].includes(scenario)) writeFileSync(settings, readFileSync(settings, 'utf8') + '\n[native_tools.write]\napproval = "never"\n');
    // Automatically repairable sources (#464): the App Server repairs the User
    // document at startup and Workspace A before its first Session resolves.
    if (scenario === 'web_config_repair') {
      writeFileSync(settings, 'schema_version = 8\nlegacy_root = true\n' + readFileSync(settings, 'utf8') + '\n[agent.plugins.todo]\nenabled = true\nunknown_permission = true\n');
      writeFileSync(join(workspaceA, 'rustx.toml'), '# Workspace A\n[agent]\nfuture_trust = true\n');
    }
    if (['web_agent_continuation', 'web_child_capabilities'].includes(scenario)) {
      mkdirSync(join(workspaceA, '.agents/agents'), { recursive: true });
      writeFileSync(join(workspaceA, '.agents/agents/reviewer.toml'), 'description = "Continuable reviewer"\ninstructions = "Review requests carefully."\n' + (scenario === 'web_child_capabilities' ? '[tools]\nbuiltin = ["read", "write", "present"]\n[tools.sources]\nimage_fixture = ["render_image"]\n' : ''));
      writeFileSync(join(workspaceA, 'rustx.toml'), '[agent]\nagents = ["reviewer"]\n');
    }
    if (['web_workflow_conformance', 'web_workflow_review'].includes(scenario)) {
      mkdirSync(join(workspaceA, '.agents/agents'), { recursive: true });
      mkdirSync(join(workspaceA, '.agents/workflows'), { recursive: true });
      mkdirSync(join(workspaceA, '.agents/skills/acceptance'), { recursive: true });
      writeFileSync(join(workspaceA, '.agents/agents/reviewer.toml'), 'description = "Workflow-only reviewer"\ninstructions = "Review requests carefully."\n');
      writeFileSync(join(workspaceA, '.agents/workflows/review_pr.yaml'), readFileSync(resolve(root, scenario === 'web_workflow_review' ? 'web-console/test/e2e/workflow_review.yaml' : 'web-console/test/e2e/review_pr.yaml')));
      writeFileSync(join(workspaceA, '.agents/skills/acceptance/SKILL.md'), '---\nname: acceptance\ndescription: Inspect the native acceptance fixture.\n---\nUse native authority.\n');
      writeFileSync(join(workspaceA, 'rustx.toml'), '[agent]\nworkflows = ["review_pr"]\n');
    }
    const token = randomBytes(32).toString('base64url');
    const tokenFile = join(directory, 'socket-token'); writeFileSync(tokenFile, token, { mode: 0o600 });
    const productHostToken = randomBytes(32).toString('base64url');
    const productHostTokenFile = join(directory, 'product-host-token'); writeFileSync(productHostTokenFile, productHostToken, { mode: 0o600 });
    let appErrors = '';
    const startApp = async (listen = 'ws://127.0.0.1:0') => {
      app = spawn(binary, ['app-server', '--config', settings, '--runtime-root', join(directory, 'runtime'), '--listen', listen, '--token-file', tokenFile, '--product-host-token-file', productHostTokenFile], { env, stdio: ['pipe', 'pipe', 'pipe'] });
      appIdentity = nativeIdentity(app.pid);
      if (app.pid !== undefined) measurement?.process?.({ role: 'native', pid: app.pid });
      if (measurement) observeMeasurementLines(app, measurement.line);
      app.stderr!.on('data', chunk => { appErrors = (appErrors + String(chunk)).slice(-16_384); });
      return readiness(app, 'stderr', appServerEndpoint);
    };
    const endpoint = await startApp();
    const control = async (path: string, method = 'GET') => {
      const response = await fetch(`${providerUrl}/__control/${path}`, { method });
      if (!response.ok) throw new Error(`Provider barrier failed: ${await response.text()}`);
      return response.json();
    };
    const hostConfig = { transportToken: token, productHostToken, endpoint: new URL(endpoint).href, picker: true, metadataFile: join(directory, 'workspaces.json'), roots: [
      { id: 'root-a', cwd: workspaceA, displayName: 'Workspace A' }, { id: 'root-b', cwd: workspaceB, displayName: 'Workspace B' },
    ] };
    const workspaceHost = await startWorkspaceHost(hostConfig);
    const hostConfigFile = join(directory, 'host-config.json');
    writeFileSync(hostConfigFile, JSON.stringify(hostConfig), { mode: 0o600 });
    return { hostConfigFile, workspaceHost, workspaceHostUrl: workspaceHost.url, directory, endpoint, token, tokenFile, workspaceA, workspaceB, providerUrl, settings, writeSettings, control,
      gate: (name: string) => control(`observations/await?kind=gate_reached&name=${name}&timeoutMs=30000`),
      release: (name: string) => control(`gates/${name}/release`, 'POST'),
      diagnostics: () => ({ providerErrors, appErrors, native: nativeDiagnostics(appIdentity) }),
      /** Public creation materializes defaults. Seed inherited intent only
       * before the browser connects, with the native owner physically stopped. */
      async seedInheritedSession(modelFields: string) {
        const { AppServerHost } = await import('../../../tui/src/app-server/host');
        const remote = await AppServerHost.connectRemote({ endpoint, token });
        let id: string;
        try { id = (await remote.createSession({ cwd: workspaceA })).session.id; }
        finally { await remote.shutdown(); }
        app!.kill('SIGTERM');
        await exited(app!);
        const catalogPath = join(directory, 'runtime/sessions/catalog.json');
        const catalog = JSON.parse(readFileSync(catalogPath, 'utf8'));
        if (!catalog.sessions[id].state.model) throw new Error('New Session must materialize its default before inherited fixture setup.');
        delete catalog.sessions[id].state.model;
        writeFileSync(catalogPath, JSON.stringify(catalog));
        writeSettings(primaryModel, modelFields);
        const restarted = await startApp(endpoint);
        if (new URL(restarted).href !== new URL(endpoint).href) throw new Error('Fixture restart changed the native endpoint.');
        return id;
      },
      async stop(check = true) {
        try {
          const report = await control('shutdown', 'POST');
          app!.kill('SIGTERM'); provider.stdin.end();
          const [providerCode] = await Promise.all([exited(provider), exited(app!)]);
          await measurement?.afterExit?.();
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
