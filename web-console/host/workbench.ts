/** Host-owned workspace browsing and PTYs. Never imported into the browser. */
import { constants, openSync, closeSync, fstatSync, readSync, existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import { terminalProcess, type TerminalProcess } from './terminal-process.ts';
import type { WorkbenchRequest, WorkbenchResult } from '../src/workspaces/workbench.ts';
export const workspaceDescriptors = createRequire(import.meta.url)('./workspace-fs/build/Release/workspace_fs.node') as {
  openChild(fd: number, name: string, directory: boolean): number;
  entries(fd: number): { name: Buffer; directory: boolean; link: boolean }[];
};
const MAX_OUTPUT = 256 * 1024;
interface TerminalState { owner: string; shell: string; pty: TerminalProcess; stopping?: Promise<void>; failure?: string; output: string; offset: number; exited: boolean; listeners: Set<() => void>; done: Promise<void> }
/** Descriptor-relative traversal rejects symlinks at every component, including
 * ancestors of the authorized root. POSIX openat/fdopendir work on Linux and macOS. */
export function withWorkspacePath<T>(cwd: string, path: string, directory: boolean, action: (fd: number) => T): T {
  if (typeof path !== 'string' || path.length > 4096 || path.startsWith('/') || path.split('/').some(p => p === '..' || p === '.' || p.includes('\0'))) throw new Error('Invalid workspace path');
  let fd = openSync('/', constants.O_RDONLY | constants.O_DIRECTORY);
  try {
    const parts = [...cwd.split('/').filter(Boolean), ...path.split('/').filter(Boolean)];
    parts.forEach((part, index) => {
      const next = workspaceDescriptors.openChild(fd, part, index < parts.length - 1 || directory);
      closeSync(fd); fd = next;
    });
    return action(fd);
  } finally { closeSync(fd); }
}
export function workspaceFile(cwd: string, path: string, read: boolean, binary = false): WorkbenchResult {
  return withWorkspacePath(cwd, path, !read, fd => {
    if (!read) {
      const entries = workspaceDescriptors.entries(fd).map(entry => ({ ...entry, name: new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(entry.name) }));
      return { cwd, entries: entries.sort((a, b) => Number(b.directory) - Number(a.directory) || a.name.localeCompare(b.name)) };
    }
    const limit = binary ? 16 * 1024 * 1024 : 1024 * 1024;
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size > limit) throw new Error(binary ? 'Preview supports regular files up to 16 MiB' : 'Preview supports regular text files up to 1 MiB');
    const bytes = Buffer.alloc(limit + 1); let size = 0;
    while (size < bytes.length) { const count = readSync(fd, bytes, size, bytes.length - size, null); if (!count) break; size += count; }
    if (size > limit || (!binary && bytes.subarray(0, size).includes(0))) throw new Error(binary ? 'Preview supports files up to 16 MiB' : 'Preview supports text files up to 1 MiB');
    if (binary) return { cwd, base64: bytes.subarray(0, size).toString('base64') };
    return { cwd, text: new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(0, size)) };
  });
}

