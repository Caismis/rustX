// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { MethodResult, SessionSummary } from '../../protocol/app-server/v81';
import { ResourceSurface } from '../src/app/resources/ResourceSurface';
import { WorkspaceAssociations } from '../src/workspaces/associations';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { cfg3Client, cfg3Host } from './cfg3-fixture';

afterEach(cleanup);

const cwd = '/workspace/A';
const session = (id: string): SessionSummary => ({
  id, cwd, active_node: id, ownership_generation: '1', updated_at: '2026-01-01T00:00:00Z',
});
const first = session('first');
const second = session('second');

function mountResources(subject: ReturnType<typeof cfg3Client>, missing = false) {
  const host = { ...cfg3Host(subject), classifyLocations: async (paths: readonly string[]) =>
    paths.map(path => missing ? { authorized: false as const, reason: 'unavailable' as const,
      clientCatalogWorkspaceId: path === cwd ? 'A' : undefined }
      : { authorized: true as const, workspaceId: path === cwd ? 'A' : undefined }) };
  const authority = new WorkspaceAuthority(host);
  const associations = new WorkspaceAssociations(subject.client, authority);
  subject.publish({sessions: [first, second]});
  return { associations, surface: (module: 'skills' | 'mcp', active: SessionSummary) =>
    <ResourceSurface module={module} visible client={subject.client} host={host} authority={authority}
      associations={associations} initialWorkspace="A" executionSession={active} /> };
}

const mcpDocument = (id: string, revision: string): MethodResult => ({
  type: 'mcp_source', source: { path: cwd + '/.agents/mcp.toml', revision, authored: {
    [id]: { definition: { type: 'stdio', command: id }, retained_env: [], retained_headers: [] },
  } },
});

it('opens Client-only Skills without a Server Workspace source observation', async () => {
  const subject = cfg3Client(async op => {
    if (op.method === 'configuration/sourcesRead' && op.params.target.kind === 'workspace') throw new Error('Server path absent');
    if (op.method !== 'skills/list' || op.params.target.kind !== 'workspace') return;
    expect(op.params).toMatchObject({ execution_session: first.id, target: { kind: 'workspace', directory: cwd } });
    const path = cwd + '/.agents/skills/client-guide/SKILL.md';
    return { type: 'skills', page: { revision: 'client-skills', diagnostics: [], diagnostics_truncated: false, data: [{
      catalog: { name: 'client-guide', description: 'A Client-only skill', location: path },
      provenance: { name: 'client-guide', source: 'workspace', location: path, shadowed: [] },
      operation_revision: 'client-skill-write', metadata_revision: 'client-skill', disable_model_invocation: false, enabled_by_default: true,
    }] } };
  });
  const { associations, surface } = mountResources(subject, true);
  const stop = associations.start();
  try {
    render(surface('skills', first));
    const skill = await screen.findByRole('listitem', { name: 'client-guide' });
    expect(within(skill).getByText('A Client-only skill')).toBeTruthy();
    expect(within(skill).queryByRole('button', { name: /Actions/ })).toBeNull();
    expect(subject.request.mock.calls.some(([op]) =>
      op.method === 'configuration/sourcesRead' && op.params.target.kind === 'workspace')).toBe(false);
  } finally { stop(); }
});

it('uses the selected Client MCP revision and ignores a late document from the former Session', async () => {
  let resolveRead!: (result: MethodResult) => void;
  const previous = new Promise<MethodResult>(resolve => { resolveRead = resolve; });
  const subject = cfg3Client(async op => {
    if (op.method === 'mcp/sourceRead') {
      expect(op.params.target).toEqual({ kind: 'workspace', directory: cwd });
      return op.params.execution_session === first.id ? previous : mcpDocument('client-server', 'client-revision');
    }
  });
  subject.source.workspace_mcp = {
    path: cwd + '/.agents/mcp.toml', revision: 'different-server-revision',
    authored: { decoy: { definition: { type: 'stdio', command: 'server-only' }, retained_env: [], retained_headers: [] } },
  };
  const { associations, surface } = mountResources(subject);
  const stop = associations.start();
  try {
    const ui = render(surface('mcp', first));
    await waitFor(() => expect(subject.request.mock.calls.some(([op]) =>
      op.method === 'mcp/sourceRead' && op.params.execution_session === first.id)).toBe(true));
    ui.rerender(surface('mcp', second));
    const row = await screen.findByRole('listitem', { name: 'client-server' });
    fireEvent.click(within(row).getByRole('button', { name: 'Test connection client-server' }));
    await waitFor(() => {
      const probes = subject.request.mock.calls.flatMap(([op]) => op.method === 'mcp/probe' ? [op.params] : []);
      expect(probes).toEqual([{
        id: 'client-server', expected_revision: 'client-revision', execution_session: second.id,
        target: { kind: 'workspace', directory: cwd },
      }]);
    });
    await act(async () => resolveRead(mcpDocument('stale-server', 'old-client-revision')));
    expect(screen.getByRole('listitem', { name: 'client-server' })).toBeTruthy();
    expect(screen.queryByRole('listitem', { name: 'stale-server' })).toBeNull();
    expect(screen.queryByRole('listitem', { name: 'decoy' })).toBeNull();
  } finally { stop(); }
});
