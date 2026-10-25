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
export async function mcpStartupGate(workspace: string, signal: AbortSignal) {
  signal.throwIfAborted();
  let peer: Socket | undefined, calls = 0;
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
    socket.once('data', bytes => {
      if (bytes.toString() !== 'L') {
        failed(new Error('Unexpected MCP startup receipt'));
        socket.destroy();
        return;
      }
      calls++;
      entered();
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
    writeFileSync(join(workspace, '.agents/mcp.toml'), `[mcp_servers.startup]\ntype = "stdio"\ncommand = "python3"\nargs = [${JSON.stringify(script)}, "${address.port}"]\n`);
    writeFileSync(join(workspace, 'rustx.toml'), '[agent.tools]\nbuiltin = []\n[agent.tools.sources]\nstartup = ["startup_echo"]\n');
  } catch (error) {
    await close(error);
    throw error;
  }
  return {
    reached,
    get calls() { return calls; },
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
