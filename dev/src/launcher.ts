import { accessSync, constants, mkdtempSync, writeFileSync, rmSync, statSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import type { LocalHostConfig } from '../../web-console/host/workspaces.ts';
import type { Arguments } from './arguments.ts';
import { spawnOwned, type Spawn, type OwnedChild, type ChildSpec } from './process.ts';

export interface WebReady { url: string; endpoint: string; tokenFile: string; hostConfigFile: string; workspaces: string[] }

/** Development composition only. No native settings are read or resolved here. */
export class Launcher {
  readonly done: Promise<number>;
  readonly #root: string;
  readonly #spawn: Spawn;
  readonly #children: OwnedChild[] = [];
  readonly #abort = new AbortController();
  #resolveDone!: (code: number) => void;
  #terminal: Promise<number> | undefined;
  #directory: string | undefined;
  #started = false;

  constructor(root: string, spawn: Spawn = spawnOwned) {
    this.#root = root; this.#spawn = spawn;
    this.done = new Promise(resolve => { this.#resolveDone = resolve; });
  }
  #active() { if (this.#abort.signal.aborted) throw new Error('Development composition is settling'); }
  #scratch() {
    this.#active();
    return this.#directory ??= mkdtempSync(join(tmpdir(), 'rustx-dev-'));
  }
  #write(name: string, content: string) {
    this.#active();
    const path = join(this.#scratch(), name);
    writeFileSync(path, content, { mode: 0o600, flag: 'wx' });
    return path;
  }
  #child(spec: ChildSpec, standalone = false) {
    this.#active();
    const child = this.#spawn(spec, code => { void this.settle(standalone ? code : code || 1); });
    this.#children.push(child);
    return child;
  }
  async #ready(child: OwnedChild) {
    const signal = this.#abort.signal;
    this.#active();
    let abort!: () => void;
    const cancelled = new Promise<never>((_, reject) => {
      abort = () => reject(new Error('Development composition is settling'));
      signal.addEventListener('abort', abort, { once: true });
    });
    try { const result = await Promise.race([child.ready, cancelled]); this.#active(); return result; }
    finally { signal.removeEventListener('abort', abort); }
  }
  async start(args: Arguments): Promise<WebReady | undefined> {
    if (this.#started) throw new Error('Launcher can only start once');
    this.#started = true;
    try {
      this.#active();
      try { accessSync(args.binary, constants.X_OK); }
      catch { throw new Error(`Native binary unavailable: ${args.binary}. Run cargo build --bins, or pass --binary /absolute/path/rustx.`); }
      if (args.mode === 'tui') {
        this.#child({ component: 'tui', command: process.execPath, args: [join(this.#root, 'tui/src/main.ts'), '--binary', args.binary, ...args.forwarded], cwd: process.cwd(), terminal: true }, true);
        return;
      }
      if (args.mode === 'app-server') {
        const native = [...args.forwarded];
        if (!native.some((argument, index) => index % 2 === 0 && argument === '--listen')) native.push('--listen', 'stdio');
        this.#child({ component: 'app-server', command: args.binary, args: ['app-server', ...native], cwd: process.cwd(), protocolStdio: true }, true);
        return;
      }
      for (const workspace of args.workspaces) if (!statSync(workspace).isDirectory()) throw new Error(`Workspace is not a directory: ${workspace}`);
      this.#active();
      const tokenFile = this.#write('transport-token', randomBytes(32).toString('base64url'));
      const endpoint = await this.#ready(this.#child({ component: 'app-server', command: args.binary,
        args: ['app-server', ...args.forwarded, '--listen', 'ws://127.0.0.1:0', '--token-file', tokenFile], cwd: process.cwd(), readiness: 'app-server' }));
      const config: LocalHostConfig = { endpoint, picker: true, metadataFile: join(this.#scratch(), 'workspaces.json'),
        roots: args.workspaces.map((cwd, index) => ({ id: `root-${index + 1}`, cwd, displayName: basename(cwd) || cwd })) };
      const hostConfigFile = this.#write('host-config.json', JSON.stringify(config));
      const url = await this.#ready(this.#child({ component: 'web', command: process.execPath,
        args: [join(this.#root, 'web-console/scripts/dev-carrier.ts')], cwd: join(this.#root, 'web-console'),
        env: { ...process.env, RUSTX_WORKSPACE_HOST_CONFIG: hostConfigFile }, readiness: 'web' }));
      return { url, endpoint, tokenFile, hostConfigFile, workspaces: args.workspaces };
    } catch (error) {
      if (!this.#abort.signal.aborted) process.stderr.write(`[dev] ${String(error)}\n`);
      await this.settle(1);
      return;
    }
  }
  settle(code: number): Promise<number> {
    if (this.#terminal) return this.#terminal;
    // Linearization point: synchronous fence BEFORE any cleanup await. Resource
    // allocation and ownership registration are synchronous, never in flight here.
    this.#abort.abort();
    this.#terminal = Promise.resolve().then(async () => {
      const results = await Promise.allSettled(this.#children.map(child => child.stop()));
      for (const result of results) if (result.status === 'rejected') {
        process.stderr.write(`[dev] cleanup: ${String(result.reason)}\n`); code = 1;
      }
      try { if (this.#directory) rmSync(this.#directory, { recursive: true, force: true }); }
      catch (error) { process.stderr.write(`[dev] scratch cleanup: ${String(error)}\n`); code = 1; }
      this.#resolveDone(code);
      return code;
    });
    return this.#terminal;
  }
}
