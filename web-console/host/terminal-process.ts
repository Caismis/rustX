/** PTY transport around the existing native process ownership supervisor.
 * Only ALL_CHILDREN_REAPED/NO_OWNERSHIP followed by outer exit settles a unit.
 * The PTY's direct exit alone is deliberately insufficient. */
import { createServer, type Socket } from 'node:net';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn, type IPty } from 'node-pty';
import { desktopEnvironment } from './desktop.ts';
export interface TerminalProcess {
  ready: Promise<void>;
  done: Promise<void>;
  stop(): Promise<void>;
  write(data: string): void;
  resize(cols: number, rows: number): void;
}
function deferred() {
  let resolve!: () => void, reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  void promise.catch(() => {});
  return { promise, resolve, reject };
}
export function terminalProcess(binary: string, shell: string, cwd: string, output: (data: string) => void): TerminalProcess {
  const ready = deferred(), done = deferred();
  const directory = mkdtempSync(join(tmpdir(), 'rustx-pty-'));
  const path = join(directory, 'control');
  let pty: IPty | undefined, socket: Socket | undefined, stopping = false, proof = false, exited = false;
  let input = Buffer.alloc(0);
  const fail = (error: Error) => { ready.reject(error); done.reject(error); };
  const send = (kind: number) => socket?.write(Buffer.from([1, 0, 0, 0, kind]));
  const server = createServer(connection => {
    if (socket) { connection.destroy(); return; }
    socket = connection;
    connection.on('error', fail);
    connection.on('close', () => { if (!proof) fail(new Error('Terminal supervisor lost its settlement channel')); });
    connection.on('data', chunk => {
      input = Buffer.concat([input, chunk]);
      while (input.length >= 4) {
        const size = input.readUInt32LE();
        if (size < 1 || size > 65536) { fail(new Error('Invalid terminal supervisor frame')); connection.destroy(); return; }
        if (input.length < 4 + size) break;
        const kind = input[4], payload = input.subarray(5, size + 4); input = input.subarray(size + 4);
        if (kind === 0x06) { // Native anchor retained; admit the shell only while owned.
          if (payload.length !== 4 || payload.readInt32LE() <= 0) { fail(new Error('Invalid terminal anchor')); connection.destroy(); return; }
          send(stopping ? 0x10 : 0x11);
        } else if (kind === 0x07) {
          ready.resolve(); if (stopping) send(0x10);
        } else if (kind === 0x03 || kind === 0x08) {
          proof = true; send(0x12);
          ready.reject(new Error('Terminal ended before execution admission'));
          if (exited) done.resolve();
        } else if (kind === 0x04) {
          fail(new Error(`Terminal process control failed: ${payload.toString('utf8')}`));
          stopping = true; send(0x10);
        } else if (kind !== 0x05 && kind !== 0x02) {
          fail(new Error('Unknown terminal supervisor frame')); stopping = true; send(0x10);
        }
      }
    });
    send(0x13);
    if (stopping) send(0x10);
  });
  server.on('error', error => {
    ready.reject(error);
    // A listen failure precedes spawn and owns no process capacity.
    if (pty) done.reject(error); else done.resolve();
    rmSync(directory, { recursive: true, force: true });
  });
  server.listen(path, () => {
    try {
      const environment = { ...desktopEnvironment(process.env), TERM: 'xterm-256color', COLORTERM: 'truecolor' };
      pty = spawn(binary, ['outer', shell, '-i'], { cwd, cols: 80, rows: 24, name: 'xterm-256color', env: {
        RUSTX_INTERACTIVE_CONTROL: path, RUSTX_COMMAND_ENVIRONMENT: JSON.stringify(Object.entries(environment)),
      } });
      const data = pty.onData(output);
      pty.onExit(({ exitCode, signal }) => {
        exited = true; data.dispose();
        if (proof && exitCode === 0 && !signal) done.resolve();
        else fail(new Error('Terminal supervisor exited without successful settlement'));
        socket?.destroy(); server.close(); rmSync(directory, { recursive: true, force: true });
      });
    } catch (error) {
      // spawn failed: no PTY was admitted, hence no descendants exist.
      ready.reject(error instanceof Error ? error : new Error(String(error)));
      done.resolve(); server.close(); rmSync(directory, { recursive: true, force: true });
    }
  });
  return {
    ready: ready.promise, done: done.promise,
    stop() { if (!stopping) { stopping = true; send(0x10); } return done.promise; },
    write(data) { if (!pty || stopping || exited) throw new Error('Terminal is not executing'); pty.write(data); },
    resize(cols, rows) { if (pty && !stopping && !exited) pty.resize(cols, rows); },
  };
}
