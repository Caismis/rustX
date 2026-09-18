import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

/** Explicit desktop handoff environment. Native/provider credentials never propagate. */
export function browserEnvironment(parent: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const result: NodeJS.ProcessEnv = {};
  for (const key of ['PATH', 'HOME', 'USER', 'LOGNAME', 'LANG', 'DISPLAY', 'WAYLAND_DISPLAY', 'XAUTHORITY', 'XDG_RUNTIME_DIR', 'XDG_CURRENT_DESKTOP', 'XDG_SESSION_TYPE', 'DBUS_SESSION_BUS_ADDRESS', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP']) {
    if (parent[key] !== undefined) result[key] = parent[key];
  }
  return result;
}
export function openBrowser(url: string, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) { reject(new Error('Browser handoff cancelled')); return; }
    const child = spawn(process.execPath, [fileURLToPath(new URL('./browser-worker.ts', import.meta.url)), url], { env: browserEnvironment(process.env), stdio: 'ignore' });
    let failed = false;
    const stop = () => { failed = true; child.kill('SIGKILL'); };
    const timer = setTimeout(stop, 10_000);
    signal?.addEventListener('abort', stop, { once: true });
    child.once('error', () => { failed = true; });
    child.once('close', code => { clearTimeout(timer); signal?.removeEventListener('abort', stop); if (code === 0 && !failed) resolve(); else reject(new Error('Browser handoff failed')); });
  });
}

export async function handoff(url: string, noOpen: boolean, open = openBrowser, print = console.log, warn = console.error) {
  print(`[dev] rustX Web: ${url}`);
  if (!noOpen) {
    try { await open(url); }
    catch { warn('[dev] Could not open the browser. Open the startup URL printed above.'); }
  }
}
