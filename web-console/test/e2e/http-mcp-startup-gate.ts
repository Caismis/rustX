import { randomUUID } from 'node:crypto';
import { once } from 'node:events';
import { mkdirSync, writeFileSync } from 'node:fs';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { networkInterfaces } from 'node:os';
import { join } from 'node:path';
import { startupWait } from '../../../tui/test/support/mcp-startup-gate';

interface Session {
  id: string;
  deleted: boolean;
  held: Set<ServerResponse>;
  closed: Promise<void>;
  complete: () => void;
}

/** A local interface the Workspace MCP dial policy permits. */
function fixtureAddress(): string {
  for (const addresses of Object.values(networkInterfaces())) {
    for (const candidate of addresses ?? []) {
      if (candidate.family !== 'IPv4' || candidate.internal) continue;
      const [first, second, third, fourth] = candidate.address.split('.').map(Number);
      const privateAddress = first === 10
        || (first === 172 && second! >= 16 && second! <= 31)
        || (first === 192 && second === 168)
        || (first === 100 && second! >= 64 && second! <= 127
          && !(second === 100 && third === 100 && fourth === 200));
      if (privateAddress) return candidate.address;
    }
  }
  throw new Error('The Workspace MCP fixture requires a non-loopback private IPv4 interface.');
}

