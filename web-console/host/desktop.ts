/** Node-only desktop adapter. Session and Workspace ownership stay elsewhere. */
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { accessSync, constants, realpathSync, statSync } from 'node:fs';
import { delimiter, isAbsolute, join } from 'node:path';
import type { DesktopApplication, DesktopAppId, DesktopCatalog, DesktopLaunch } from '../src/workspaces/desktop.ts';

export interface DesktopProcess { command: string; args: string[]; cwd: string; env: NodeJS.ProcessEnv }
export interface DesktopSystem {
  platform: NodeJS.Platform;
  env: NodeJS.ProcessEnv;
  macOSDesktop?(): boolean;
  executable(path: string): string | undefined;
  launch(spec: DesktopProcess): Promise<DesktopLaunch>;
}
/** Observe early launcher failure for one second; never kill a handed-off app.
 * A running launcher at the bound is only a spawn acknowledgement, not GUI proof. */
export function launchDesktop(spec: DesktopProcess, spawnProcess = spawn): Promise<DesktopLaunch> {
  return new Promise((resolve, reject) => {
    let child: ChildProcess;
    try { child = spawnProcess(spec.command, spec.args, { cwd: spec.cwd, env: spec.env, shell: false, detached: true, stdio: 'ignore' }); }
    catch { reject(new Error('Desktop application could not be started')); return; }
    let settled = false, watch: ReturnType<typeof setTimeout> | undefined;
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true; clearTimeout(watch); child.unref();
      if (error) reject(error); else resolve({ status: 'spawned' });
    };
    child.once('error', () => finish(new Error('Desktop application could not be started')));
    child.once('exit', (code, signal) => finish(code === 0 ? undefined : new Error(`Desktop launcher failed (${signal ?? `exit ${code}`})`)));
    child.once('spawn', () => { if (!settled) watch = setTimeout(() => finish(), 1_000); });
  });
}
export function desktopEnvironment(parent: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const key of ['PATH', 'HOME', 'USER', 'LOGNAME', 'LANG', 'DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'XDG_RUNTIME_DIR', 'XDG_CURRENT_DESKTOP', 'XDG_SESSION_TYPE', 'DBUS_SESSION_BUS_ADDRESS']) {
    if (parent[key] !== undefined) env[key] = parent[key];
  }
  return env;
}
/** Conservative macOS eligibility, not proof that an application will display a window.
 * Console ownership alone also admits background jobs, so require the current
 * launchd bootstrap to be Aqua. One fixed OS query; no process enumeration. */
export function macOSDesktop(
  uid = process.getuid?.(),
  consoleOwner = () => statSync('/dev/console').uid,
  managerName = () => execFileSync('/bin/launchctl', ['managername'], {
    encoding: 'utf8', env: {}, shell: false, stdio: ['ignore', 'pipe', 'ignore'],
    timeout: 1_000, killSignal: 'SIGKILL', maxBuffer: 1024,
  }),
): boolean {
  try { return uid !== undefined && uid > 0 && consoleOwner() === uid && managerName().trim() === 'Aqua'; }
  catch { return false; }
}
const nativeSystem: DesktopSystem = {
  platform: process.platform, env: process.env, macOSDesktop,
  executable(path) { try { accessSync(path, constants.X_OK); return statSync(path).isFile() ? realpathSync(path) : undefined; } catch { return; } },
  launch: launchDesktop,
};
interface Resolved { application: DesktopApplication; command: string; required?: string; args: (cwd: string) => string[] }
export class DesktopAdapter {
  private resolved?: Resolved[];
  private readonly system: DesktopSystem;
  constructor(system: DesktopSystem = nativeSystem) { this.system = system; }
  private unavailable(): DesktopCatalog | undefined {
    const { platform, env } = this.system;
    if (!['linux', 'darwin'].includes(platform)) return { available: false, reason: 'platform' };
    if (env.SSH_CONNECTION || env.SSH_TTY || (platform === 'linux' && !env.DISPLAY && !env.WAYLAND_DISPLAY)
      || (platform === 'darwin' && !this.system.macOSDesktop?.())) return { available: false, reason: 'headless' };
  }
  private verified(entry: Resolved) {
    return this.system.executable(entry.command) === entry.command && (!entry.required || this.system.executable(entry.required) === entry.required);
  }
  /** Cached executable catalog; explicit refresh rebuilds it once.
   * Application discovery is filesystem-only; macOS eligibility has one OS query. */
  catalog(refresh = false): DesktopCatalog {
    if (refresh) this.resolved = undefined;
    const unavailable = this.unavailable();
    if (unavailable) return unavailable;
    this.resolved ??= this.discover();
    this.resolved = this.resolved.filter(entry => this.verified(entry));
    return this.resolved.length ? { available: true, applications: this.resolved.map(entry => entry.application) } : { available: false, reason: 'applications' };
  }
  private discover(): Resolved[] {
    const { platform, env, executable } = this.system;
    const entries: Resolved[] = [];
    const add = (id: DesktopAppId, label: string, candidates: string[], args: Resolved['args'], required?: string) => {
      for (const candidate of candidates) { const command = executable(candidate); if (command) { entries.push({ application: { id, label }, command, args, required }); break; } }
    };
    // Ignore relative and excessively long PATH entries. No recursive filesystem scans.
    const paths = (env.PATH ?? '').split(delimiter).filter(p => isAbsolute(p) && p.length <= 4096).slice(0, 32);
    if (platform === 'darwin') {
      add('files', 'Finder', ['/usr/bin/open'], cwd => ['--', cwd]);
      const terminal = executable('/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal');
      if (terminal) add('terminal', 'Terminal', ['/usr/bin/open'], cwd => ['-a', '/System/Applications/Utilities/Terminal.app', '--', cwd], terminal);
      add('code', 'Visual Studio Code', ['/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code'], cwd => ['--new-window', '--', cwd]);
    } else if (platform === 'linux') {
      add('files', 'File manager', paths.map(p => join(p, 'xdg-open')), cwd => [cwd]);
      add('terminal', 'GNOME Terminal', paths.map(p => join(p, 'gnome-terminal')), cwd => ['--working-directory', cwd]);
      add('code', 'Visual Studio Code', paths.map(p => join(p, 'code')), cwd => ['--new-window', '--', cwd]);
    }
    return entries;
  }
  prepare(id: DesktopAppId): (cwd: string) => Promise<DesktopLaunch> {
    const catalog = this.catalog();
    const entry = catalog.available ? this.resolved?.find(entry => entry.application.id === id) : undefined;
    if (!entry) throw new Error('Desktop application is unavailable; refresh applications and try again');
    return cwd => {
      // No await between the last executable verification and spawn.
      if (!this.verified(entry)) {
        this.resolved = this.resolved?.filter(item => item !== entry);
        throw new Error('Desktop application disappeared; refresh applications');
      }
      return this.system.launch({ command: entry.command, args: entry.args(cwd), cwd, env: desktopEnvironment(this.system.env) });
    };
  }
}
