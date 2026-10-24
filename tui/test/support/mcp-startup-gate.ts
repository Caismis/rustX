import { createServer, type Socket } from 'node:net';
import { once } from 'node:events';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** The gate is reached only by the real peer's tools/list request. */
export async function mcpStartupGate(workspace: string) {
  let peer: Socket | undefined, calls = 0;
  const peers = new Set<Socket>();
  let entered!: () => void;
  const reached = new Promise<void>(resolve => { entered = resolve; });
  const server = createServer(socket => {
    peer = socket;
    peers.add(socket);
    socket.once('close', () => peers.delete(socket));
    socket.once('data', bytes => {
      if (bytes.toString() !== 'L') throw new Error('Unexpected MCP startup receipt');
      calls++;
      entered();
    });
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const address = server.address();
  if (typeof address !== 'object' || address === null) throw new Error('Missing MCP startup address');
  mkdirSync(join(workspace, '.agents'), { recursive: true });
  const script = fileURLToPath(new URL('./gated-mcp.py', import.meta.url));
  writeFileSync(join(workspace, '.agents/mcp.toml'), `[mcp_servers.startup]\ntype = "stdio"\ncommand = "python3"\nargs = [${JSON.stringify(script)}, "${address.port}"]\n`);
  writeFileSync(join(workspace, 'rustx.toml'), '[agent.tools]\nbuiltin = []\n[agent.tools.sources]\nstartup = ["startup_echo"]\n');
  return {
    reached,
    get calls() { return calls; },
    release() { if (!peer) throw new Error('MCP discovery has not entered'); peer.write('R'); },
    async close() { for (const socket of peers) socket.destroy(); const closed = once(server, 'close'); server.close(); await closed; },
  };
}

export const startupToolSchema = { type: 'object', properties: { message: { type: 'string' } }, required: ['message'] };
