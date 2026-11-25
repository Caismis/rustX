// @vitest-environment node
import { createServer } from 'node:http';
import { mkdtempSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, it, vi } from 'vitest';
import type { WorkflowToolSource } from '../../protocol/app-server/v81';
import { BaseTransport } from '../../tui/src/app-server/transport.ts';
import { workspaceHandler } from '../host/http';
import { LocalWorkspaceHost } from '../host/workspaces';
import { createWorkflowPort } from '../src/app/workflows/port';
import { ToolCatalogProvider, useToolCatalog } from '../src/app/workflows/ui/editor/toolCatalog';
import type { AppServerClient } from '../src/client/app-server';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { HttpWorkspaceHost } from '../src/workspaces/http-host';
import { capabilities } from './fixture';

const { connect } = vi.hoisted(() => ({ connect: vi.fn() }));
vi.mock('../../tui/src/app-server/websocket-transport.ts', () => ({ WebSocketTransport: { connect } }));

function CatalogButton() {
  const catalog = useToolCatalog();
  return <button onClick={catalog.load}>Load tools</button>;
}

it('uses one Host request and native connection for eight sources and retires the batch when the editor closes', async () => {
  const root = mkdtempSync(join(tmpdir(), 'workflow-tool-transport-'));
  const endpoint = 'ws://127.0.0.1:8080/', authorityId = 'fixture-native';
  const host = new LocalWorkspaceHost({ endpoint, picker: true, transportToken: 'transport-token',
    metadataFile: join(root, 'workspaces.json'), roots: [{ id: 'location', cwd: root, displayName: 'Location' }] });
  const server = createServer(workspaceHandler(host));
  const sources: WorkflowToolSource[] = Array.from({ length: 8 }, (_, index) => ({
    source_id: `source-${index}`, kind: 'mcp', scope: 'workspace', state: 'pending', tools: [],
  }));
  const listings = new Map<string, () => void>();
  const requested: string[] = [];
  let nativeActive = 0, nativePeak = 0, httpActive = 0, httpPeak = 0;
  class CatalogTransport extends BaseTransport {
    describe() { return 'fixture native'; }
    protected disposeTransport() { --nativeActive; }
    protected async writeMessage(record: unknown) {
      const request = record as { id: string; method: string; params: { source?: string } };
      const reply = (result: unknown) => this.deliver({ jsonrpc: '2.0', id: request.id, result });
      if (request.method === 'initialize') {
        reply({ type: 'initialized', authority_id: authorityId, protocol_version: 81, capabilities });
        return;
      }
      expect(request.method).toBe('workflow/toolCatalog');
      const { source } = request.params;
      if (source === undefined) {
        reply({ type: 'workflow_tool_catalog', catalog: { sources } });
      } else {
        requested.push(source);
        listings.set(source, () => reply({ type: 'workflow_tool_catalog', catalog: {
          sources: [{ ...sources.find(item => item.source_id === source), state: 'listed' }],
        } }));
      }
    }
  }
  connect.mockImplementation(async () => {
    nativePeak = Math.max(nativePeak, ++nativeActive);
    return new CatalogTransport();
  });
  let dispose = () => {};
  try {
    await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('Missing Host address');
    // Load the real Product Host under Node and give only the editor a DOM.
    const { JSDOM } = createRequire(import.meta.url)('jsdom') as {
      JSDOM: new (html: string, options: { url: string }) => { window: Window };
    };
    const { window } = new JSDOM('', { url: `http://127.0.0.1:${address.port}/` });
    vi.stubGlobal('window', window);
    vi.stubGlobal('document', window.document);
    vi.stubGlobal('location', window.location);
    const { cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react');
    dispose = () => { cleanup(); window.close(); };
    const fetchHost = fetch;
    vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
      const pathname = new URL(url).pathname;
      const listing = pathname === '/product-host/workflow';
      if (listing) httpPeak = Math.max(httpPeak, ++httpActive);
      try { return await fetchHost(url, init); }
      finally { if (listing) --httpActive; }
    });
    const http = new HttpWorkspaceHost();
    const authority = await new WorkspaceAuthority(http).observe();
    const client = {
      getSnapshot: () => ({ endpoint, authorityId, authorityRevision: 1 }),
    } as unknown as AppServerClient;
    const port = createWorkflowPort({ client, host: () => http, hostAuthority: authority,
      scope: { kind: 'workspace', workspaceId: authority.catalog.workspaces[0].id } });
    const editor = render(<ToolCatalogProvider port={port}><CatalogButton /></ToolCatalogProvider>);
    fireEvent.click(screen.getByRole('button', { name: 'Load tools' }));
    await waitFor(() => expect(listings.has('source-0')).toBe(true));
    expect(requested).toEqual(['source-0']);
    await expect(http.listWorkspaces()).resolves.toMatchObject({ authorityId: authority.scope.authorityId });
    listings.get('source-0')!();
    await waitFor(() => expect(listings.has('source-1')).toBe(true));
    editor.unmount();
    await waitFor(() => expect([httpActive, nativeActive]).toEqual([0, 0]));
    expect([httpPeak, nativePeak]).toEqual([1, 1]);
    expect(requested).toEqual(['source-0', 'source-1']);
    expect(connect).toHaveBeenCalledTimes(3);
  } finally {
    dispose();
    vi.unstubAllGlobals();
    server.closeAllConnections();
    await new Promise<void>(resolve => server.close(() => resolve()));
    await host.close();
    rmSync(root, { recursive: true, force: true });
  }
});
