// @vitest-environment node
import { networkInterfaces } from 'node:os';
import { expect, it } from 'vitest';
import { startHttpMcpPeer } from './e2e/http-mcp-startup-gate';

it('serves a real MCP catalog on a non-loopback local interface', async () => {
  const controller = new AbortController();
  const peer = await startHttpMcpPeer(controller.signal, () => ({
    tools: [{ name: 'render_image', description: 'fixture', inputSchema: { type: 'object' } }],
  }));
  try {
    const url = new URL(peer.url);
    expect(Object.values(networkInterfaces()).flatMap(addresses => addresses ?? [])
      .some(address => address.family === 'IPv4' && !address.internal && address.address === url.hostname)).toBe(true);
    const request = (message: unknown, session?: string) => fetch(peer.url, {
      method: 'POST',
      headers: { 'content-type': 'application/json', ...(session ? { 'mcp-session-id': session } : {}) },
      body: JSON.stringify(message),
    });
    const initialized = await request({ jsonrpc: '2.0', id: 1, method: 'initialize',
      params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'test', version: '1' } } });
    expect(initialized.status).toBe(200);
    const session = initialized.headers.get('mcp-session-id');
    expect(session).toBeTruthy();
    const response = await request({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }, session!);
    expect(response.status).toBe(200);
    expect((await response.json()).result.tools[0].name).toBe('render_image');
  } finally {
    await peer.close();
  }
});
