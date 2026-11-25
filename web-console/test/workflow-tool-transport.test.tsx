// @vitest-environment node
import { createServer } from 'node:http';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, it, vi } from 'vitest';
import type { Request1, WorkflowToolSource } from '../../protocol/app-server/v81';
import { BaseTransport } from '../../tui/src/app-server/transport.ts';
import { workspaceHandler } from '../host/http';
import { LocalWorkspaceHost } from '../host/workspaces';
import { createWorkflowPort } from '../src/app/workflows/port';
import { ToolCatalogProvider, useToolCatalog } from '../src/app/workflows/ui/editor/toolCatalog';
import type { AppServerClient } from '../src/client/app-server';
import { WorkspaceAssociations } from '../src/workspaces/associations';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { HttpWorkspaceHost } from '../src/workspaces/http-host';
import { cfg3Client, cfg3Host } from './cfg3-fixture';
import { capabilities } from './fixture';

const { connect } = vi.hoisted(() => ({ connect: vi.fn() }));
vi.mock('../../tui/src/app-server/websocket-transport.ts', () => ({ WebSocketTransport: { connect } }));

function CatalogButton() {
  const catalog = useToolCatalog();
  return <button onClick={catalog.load}>Load tools</button>;
}

type CatalogParams = Extract<Request1, { method: 'workflow/toolCatalog' }>['params'];
interface ToolTransport {
  root: string;
  endpoint: string;
  authorityId: string;
  http: HttpWorkspaceHost;
  rtl: typeof import('@testing-library/react');
  listings: Map<string, () => void>;
  requests: CatalogParams[];
  active: () => number[];
  peak: () => number[];
}

async function withToolTransport(sources: WorkflowToolSource[], test: (fixture: ToolTransport) => Promise<void>) {
  connect.mockClear();
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'workflow-tool-transport-')));
  const endpoint = 'ws://127.0.0.1:8080/', authorityId = 'fixture-native';
  const host = new LocalWorkspaceHost({ endpoint, picker: true, transportToken: 'transport-token',
    metadataFile: join(root, 'workspaces.json'), roots: [{ id: 'location', cwd: root, displayName: 'Location' }] });
  const server = createServer(workspaceHandler(host));
  const listings = new Map<string, () => void>();
  const requests: CatalogParams[] = [];
  let nativeActive = 0, nativePeak = 0, httpActive = 0, httpPeak = 0;
  class CatalogTransport extends BaseTransport {
    describe() { return 'fixture native'; }
    protected disposeTransport() { --nativeActive; }
    protected async writeMessage(record: unknown) {
      const request = record as { id: string; method: string; params: CatalogParams };
      const reply = (result: unknown) => this.deliver({ jsonrpc: '2.0', id: request.id, result });
      if (request.method === 'initialize') {
        reply({ type: 'initialized', authority_id: authorityId, protocol_version: 81, capabilities });
        return;
      }
      expect(request.method).toBe('workflow/toolCatalog');
      const { source } = request.params;
      if (source == null) {
        reply({ type: 'workflow_tool_catalog', catalog: { sources } });
      } else {
        requests.push(request.params);
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
      JSDOM: new (html: string, options: { url: string }) => { window: Window & typeof globalThis };
    };
    const { window } = new JSDOM('<!doctype html>', { url: `http://127.0.0.1:${address.port}/` });
    vi.stubGlobal('window', window);
    vi.stubGlobal('document', window.document);
    vi.stubGlobal('location', window.location);
    for (const key of ['Node', 'NodeFilter', 'Element', 'HTMLElement', 'SVGElement', 'MutationObserver', 'getComputedStyle'] as const) {
      vi.stubGlobal(key, window[key]);
    }
    const rtl = await import('@testing-library/react');
    dispose = () => { rtl.cleanup(); window.close(); };
    const fetchHost = fetch;
    vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
      const pathname = new URL(url).pathname;
      const listing = pathname === '/product-host/workflow';
      if (listing) httpPeak = Math.max(httpPeak, ++httpActive);
      try { return await fetchHost(url, init); }
      finally { if (listing) --httpActive; }
    });
    await test({ root, endpoint, authorityId, http: new HttpWorkspaceHost(), rtl, listings, requests,
      active: () => [httpActive, nativeActive], peak: () => [httpPeak, nativePeak] });
  } finally {
    dispose();
    vi.unstubAllGlobals();
    server.closeAllConnections();
    await new Promise<void>(resolve => server.close(() => resolve()));
    await host.close();
    rmSync(root, { recursive: true, force: true });
  }
}

