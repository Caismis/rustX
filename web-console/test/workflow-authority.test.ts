// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { AppServerTransport } from '../../tui/src/app-server/transport.ts';
import type { WorkflowDefinition, WorkflowSource } from '../../protocol/app-server/v51';
import { capabilities as nativeCapabilities } from './fixture';
import { exampleSource } from './workflow-fixtures';
import { LocalWorkspaceHost } from '../host/workspaces';
import type { AppServerClient } from '../src/client/app-server';
import { createWorkflowPort, WorkflowOperationError } from '../src/app/workflows/port';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { WorkspaceHostError, type ProductHostWorkspaces, type WorkflowHostOperation } from '../src/workspaces/host';

interface ObservedRequest { id: string | number; method: string; params: Record<string, unknown> }
const native = vi.hoisted(() => {
  const state = { requests: [] as ObservedRequest[], connects: 0 };
  class FakeTransport implements AppServerTransport {
    readonly closed = undefined;
    private listeners = new Set<(record: unknown) => void>();
    describe() { return 'fake'; }
    send(message: unknown) {
      const request = message as ObservedRequest;
      state.requests.push(request);
      const reply = (result: unknown) => queueMicrotask(() => this.deliver({ jsonrpc: '2.0', id: request.id, result }));
      if (request.method === 'initialize') reply({ type: 'initialized', authority_id: 'fixture-app-server-authority', protocol_version: 51, capabilities: nativeCapabilities });
      else if (request.method === 'configuration/sourceWrite') reply({ type: 'sources_written', revision: 'sha256:created', configuration_revision: null });
      else if (request.method === 'workflow/read') reply({ type: 'workflow_source', source: { id: 'flow', revision: 'sha256:created' } });
      else if (request.method === 'workflow/list') reply({ type: 'workflow_library', library: { target: request.params.target, directory: '/x', absent_revision: 'missing', files: [], diagnostic: null } });
      return Promise.resolve();
    }
    onMessage(listener: (record: unknown) => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
    onClose() { return () => {}; }
    close() {}
    deliver(record: unknown) { for (const listener of [...this.listeners]) listener(record); }
  }
  return { state, FakeTransport };
});
vi.mock('../../tui/src/app-server/websocket-transport.ts', () => ({
  WebSocketTransport: { connect: async () => { native.state.connects++; return new native.FakeTransport(); } },
}));

const directories: string[] = [];
afterEach(() => { directories.splice(0).forEach(path => rmSync(path, { force: true, recursive: true })); });
beforeEach(() => { native.state.requests.length = 0; native.state.connects = 0; });

const endpoint = 'ws://127.0.0.1:8080/';
const document = exampleSource('parallel_review').document as unknown as WorkflowDefinition;
const operations: WorkflowHostOperation[] = [
  { kind: 'list' },
  { kind: 'read', id: 'flow' },
  { kind: 'check', scope: 'workspace', id: 'flow', document, explain: false },
  { kind: 'check', scope: 'user', id: 'flow', document, explain: false },
  { kind: 'write', id: 'flow', expected_revision: 'missing', edit: { operation: 'create', description: 'Created' } },
];

describe('Workflow operations and Product Host authority (R1)', () => {
  /** H1 maps registration R to directory A. H2 replaces H1, keeps the
   * persisted registration R, and maps its location to directory B. */
  function replacement() {
    const root = mkdtempSync(join(tmpdir(), 'workflow-authority-')); directories.push(root);
    const a = join(root, 'A'), b = join(root, 'B'); mkdirSync(a); mkdirSync(b);
    const metadataFile = join(root, 'registrations.json');
    const base = { endpoint, picker: true, transportToken: 'transport-token', metadataFile };
    const h1 = new LocalWorkspaceHost({ ...base, roots: [{ id: 'location', cwd: a, displayName: 'Location' }] });
    const h2 = new LocalWorkspaceHost({ ...base, roots: [{ id: 'location', cwd: b, displayName: 'Location' }] });
    return { h1, h2, a, b };
  }

  it('refuses every operation of a replaced Host before any native dispatch, even under a reused registration ID', async () => {
    const { h1, h2, b } = replacement();
    const old = await h1.listWorkspaces(), current = await h2.listWorkspaces();
    const [registration] = old.workspaces;
    expect(current.workspaces.map(row => row.id)).toEqual([registration.id]);
    expect(current.workspaces[0].displayPath).toBe(b);
    for (const operation of operations) {
      const refused = h2.workflowWorkspace({ authorityId: old.authorityId, endpoint: old.endpoint }, registration.id, operation);
      await expect(refused).rejects.toMatchObject({ kind: 'authority_replaced' });
    }
    expect(native.state.connects).toBe(0);
    expect(native.state.requests).toHaveLength(0);

    // The replacement's own scope reaches exactly its own mapping.
    await h2.workflowWorkspace({ authorityId: current.authorityId, endpoint: current.endpoint }, registration.id, operations[4]);
    const [write] = native.state.requests.filter(request => request.method === 'configuration/sourceWrite');
    expect(write.params.target).toEqual({ kind: 'workspace', directory: b });
  });

  it('binds a port to the observed Host, so the old transaction is refused before reaching the replacement', async () => {
    const { h1, h2 } = replacement();
    let host: ProductHostWorkspaces = h1;
    const forwarding: ProductHostWorkspaces = {
      listWorkspaces: () => host.listWorkspaces(),
      classifyLocations: (...args) => host.classifyLocations(...args),
      workflowWorkspace: (...args) => host.workflowWorkspace!(...args),
    } as ProductHostWorkspaces;
    const authority = new WorkspaceAuthority(forwarding);
    const observed = await authority.observe();
    const [registration] = observed.catalog.workspaces;
    const client = fakeClient();
    const port = createWorkflowPort({ client: client.client, host: () => forwarding, hostAuthority: authority.capture(), scope: { kind: 'workspace', workspaceId: registration.id } });
    const global = createWorkflowPort({ client: client.client, host: () => forwarding, hostAuthority: authority.capture(), scope: { kind: 'user' } });

    // H2 replaces H1 behind the same Product Host address.
    host = h2;
    // Before the browser observes the replacement, the Host refuses it.
    await expect(port.write('flow', 'missing', { operation: 'create', description: 'x' })).rejects.toMatchObject({ failure: { kind: 'unavailable' } });
    await expect(global.check('flow', document, false, registration.id)).rejects.toMatchObject({ failure: { kind: 'unavailable' } });
    // After it does, the port refuses before calling the Host at all.
    await authority.observe();
    await expect(port.list()).rejects.toMatchObject({ failure: { kind: 'unavailable', message: 'product_host_replaced' } });
    expect(native.state.requests).toHaveLength(0);
  });
});

/** An App Server client whose authority and replies the test controls. */
function fakeClient() {
  const snapshot = { endpoint, authorityRevision: 1 };
  const pending: { method: string; resolve: (value: unknown) => void; reject: (reason: unknown) => void }[] = [];
  const client = {
    getSnapshot: () => snapshot,
    request: ({ method }: { method: string }) => new Promise((resolve, reject) => { pending.push({ method, resolve, reject }); }),
  } as unknown as AppServerClient;
  return { client, snapshot, pending };
}
const flush = async () => { for (let turn = 0; turn < 8; turn++) await Promise.resolve(); };

describe('A known write acknowledgement across a later authority change (R5)', () => {
  it('keeps the acknowledged commit when the server is replaced during its reread', async () => {
    const { client, snapshot, pending } = fakeClient();
    const port = createWorkflowPort({ client, host: () => undefined, hostAuthority: undefined, scope: { kind: 'user' } });
    const writing = port.write('flow', 'sha256:base', { operation: 'delete' });
    await flush();
    expect(pending.map(row => row.method)).toEqual(['configuration/sourceWrite']);
    pending[0].resolve({ type: 'sources_written', revision: 'sha256:acknowledged', configuration_revision: '9' });
    await flush();
    expect(pending.map(row => row.method)).toEqual(['configuration/sourceWrite', 'workflow/read']);
    // The authority changes while the reread is pending, and the reread fails.
    snapshot.authorityRevision = 2;
    pending[1].reject(new Error('connection replaced'));
    await expect(writing).resolves.toEqual({
      acknowledgement: { revision: 'sha256:acknowledged', configuration_revision: '9' },
      reread: { status: 'failed', error: 'connection replaced' },
    });
  });

  it('never attributes a replacement server\'s reread to the original commit', async () => {
    const { client, snapshot, pending } = fakeClient();
    const port = createWorkflowPort({ client, host: () => undefined, hostAuthority: undefined, scope: { kind: 'user' } });
    const writing = port.write('flow', 'sha256:base', { operation: 'delete' });
    await flush();
    pending[0].resolve({ type: 'sources_written', revision: 'sha256:acknowledged', configuration_revision: null });
    await flush();
    snapshot.authorityRevision = 2;
    pending[1].resolve({ type: 'workflow_source', source: { ...exampleSource('parallel_review'), revision: 'sha256:elsewhere' } as unknown as WorkflowSource });
    await expect(writing).resolves.toEqual({
      acknowledgement: { revision: 'sha256:acknowledged', configuration_revision: null },
      reread: { status: 'failed', error: 'server_replaced' },
    });
    // The port stays bound to its original server: it neither rebinds nor
    // reads from the replacement.
    await expect(port.read('flow')).rejects.toBeInstanceOf(WorkflowOperationError);
    expect(pending).toHaveLength(2);
  });

  it('keeps an acknowledged Workspace commit when the server is replaced while the Host rereads', async () => {
    const { client, snapshot } = fakeClient();
    let release!: () => void;
    const host = {
      workflowWorkspace: () => new Promise(resolve => { release = () => resolve({ kind: 'write', commit: { acknowledgement: { revision: 'sha256:w', configuration_revision: null }, reread: { status: 'observed', source: exampleSource('parallel_review') } } }); }),
    } as unknown as ProductHostWorkspaces;
    const hostAuthority = { scope: { authorityId: 'h', endpoint }, catalog: undefined as never, current: () => true };
    const port = createWorkflowPort({ client, host: () => host, hostAuthority, scope: { kind: 'workspace', workspaceId: 'r' } });
    const writing = port.write('flow', 'missing', { operation: 'create', description: 'x' });
    await flush();
    snapshot.authorityRevision = 2;
    release();
    await expect(writing).resolves.toMatchObject({ acknowledgement: { revision: 'sha256:w' }, reread: { status: 'failed', error: 'server_replaced' } });
  });

  it('reports a write refused by a replaced Host as unavailable, never as uncertain', async () => {
    const { client } = fakeClient();
    const host = { workflowWorkspace: () => Promise.reject(new WorkspaceHostError('Workspace Host authority replaced', 'authority_replaced')) } as unknown as ProductHostWorkspaces;
    const port = createWorkflowPort({ client, host: () => host, hostAuthority: { scope: { authorityId: 'h', endpoint }, catalog: undefined as never, current: () => true }, scope: { kind: 'workspace', workspaceId: 'r' } });
    await expect(port.write('flow', 'missing', { operation: 'create', description: 'x' })).rejects.toMatchObject({ failure: { kind: 'unavailable' } });
  });
});

describe('Library activation after connection bootstrap (R9)', () => {
  it('lists Global on first activation when the surface was created before the connection completed', async () => {
    const { client, snapshot, pending } = fakeClient();
    // App startup: no server authority observed yet, and no Workspace.
    snapshot.endpoint = '' as typeof snapshot.endpoint;
    snapshot.authorityRevision = 0;
    const authority = new WorkspaceAuthority({ listWorkspaces: async () => ({ authorityId: 'h', endpoint, workspaces: [], picker: { kind: 'unavailable', reason: 'none' } }) } as unknown as ProductHostWorkspaces);
    const { createActor } = await import('xstate');
    const { workflowSurfaceMachine } = await import('../src/app/workflows/machines/surface');
    const surface = createActor(workflowSurfaceMachine, { input: { ports: scope => createWorkflowPort({ client, host: () => undefined, hostAuthority: authority.capture(), scope }) } }).start();
    // The connection completes.
    snapshot.endpoint = endpoint;
    snapshot.authorityRevision = 1;
    await authority.observe();
    const library = surface.getSnapshot().context.library;
    library.send({ type: 'ACTIVATE' });
    await flush();
    expect(pending.map(row => row.method)).toEqual(['workflow/list']);
    pending[0].resolve({ type: 'workflow_library', library: { target: { kind: 'user' }, directory: '/home/rustx/.agents/workflows', absent_revision: 'missing', files: [], diagnostic: null } });
    await flush();
    expect(library.getSnapshot().matches('ready')).toBe(true);
    expect(library.getSnapshot().context.listFailure).toBeUndefined();
  });

  it('keeps an open confirmation bound to its port when the library refreshes', async () => {
    const { client, pending } = fakeClient();
    const { createActor } = await import('xstate');
    const { workflowLibraryMachine } = await import('../src/app/workflows/machines/library');
    const made: ReturnType<typeof createWorkflowPort>[] = [];
    const library = createActor(workflowLibraryMachine, { input: { scope: { kind: 'user' }, ports: scope => { const port = createWorkflowPort({ client, host: () => undefined, hostAuthority: undefined, scope }); made.push(port); return port; } } }).start();
    library.send({ type: 'ACTIVATE' });
    await flush();
    pending[0].resolve({ type: 'workflow_library', library: { target: { kind: 'user' }, directory: '/d', absent_revision: 'missing', files: [], diagnostic: null } });
    await flush();
    const bound = library.getSnapshot().context.port!;
    // A refresh creates a newer read port; the confirmation keeps its own.
    library.send({ type: 'REFRESH' });
    expect(library.getSnapshot().context.port).not.toBe(bound);
    pending[1].resolve({ type: 'workflow_library', library: { target: { kind: 'user' }, directory: '/d', absent_revision: 'missing', files: [], diagnostic: null } });
    await flush();
    library.send({ type: 'OPERATE', operation: { kind: 'create', id: 'fresh', description: 'x' }, port: bound, absent: 'missing' });
    expect(library.getSnapshot().context.pendingTarget?.port).toBe(bound);
    // A refresh while the operation is in flight neither stops nor retargets it.
    library.send({ type: 'REFRESH' });
    expect(library.getSnapshot().matches('operating')).toBe(true);
    expect(made).toHaveLength(2);
  });
});
