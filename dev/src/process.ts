import { appServerEndpoint } from './app-server-readiness.ts';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { StringDecoder } from 'node:string_decoder';

export interface ChildSpec {
  component: 'app-server' | 'tui' | 'web';
  command: string;
  args: string[];
  cwd: string;
  env?: NodeJS.ProcessEnv;
  readiness?: 'app-server' | 'web';
  terminal?: boolean;
  protocolStdio?: boolean;
  /** Owner lifetime, independent of protocol stream forwarding. */
  ownerStdin?: 'shutdown-on-eof';
}
export interface OwnedChild {
  readonly pid?: number;
  ready: Promise<string>;
  stop(): Promise<void>;
}
export type Spawn = (spec: ChildSpec, exited: (code: number) => void, ownerShutdown: () => void) => OwnedChild;

/** Direct executables only: no pnpm/shell wrapper between us and the owner. */
export const spawnOwned: Spawn = (spec, exited, ownerShutdown) => {
  if (process.platform === 'win32') throw new Error('The native rustX runtime currently requires Unix (Linux/macOS); use WSL on Windows.');
  const child = spawn(spec.command, spec.args, {
    cwd: spec.cwd, env: spec.env ?? process.env, shell: false,
    detached: true,
    stdio: spec.terminal ? ['inherit', 'inherit', 'inherit', 'ipc']
      : spec.readiness === 'web' ? ['ignore', 'pipe', 'pipe', 'ipc'] : ['pipe', 'pipe', 'pipe'],
  });
  let resolveReady!: (value: string) => void, rejectReady!: (error: Error) => void;
  let readySettled = false;
  const ready = new Promise<string>((resolve, reject) => { resolveReady = resolve; rejectReady = reject; });
  // A child may fail before composition reaches its readiness await.
  void ready.catch(() => {});
  const finishReady = (value: string | Error) => {
    if (readySettled) return;
    readySettled = true;
    clearTimeout(deadline);
    if (value instanceof Error) rejectReady(value); else resolveReady(value);
  };
  const deadline = spec.readiness ? setTimeout(() => finishReady(new Error(`[${spec.component}] readiness deadline exceeded`)), 60_000) : undefined;
  let closed = false;
  const close = new Promise<void>(resolve => child.once('close', () => { closed = true; resolve(); }));
  child.once('error', error => {
    process.stderr.write(`[${spec.component}] ${error.message}\n`);
    finishReady(error); exited(1);
  });
  child.once('exit', (code, signal) => {
    finishReady(new Error(`[${spec.component}] exited before readiness (${code ?? signal})`));
    exited(code ?? 1);
  });
  if (!spec.readiness) child.once('spawn', () => finishReady(''));
  child.on('message', message => {
    if (spec.readiness !== 'web') return;
    if (typeof message === 'object' && message !== null && 'ready' in message && typeof message.ready === 'string'
      && /^http:\/\/127\.0\.0\.1:\d+\/$/.test(message.ready)) finishReady(message.ready);
  });
  // Readiness uses the native startup contract, bounded independently of logs.
  let line = '', discardLine = false;
  const decoder = new StringDecoder('utf8');
  child.stderr?.on('data', (chunk: Buffer) => {
    process.stderr.write(`[${spec.component}] ${chunk.toString()}`);
    if (spec.readiness !== 'app-server' || readySettled) return;
    for (const part of decoder.write(chunk)) {
      if (part === '\n') {
        const endpoint = discardLine ? undefined : appServerEndpoint(line);
        if (endpoint) finishReady(new URL(endpoint).href);
        line = ''; discardLine = false;
      } else if (line.length < 512) line += part;
      else discardLine = true;
    }
  });
  child.stdout?.on('data', (chunk: Buffer) => {
    if (spec.protocolStdio) process.stdout.write(chunk);
    else process.stdout.write(`[${spec.component}] ${chunk.toString()}`);
  });
  child.stdin?.on('error', () => {});
  let observingStdin = spec.ownerStdin === 'shutdown-on-eof';
  const stdinEnded = () => { if (observingStdin) ownerShutdown(); };
  if (observingStdin) {
    process.stdin.once('end', stdinEnded);
    // Defer until the composition has registered this child's ownership.
    if (process.stdin.readableEnded) queueMicrotask(stdinEnded);
  }
  if (spec.protocolStdio) process.stdin.pipe(child.stdin!);

  let stopping: Promise<void> | undefined;
  return { ready, pid: child.pid, stop() {
    return stopping ??= (async () => {
      finishReady(new Error(`[${spec.component}] stopping`));
      observingStdin = false;
      process.stdin.off('end', stdinEnded);
      if (spec.protocolStdio) { process.stdin.unpipe(child.stdin!); process.stdin.pause(); }
      child.stdin?.end();
      const signalGroup = (signal: NodeJS.Signals) => {
        if (!child.pid) return;
        try { process.kill(-child.pid, signal); }
        catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error; }
      };
      // IPC asks the carrier/TUI to close and reap its own resources. Native
      // receives SIGTERM for its drain. Escalation is a deadline, not readiness.
      if (child.connected) child.send({ stop: true }, () => {});
      else if (!closed) child.kill('SIGTERM');
      const escalation = setTimeout(() => signalGroup('SIGKILL'), 10_000);
      try {
        if (!closed) await close; // close proves exit AND drained inherited pipes
        // An exited group leader must not leave a surviving descendant.
        signalGroup('SIGKILL');
        // SIGKILL delivery is not settlement. Observe group disappearance before
        // declaring cleanup complete, including descendants without our pipes.
        if (child.pid) for (;;) {
          try { process.kill(-child.pid, 0); }
          catch (error) { if ((error as NodeJS.ErrnoException).code === 'ESRCH') break; throw error; }
          await delay(10);
        }
      } finally { clearTimeout(escalation); }
    })();
  } };
};