function toolSources(scopeAt: (index: number) => WorkflowToolSource['scope']): WorkflowToolSource[] {
  return Array.from({ length: 8 }, (_, index) => ({
    source_id: `source-${index}`, kind: 'mcp', scope: scopeAt(index), state: 'pending', tools: [],
  }));
}

it('uses one Host request and native connection for eight sources and retires the batch when the editor closes', async () => {
  await withToolTransport(toolSources(() => 'workspace'), async ({ http, endpoint, authorityId, rtl, listings, requests, active, peak }) => {
    const { fireEvent, render, waitFor } = rtl;
    const authority = await new WorkspaceAuthority(http).observe();
    const client = {
      getSnapshot: () => ({ endpoint, authorityId, authorityRevision: 1 }),
    } as unknown as AppServerClient;
    const port = createWorkflowPort({ client, host: () => http, hostAuthority: authority,
      scope: { kind: 'workspace', workspaceId: authority.catalog.workspaces[0].id } });
    const editor = render(<ToolCatalogProvider port={port}><CatalogButton /></ToolCatalogProvider>);
    fireEvent.click(editor.getByRole('button', { name: 'Load tools' }));
    await waitFor(() => expect(listings.has('source-0')).toBe(true));
    expect(requests.map(request => request.source)).toEqual(['source-0']);
    await expect(http.listWorkspaces()).resolves.toMatchObject({ authorityId: authority.scope.authorityId });
    listings.get('source-0')!();
    await waitFor(() => expect(listings.has('source-1')).toBe(true));
    editor.unmount();
    await waitFor(() => expect(active()).toEqual([0, 0]));
    expect(peak()).toEqual([1, 1]);
    expect(requests.map(request => request.source)).toEqual(['source-0', 'source-1']);
    expect(connect).toHaveBeenCalledTimes(3);
  });
});

it('serializes Workspace and inherited MCP tool reads and cancels them when the Subagent editor closes', async () => {
  const sources = toolSources(index => index === 0 ? 'workspace' : 'user');
  await withToolTransport(sources, async ({ root, endpoint, authorityId, http, rtl, listings, requests, active, peak }) => {
    const { ResourceSurface } = await import('../src/app/resources/ResourceSurface');
    const { fireEvent, render, waitFor, within } = rtl;
    const subject = cfg3Client();
    subject.publish({ endpoint, authorityId, authorityRevision: 1 });
    subject.source.prospective_resources = {
      ...subject.effective.resources,
      definitions: sources.map(source => ({ family: 'mcp', name: source.source_id, valid: true,
        location: { scope: source.scope, path: join(root, source.source_id) } })),
    };
    const authority = new WorkspaceAuthority(http);
    const { catalog } = await authority.observe();
    const host = {
      ...cfg3Host(subject),
      listWorkspaces: http.listWorkspaces,
      workflowWorkspace: http.workflowWorkspace,
      configureWorkspace: async () => ({ kind: 'read' as const, projection: {
        ...subject.source, target: { kind: 'workspace' as const, directory: root },
      } }),
    };
    const resources = render(<ResourceSurface module="subagents" visible client={subject.client} host={host} authority={authority}
      associations={new WorkspaceAssociations(subject.client, authority)} initialWorkspace={catalog.workspaces[0].id} />);
    fireEvent.click(await resources.findByRole('button', { name: 'New subagent' }));
    const form = within(await resources.findByRole('form', { name: 'New subagent' }));
    expect(form.getByText('source-7')).toBeDefined();
    await waitFor(() => expect(listings.has('source-0')).toBe(true));
    expect(active()).toEqual([1, 1]);
    await expect(http.listWorkspaces()).resolves.toMatchObject({ authorityId: catalog.authorityId });
    listings.get('source-0')!();
    await form.findByText('This server returned no tools.');
    await waitFor(() => expect(listings.has('source-1')).toBe(true));
    fireEvent.click(form.getByRole('button', { name: 'Cancel' }));
    expect(resources.queryByRole('form', { name: 'New subagent' })).toBeNull();
    await waitFor(() => expect(active()).toEqual([0, 0]));
    expect(peak()).toEqual([1, 1]);
    expect(requests.map(({ source, expected_scope, target, execution_session }) =>
      [source, expected_scope, target, execution_session ?? null])).toEqual([
      ['source-0', 'workspace', { kind: 'workspace', directory: root }, null],
      ['source-1', 'user', { kind: 'workspace', directory: root }, null],
    ]);
    expect(connect).toHaveBeenCalledTimes(2);
  });
});
