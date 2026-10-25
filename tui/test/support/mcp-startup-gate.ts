import { createServer, type Socket } from 'node:net';
import { once } from 'node:events';
import { accessSync, constants, mkdirSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** Native stdio MCP launches its process through this sibling executable. */
export function requireMcpStartupSupervisor(binary: string): void {
  const supervisor = join(dirname(realpathSync(binary)), `interactive-supervisor${process.platform === 'win32' ? '.exe' : ''}`);
  try {
    accessSync(supervisor, constants.X_OK);
  } catch (cause) {
    throw new Error(`Missing executable ${supervisor}; run \`cargo build --bins\` before the TUI MCP integration test`, { cause });
  }
}

/** Let Node's test watchdog unwind the caller and enter its owned cleanup. */
export async function startupWait<T>(operation: () => Promise<T>, signal: AbortSignal): Promise<T> {
  signal.throwIfAborted();
  let cancel!: () => void;
  const cancelled = new Promise<never>((_, reject) => {
    cancel = () => reject(signal.reason);
    signal.addEventListener('abort', cancel, { once: true });
  });
  try {
    const result = await Promise.race([operation(), cancelled]);
    signal.throwIfAborted();
    return result;
  } finally {
    signal.removeEventListener('abort', cancel);
  }
}

/** The gate is reached only by the real peer's tools/list request. */
export async function mcpStartupGate(workspace: string, signal: AbortSignal, toolName = 'startup_echo') {
  signal.throwIfAborted();
  let peer: Socket | undefined, calls = 0, initializations = 0, probes = 0, completedLists = 0;
  const processes: { pid: number; supervisorPid: number; pgid: number }[] = [];
  const events: { kind: string; at: number }[] = [];
  const peers = new Set<Socket>();
  let entered!: () => void, failed!: (reason: unknown) => void;
  const reached = new Promise<void>((resolve, reject) => { entered = resolve; failed = reject; });
  // Closing an unused fixture still settles this owned receipt.
  void reached.catch(() => {});
  const server = createServer(socket => {
    peer = socket;
    peers.add(socket);
    socket.once('error', failed);
    socket.once('close', () => {
      peers.delete(socket);
      failed(new Error('MCP startup peer closed before discovery completed'));
    });
    let buffered = '';
    socket.on('data', bytes => {
      buffered += bytes.toString();
      while (buffered.includes('\n')) {
        const end = buffered.indexOf('\n'), line = buffered.slice(0, end); buffered = buffered.slice(end + 1);
        const colon = line.indexOf(':'); const kind = colon < 0 ? line : line.slice(0, colon); events.push({ kind, at: performance.now() });
        if (kind === 'P') { const parts = line.split(':'); const pid = Number(parts[1]), supervisorPid = Number(parts[2]), pgid = Number(parts[3]); if (parts.length !== 4 || ![pid, supervisorPid, pgid].every(value => Number.isInteger(value) && value > 0)) { failed(new Error('Invalid MCP process receipt')); socket.destroy(); return; } processes.push({ pid, supervisorPid, pgid }); }
        else if (kind === 'D') probes++;
        else if (kind === 'I') initializations++;
        else if (kind === 'T') completedLists++;
        else if (kind === 'L') { calls++; peer = socket; entered(); }
        else { failed(new Error('Unexpected MCP startup receipt')); socket.destroy(); }
      }
    });
  });
  server.on('error', failed);
  let closing: Promise<void> | undefined;
  function close(reason: unknown = new Error('MCP startup gate closed before discovery')): Promise<void> {
    return closing ??= (async () => {
      signal.removeEventListener('abort', aborted);
      failed(reason);
      for (const socket of peers) socket.destroy();
      await new Promise<void>((resolve, reject) => {
        server.close(error => {
          if (error && (error as NodeJS.ErrnoException).code !== 'ERR_SERVER_NOT_RUNNING') reject(error);
          else resolve();
        });
      });
    })();
  }
  const aborted = () => { void close(signal.reason); };
  signal.addEventListener('abort', aborted, { once: true });
  try {
    server.listen({ port: 0, host: '127.0.0.1', signal });
    await startupWait(() => once(server, 'listening'), signal);
    const address = server.address();
    if (typeof address !== 'object' || address === null) throw new Error('Missing MCP startup address');
    mkdirSync(join(workspace, '.agents'), { recursive: true });
    const script = fileURLToPath(new URL('./gated-mcp.py', import.meta.url));
    writeFileSync(join(workspace, '.agents/mcp.toml'), `[mcp_servers.startup]\ntype = "stdio"\ncommand = "python3"\nargs = [${JSON.stringify(script)}, "${address.port}", ${JSON.stringify(toolName)}]\n`);
    writeFileSync(join(workspace, 'rustx.toml'), '[agent.tools]\nbuiltin = []\n[agent.tools.sources]\nstartup = ["startup_echo"]\n');
  } catch (error) {
    await close(error);
    throw error;
  }
  return {
    reached,
    get calls() { return calls; },
    get counts() { return { processes: processes.length, initializations, probes, lists: calls, completedLists }; },
    processes, events,
    async waitForDiscovery(attachment: Promise<unknown>): Promise<void> {
      await startupWait(() => Promise.race([
        reached,
        attachment.then(() => { throw new Error('Attachment completed before the held MCP discovery'); }),
      ]), signal);
    },
    release() {
      if (!peer || peer.destroyed) throw new Error('MCP discovery has not entered');
      peer.write('R');
    },
    close,
  };
}

export const startupToolSchema = { type: 'object', properties: { message: { type: 'string' } }, required: ['message'] };