export async function startHttpMcpPeer(
  signal: AbortSignal,
  catalog?: (cursor: string | undefined) => unknown,
  toolName = 'startup_echo',
) {
  signal.throwIfAborted();
  let calls = 0, probes = 0, completedLists = 0;
  const sessions = new Map<string, Session>();
  const events: { kind: string; at: number }[] = [];
  let current: (() => void) | undefined;
  let entered!: () => void, failed!: (error: unknown) => void;
  const reached = new Promise<void>((resolve, reject) => { entered = resolve; failed = reject; });
  void reached.catch(() => {});
  function observed(kind: string) { events.push({ kind, at: performance.now() }); }

  const server = createServer((request, response) => {
    void handle(request, response).catch(error => {
      failed(error);
      response.destroy();
    });
  });
  async function handle(request: IncomingMessage, response: ServerResponse) {
    if (request.url !== '/mcp') { response.writeHead(404).end(); return; }
    if (request.method === 'GET') { response.writeHead(405).end(); return; }
    const id = request.headers['mcp-session-id'];
    const session = typeof id === 'string' ? sessions.get(id) : undefined;
    if (request.method === 'DELETE') {
      if (!session || session.deleted) { response.writeHead(404).end(); return; }
      // The cancelled tools/list and DELETE can travel on different sockets.
      // Only acknowledge the fixture's complete cut after observing both.
      session.deleted = true;
      observed('X');
      response.writeHead(204).end();
      if (session.held.size === 0) session.complete();
      return;
    }
    if (request.method !== 'POST') { response.writeHead(405).end(); return; }
    const chunks: Buffer[] = [];
    let size = 0;
    for await (const chunk of request) {
      size += chunk.length;
      if (size > 1024 * 1024) throw new Error('MCP fixture request exceeds its bound');
      chunks.push(chunk);
    }
    const message = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    const reply = (result: unknown, newId?: string) => {
      const headers: Record<string, string> = { 'Content-Type': 'application/json' };
      if (newId !== undefined) headers['Mcp-Session-Id'] = newId;
      response.writeHead(200, headers).end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result }));
    };
    if (message.method === 'server/discover' && !session) {
      probes++;
      observed('D');
      response.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify({
        jsonrpc: '2.0', id: message.id, error: { code: -32601, message: 'Legacy fixture' },
      }));
      return;
    }
    if (message.method === 'initialize' && !session) {
      const newId = randomUUID();
      let complete!: () => void;
      const closed = new Promise<void>(resolve => { complete = resolve; });
      sessions.set(newId, { id: newId, deleted: false, held: new Set(), closed, complete });
      observed('I');
      reply({ protocolVersion: message.params.protocolVersion, capabilities: { tools: {} },
        serverInfo: { name: 'gated-startup', version: '1' } }, newId);
      return;
    }
    if (!session || session.deleted) { response.writeHead(404).end(); return; }
    if (message.id === undefined) { response.writeHead(202).end(); return; }
    if (message.method === 'tools/list') {
      if (catalog) {
        reply(catalog(message.params?.cursor));
        return;
      }
      calls++;
      observed('L');
      session.held.add(response);
      response.once('close', () => {
        session.held.delete(response);
        if (!response.writableEnded) observed('A');
        if (session.deleted && session.held.size === 0) session.complete();
      });
      current = () => {
        if (response.destroyed || response.writableEnded) throw new Error('MCP discovery POST is closed');
        reply({ tools: [{ name: toolName, description: 'Echo from the prepared source',
          inputSchema: { type: 'object', properties: { message: { type: 'string' } }, required: ['message'] } }] });
        completedLists++;
        observed('T');
      };
      entered();
    } else if (message.method === 'tools/call') {
      reply({ content: [{ type: 'text', text: 'prepared echo' }] });
    } else if (message.method === 'ping') {
      reply({});
    } else {
      response.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify({
        jsonrpc: '2.0', id: message.id, error: { code: -32601, message: 'Unknown method' },
      }));
    }
  }

  let closing: Promise<void> | undefined;
  const aborted = () => { void close(signal.reason); };
  function close(reason: unknown = new Error('HTTP MCP gate closed')): Promise<void> {
    return closing ??= new Promise<void>((resolve, reject) => {
      signal.removeEventListener('abort', aborted);
      failed(reason);
      server.close(error => {
        if (error && (error as NodeJS.ErrnoException).code !== 'ERR_SERVER_NOT_RUNNING') reject(error);
        else resolve();
      });
      server.closeAllConnections();
    });
  }
  signal.addEventListener('abort', aborted, { once: true });
  server.on('error', failed);
  let url: string;
  try {
    const host = fixtureAddress();
    server.listen({ port: 0, host, signal });
    await startupWait(() => once(server, 'listening'), signal);
    const address = server.address();
    if (typeof address !== 'object' || address === null) throw new Error('Missing HTTP MCP fixture address');
    url = `http://${host}:${address.port}/mcp`;
  } catch (error) {
    await close(error);
    throw error;
  }
  return {
    url,
    reached, events, close,
    get calls() { return calls; },
    get counts() { return { sessions: sessions.size, initializations: sessions.size, probes, lists: calls, completedLists }; },
    get sessions() { return [...sessions.values()].map(session => ({ id: session.id, deleted: session.deleted })); },
    async waitForClose(index: number) {
      const session = [...sessions.values()][index];
      if (!session) throw new Error('MCP session has not initialized');
      try {
        await startupWait(() => session.closed, AbortSignal.any([signal, AbortSignal.timeout(15_000)]));
      } catch (error) {
        throw new Error(`HTTP MCP session did not settle: ${events.map(event => event.kind).join(', ')}`, { cause: error });
      }
    },
    release() {
      const release = current;
      if (!release) throw new Error('MCP discovery has not entered');
      current = undefined;
      release();
    },
  };
}

/** Workspace drafts have no Session Client. Their live peer is Server-side HTTP. */
export async function mcpHttpStartupGate(workspace: string, signal: AbortSignal, toolName = 'startup_echo') {
  const peer = await startHttpMcpPeer(signal, undefined, toolName);
  try {
    mkdirSync(join(workspace, '.agents'), { recursive: true });
    writeFileSync(join(workspace, '.agents/mcp.toml'), `[mcp_servers.startup]\nurl = "${peer.url}"\n`);
    writeFileSync(join(workspace, 'rustx.toml'), `[agent.tools]\nbuiltin = []\n[agent.tools.sources]\nstartup = [${JSON.stringify(toolName)}]\n`);
    return peer;
  } catch (error) {
    await peer.close(error);
    throw error;
  }
}
