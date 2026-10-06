/** Host-owned workspace browsing and PTYs. Never imported into the browser. */
import { constants, openSync, closeSync, readdirSync, fstatSync, readSync, existsSync } from 'node:fs';
import { spawn, type IPty } from 'node-pty';
import { desktopEnvironment } from './desktop.ts';
import type { WorkbenchRequest, WorkbenchResult } from '../src/workspaces/workbench.ts';
const MAX_OUTPUT = 256 * 1024;
interface TerminalState { owner: string; shell: string; pty: IPty; output: string; offset: number; exited: boolean; listeners: Set<() => void>; done: Promise<void> }
/** Descriptor-relative traversal rejects symlinks at every component, including
 * ancestors of the authorized root. Linux procfs exposes each held directory. */
export function workspaceFile(cwd: string, path: string, read: boolean): WorkbenchResult {
  if (process.platform !== 'linux') throw new Error('Workspace file browsing requires a Linux Product Host');
  if (typeof path !== 'string' || path.length > 4096 || path.startsWith('/') || path.split('/').some(p => p === '..' || p === '.' || p.includes('\0'))) throw new Error('Invalid workspace path');
  let fd = openSync('/', constants.O_RDONLY | constants.O_DIRECTORY);
  try {
    const parts = [...cwd.split('/').filter(Boolean), ...path.split('/').filter(Boolean)];
    parts.forEach((part, index) => {
      const directory = index < parts.length - 1 || !read;
      const next = openSync(`/proc/self/fd/${fd}/${part}`, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK | (directory ? constants.O_DIRECTORY : 0));
      closeSync(fd); fd = next;
    });
    if (!read) {
      const entries = readdirSync(`/proc/self/fd/${fd}`, { withFileTypes: true });
      if (entries.length > 2000) throw new Error('Directory exceeds 2000 entries');
      return { entries: entries.map(entry => ({ name: entry.name, directory: entry.isDirectory(), link: entry.isSymbolicLink() })).sort((a, b) => Number(b.directory) - Number(a.directory) || a.name.localeCompare(b.name)) };
    }
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size > 1024 * 1024) throw new Error('Preview supports regular text files up to 1 MiB');
    const bytes = Buffer.alloc(1024 * 1024 + 1); let size = 0;
    while (size < bytes.length) { const count = readSync(fd, bytes, size, bytes.length - size, null); if (!count) break; size += count; }
    if (size > 1024 * 1024 || bytes.subarray(0, size).includes(0)) throw new Error('Preview supports text files up to 1 MiB');
    return { text: new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(0, size)) };
  } finally { closeSync(fd); }
}
export class WorkspaceTerminals {
  private terminals = new Map<string, TerminalState>();
  readonly shells = ['/bin/bash', '/bin/zsh', '/bin/sh'].filter(existsSync);
  close() { for (const terminal of this.terminals.values()) void this.stop(terminal); this.terminals.clear(); }
  private async stop(terminal: TerminalState) { if (!terminal.exited) terminal.pty.kill('SIGKILL'); await terminal.done; }
  async request(owner: string, cwd: string, request: WorkbenchRequest, signal?: AbortSignal): Promise<WorkbenchResult> {
    if (request.kind === 'files' || request.kind === 'read') return workspaceFile(cwd, request.path, request.kind === 'read');
    const list = () => ({ terminals: [...this.terminals].filter(([, t]) => t.owner === owner).map(([id, t]) => ({ id, shell: t.shell, exited: t.exited })), shells: this.shells });
    if (request.kind === 'terminals') return list();
    if (typeof request.id !== 'string' || !/^[a-f0-9-]{36}$/.test(request.id)) throw new Error('Invalid terminal identity');
    if (request.kind === 'create') {
      if (!this.shells.includes(request.shell)) throw new Error('Unsupported shell');
      const previous = this.terminals.get(request.id);
      if (previous) { if (previous.owner !== owner || previous.shell !== request.shell) throw new Error('Terminal identity conflict'); return list(); }
      if (this.terminals.size >= 8) throw new Error('Close a terminal before opening another (maximum 8)');
      signal?.throwIfAborted();
      const pty = spawn(request.shell, [], { cwd, cols: 80, rows: 24, name: 'xterm-256color', env: { ...desktopEnvironment(process.env), TERM: 'xterm-256color', COLORTERM: 'truecolor' } });
      let exited!: () => void; const done = new Promise<void>(resolve => { exited = resolve; });
      const t: TerminalState = { owner, pty, shell: request.shell, output: '', offset: 0, exited: false, listeners: new Set(), done };
      this.terminals.set(request.id, t);
      pty.onData(data => { t.output += data; if (t.output.length > MAX_OUTPUT) { const removed = t.output.length - MAX_OUTPUT; t.output = t.output.slice(removed); t.offset += removed; } t.listeners.forEach(fn => fn()); });
      pty.onExit(() => { t.exited = true; exited(); t.listeners.forEach(fn => fn()); });
      return list();
    }
    const terminal = this.terminals.get(request.id);
    if (!terminal || terminal.owner !== owner) throw new Error('Terminal does not belong to this Session');
    if (request.kind === 'close') { await this.stop(terminal); this.terminals.delete(request.id); return list(); }
    if (request.kind === 'input') { if (typeof request.data !== 'string' || request.data.length > 16384 || terminal.exited) throw new Error('Terminal input unavailable'); terminal.pty.write(request.data); return {}; }
    if (request.kind === 'resize') { if (![request.cols, request.rows].every(n => Number.isInteger(n) && n >= 1 && n <= 500)) throw new Error('Invalid terminal size'); if (!terminal.exited) terminal.pty.resize(request.cols, request.rows); return {}; }
    if (request.kind !== 'poll' || !Number.isSafeInteger(request.cursor) || request.cursor < 0 || request.cursor > terminal.offset + terminal.output.length) throw new Error('Invalid terminal cursor');
    if (request.cursor === terminal.offset + terminal.output.length && !terminal.exited) await new Promise<void>(resolve => {
      const done = () => { clearTimeout(timeout); terminal.listeners.delete(done); signal?.removeEventListener('abort', done); resolve(); };
      const timeout = setTimeout(done, 25000);
      terminal.listeners.add(done); signal?.addEventListener('abort', done, { once: true }); if (signal?.aborted) done();
    });
    signal?.throwIfAborted();
    return { output: terminal.output.slice(Math.max(0, request.cursor - terminal.offset)), cursor: terminal.offset + terminal.output.length, reset: request.cursor < terminal.offset, exited: terminal.exited };
  }
}