export class WorkspaceTerminals {
  private terminals = new Map<string, TerminalState>();
  private closed = false;
  private closing?: Promise<void>;
  private readonly removed = new Map<string, string>();
  private readonly supervisor?: string;
  private readonly spawnTerminal: typeof terminalProcess;
  constructor(supervisor?: string, spawnTerminal = terminalProcess) { this.supervisor = supervisor; this.spawnTerminal = spawnTerminal; }
  readonly shells = ['/bin/bash', '/bin/zsh', '/bin/sh'].filter(existsSync);
  close(): Promise<void> {
    this.closed = true;
    return this.closing ??= this.settle([...this.terminals]);
  }
  async retireSession(session: string) {
    return this.settle([...this.terminals].filter(([, terminal]) => JSON.parse(terminal.owner)[0] === session));
  }
  private async settle(terminals: [string, TerminalState][]) {
    const results = await Promise.allSettled(terminals.map(([id, terminal]) => this.stop(id, terminal)));
    const failures = results.flatMap(result => result.status === 'rejected' ? [result.reason] : []);
    if (failures.length) throw new AggregateError(failures, 'Terminal settlement failed');
  }
  private stop(id: string, terminal: TerminalState): Promise<void> {
    return terminal.stopping ??= (async () => {
      await terminal.pty.stop();
      this.terminals.delete(id);
      this.removed.set(id, terminal.owner);
      while (this.removed.size > 128) this.removed.delete(this.removed.keys().next().value!);
    })();
  }
  async request(owner: string, cwd: string, request: Exclude<WorkbenchRequest, { kind: 'applications' | 'open' | 'office' | 'resolve' }>, signal?: AbortSignal): Promise<WorkbenchResult> {
    if (this.closed) throw new Error('Terminal Host is closed');
    if (request.kind === 'files' || request.kind === 'read' || request.kind === 'bytes') return workspaceFile(cwd, request.path, request.kind !== 'files', request.kind === 'bytes');
    const list = () => ({ terminals: [...this.terminals].filter(([, t]) => t.owner === owner).map(([id, t]) => ({ id, shell: t.shell, exited: t.exited })), shells: this.shells });
    if (request.kind === 'terminals') return list();
    if (typeof request.id !== 'string' || !/^[a-f0-9-]{36}$/.test(request.id)) throw new Error('Invalid terminal identity');
    if (request.kind === 'create') {
      if (!this.supervisor) throw new Error('Native terminal supervisor unavailable');
      if (this.removed.has(request.id)) throw new Error('Terminal identity has settled');
      if (!this.shells.includes(request.shell)) throw new Error('Unsupported shell');
      const previous = this.terminals.get(request.id);
      if (previous) { if (previous.owner !== owner || previous.shell !== request.shell || previous.stopping || previous.failure) throw new Error('Terminal identity conflict'); await previous.pty.ready; signal?.throwIfAborted(); if (this.closed || previous.stopping) throw new Error('Terminal owner retired during creation'); return list(); }
      // Retain at most eight output buffers, evicting only proven settled units.
      if (this.terminals.size >= 8) { for (const [id, terminal] of this.terminals) if (terminal.exited) { await this.stop(id, terminal); break; } }
      if (this.closed) throw new Error('Terminal Host is closed');
      if (this.terminals.size >= 8) throw new Error('Close a terminal before opening another (maximum 8)');
      signal?.throwIfAborted();
      const pty = this.spawnTerminal(this.supervisor, request.shell, cwd, data => {
        t.output += data; if (t.output.length > MAX_OUTPUT) { const removed = t.output.length - MAX_OUTPUT; t.output = t.output.slice(removed); t.offset += removed; }
        t.listeners.forEach(fn => fn());
      });
      const t: TerminalState = { owner, pty, shell: request.shell, output: '', offset: 0, exited: false, listeners: new Set(), done: pty.done };
      this.terminals.set(request.id, t);
      void pty.done.then(() => { t.exited = true; t.listeners.forEach(fn => fn()); }, error => { t.failure = String(error); t.listeners.forEach(fn => fn()); });
      try { await pty.ready; signal?.throwIfAborted(); if (this.closed || t.stopping) throw new Error('Terminal owner retired during creation'); }
      catch (error) { await this.stop(request.id, t); throw error; }
      return list();
    }
    const terminal = this.terminals.get(request.id);
    if (!terminal && request.kind === 'close' && this.removed.get(request.id) === owner) return list();
    if (!terminal || terminal.owner !== owner) throw new Error('Terminal does not belong to this Session');
    if (request.kind === 'close') { await this.stop(request.id, terminal); return list(); }
    if (terminal.failure) throw new Error(terminal.failure);
    if (terminal.stopping) throw new Error('Terminal is closing');
    if (request.kind === 'input') { if (typeof request.data !== 'string' || request.data.length > 16384 || terminal.exited) throw new Error('Terminal input unavailable'); terminal.pty.write(request.data); return {}; }
    if (request.kind === 'resize') { if (![request.cols, request.rows].every(n => Number.isInteger(n) && n >= 1 && n <= 500)) throw new Error('Invalid terminal size'); if (!terminal.exited) terminal.pty.resize(request.cols, request.rows); return {}; }
    if (request.kind !== 'poll' || !Number.isSafeInteger(request.cursor) || request.cursor < 0 || request.cursor > terminal.offset + terminal.output.length) throw new Error('Invalid terminal cursor');
    if (request.cursor === terminal.offset + terminal.output.length && !terminal.exited && !terminal.failure) await new Promise<void>(resolve => {
      const done = () => { clearTimeout(timeout); terminal.listeners.delete(done); signal?.removeEventListener('abort', done); resolve(); };
      const timeout = setTimeout(done, 25000);
      terminal.listeners.add(done); signal?.addEventListener('abort', done, { once: true }); if (signal?.aborted) done();
    });
    signal?.throwIfAborted();
    if (terminal.failure) throw new Error(terminal.failure);
    return { output: terminal.output.slice(Math.max(0, request.cursor - terminal.offset)), cursor: terminal.offset + terminal.output.length, reset: request.cursor < terminal.offset, exited: terminal.exited };
  }
}
