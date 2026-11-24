// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { MethodResult, SessionSummary } from '../../protocol/app-server/v81';
import { configurationSystem } from '../src/app/settings/machines/system';
import { workspaceSettingsTarget } from '../src/app/settings/projection';
import { ResourceSurface } from '../src/app/resources/ResourceSurface';
import { bindResourceTransfer } from '../src/workspaces/resource-transfer';
import { WorkspaceAssociations } from '../src/workspaces/associations';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { WorkspaceHostError } from '../src/workspaces/host';
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
  return { associations, authority, host, surface: (module: 'skills' | 'mcp', active: SessionSummary) =>
    <ResourceSurface module={module} visible client={subject.client} host={host} authority={authority}
      associations={associations} initialWorkspace="A" executionSession={active} /> };
}

const mcpDocument = (id: string, revision: string): MethodResult => ({
  type: 'mcp_source', authority: 'client', source: { path: cwd + '/.agents/mcp.toml', revision, authored: {
    [id]: { definition: { type: 'stdio', command: id }, retained_env: [], retained_headers: [] },
  } },
});

it('opens Client-only Skills without a Server Workspace source observation', async () => {
  const subject = cfg3Client(async op => {
    if (op.method === 'configuration/sourcesRead' && op.params.target.kind === 'workspace') throw new Error('Server path absent');
    if (op.method !== 'skills/list' || op.params.target.kind !== 'workspace') return;
    expect(op.params).toMatchObject({ execution_session: first.id, target: { kind: 'workspace', directory: cwd } });
    const path = cwd + '/.agents/skills/client-guide/SKILL.md';
    return { type: 'skills', authority: 'client', page: { revision: 'client-skills', diagnostics: [], diagnostics_truncated: false, data: [{
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

it('keeps unclaimed selected Sessions editable via Server authoring for MCP and Skills', async () => {
  const subject = cfg3Client(async op => {
    if (op.method === 'mcp/sourceRead') return { type: 'mcp_source', authority: 'authoring' };
    if (op.method === 'skills/list' && op.params.target.kind === 'workspace') {
      return { type: 'skills', authority: 'authoring', page: { revision: 'authored-skills', diagnostics: [], diagnostics_truncated: false, data: [{
        catalog: { name: 'authored-skill', description: 'Authored skill', location: cwd + '/.agents/skills/authored-skill/SKILL.md' },
        provenance: { name: 'authored-skill', source: 'workspace', location: cwd + '/.agents/skills/authored-skill/SKILL.md', shadowed: [] },
        operation_revision: 'authored-skill-write', metadata_revision: 'authored-skill', disable_model_invocation: false, enabled_by_default: true,
      }] } };
    }
  });
  subject.source.workspace_mcp = {
    path: cwd + '/.agents/mcp.toml', revision: 'server-rev',
    authored: { authored: { definition: { type: 'stdio', command: 'authored-cmd' }, retained_env: [], retained_headers: [] } },
  };
  const { associations, surface } = mountResources(subject);
  const stop = associations.start();
  try {
    const mcpUi = render(surface('mcp', first));
    await screen.findByRole('listitem', { name: 'authored' });
    await waitFor(() => expect(document.querySelector('[data-resource-module="mcp"]')?.getAttribute('data-lifecycle')).toBe('ready'));
    expect((screen.getByRole('button', { name: /New MCP server/ }) as HTMLButtonElement).disabled).toBe(false);
    mcpUi.unmount();

    render(surface('skills', first));
    await waitFor(() => {
      expect(document.querySelector('[data-resource-module="skills"]')?.getAttribute('data-lifecycle')).toBe('ready');
      const skill = screen.getByRole('listitem', { name: 'authored-skill' });
      expect(within(skill).getByRole('button', { name: /Actions/ })).toBeTruthy();
    });
  } finally { stop(); }
});

it('never falls back to Server authoring when a claimed Session Client is unavailable', async () => {
  const subject = cfg3Client(async op => {
    if (op.method === 'mcp/sourceRead') throw new Error('ExecutionUnavailable');
    if (op.method === 'skills/list' && op.params.target.kind === 'workspace') throw new Error('ExecutionUnavailable');
  });
  subject.source.workspace_mcp = {
    path: cwd + '/.agents/mcp.toml', revision: 'server-rev',
    authored: { decoy: { definition: { type: 'stdio', command: 'decoy-cmd' }, retained_env: [], retained_headers: [] } },
  };
  const { associations, surface } = mountResources(subject);
  const stop = associations.start();
  try {
    const mcpUi = render(surface('mcp', first));
    await screen.findByRole('alert');
    expect(document.querySelector('[data-resource-module="mcp"]')?.getAttribute('data-lifecycle')).toBe('failed');
    expect(screen.queryByRole('listitem', { name: 'decoy' })).toBeNull();
    expect(subject.request.mock.calls.some(([op]) =>
      op.method === 'configuration/sourcesRead' && op.params.target.kind === 'workspace')).toBe(false);
    mcpUi.unmount();

    render(surface('skills', first));
    await screen.findByRole('alert');
    await waitFor(() => expect(document.querySelector('[data-resource-module="skills"]')?.getAttribute('data-lifecycle')).toBe('failed'));
    expect(subject.request.mock.calls.some(([op]) =>
      op.method === 'configuration/sourcesRead' && op.params.target.kind === 'workspace')).toBe(false);
  } finally { stop(); }
});

it('refreshes Skills authority when the same Workspace Session acquires a Client', async () => {
  const skills = (authority: 'authoring' | 'client', name: string): MethodResult => {
    const location = `${cwd}/.agents/skills/${name}/SKILL.md`;
    return { type: 'skills', authority, page: { revision: name, diagnostics: [], diagnostics_truncated: false, data: [{
      catalog: { name, description: name, location },
      provenance: { name, source: 'workspace', location, shadowed: [] },
      operation_revision: name, metadata_revision: name, disable_model_invocation: false, enabled_by_default: true,
    }] } };
  };
  let current: Promise<MethodResult> = Promise.resolve(skills('authoring', 'authored-skill'));
  const subject = cfg3Client(op => op.method === 'skills/list' && op.params.target.kind === 'workspace' ? current : Promise.resolve());
  const { associations, surface } = mountResources(subject);
  const stop = associations.start();
  try {
    const ui = render(surface('skills', first));
    const page = ui.container.querySelector('[data-resource-module="skills"]');
    await waitFor(() => expect(within(screen.getByRole('listitem', { name: 'authored-skill' })).getByRole('button', { name: /Actions/ }).matches(':disabled')).toBe(false));
    const refresh = screen.getByRole('button', { name: 'Refresh' });
    const workspaceLists = () => subject.request.mock.calls.flatMap(([op]) => op.method === 'skills/list' && op.params.target.kind === 'workspace' ? [op.params] : []);
    const generation = subject.state.generation;

    let fail!: (error: Error) => void;
    current = new Promise((_resolve, reject) => { fail = reject; });
    const beforeFailure = workspaceLists().length;
    fireEvent.click(refresh);
    await waitFor(() => expect(workspaceLists().length).toBeGreaterThan(beforeFailure));
    expect(workspaceLists().slice(beforeFailure).every(params => params.execution_session === first.id)).toBe(true);
    expect(page?.getAttribute('data-lifecycle')).toBe('loading');
    await act(async () => fail(new Error('ExecutionUnavailable')));
    await screen.findByRole('alert');
    expect(page?.getAttribute('data-lifecycle')).toBe('failed');

    let complete!: (result: MethodResult) => void;
    current = new Promise(resolve => { complete = resolve; });
    const beforeRecovery = workspaceLists().length;
    fireEvent.click(refresh);
    await waitFor(() => expect(workspaceLists().length).toBeGreaterThan(beforeRecovery));
    expect(workspaceLists().slice(beforeRecovery).every(params => params.execution_session === first.id)).toBe(true);
    expect(page?.getAttribute('data-lifecycle')).toBe('loading');
    await act(async () => complete(skills('client', 'client-skill')));
    const clientSkill = await screen.findByRole('listitem', { name: 'client-skill' });
    await waitFor(() => expect(page?.getAttribute('data-lifecycle')).toBe('ready'));
    expect(within(clientSkill).queryByRole('button', { name: /Actions/ })).toBeNull();
    expect(screen.queryByRole('listitem', { name: 'authored-skill' })).toBeNull();
    expect(subject.state.generation).toBe(generation);
  } finally { stop(); }
});

const skillResult = (authority: 'authoring' | 'client', name: string): MethodResult => {
  const location = `${cwd}/.agents/skills/${name}/SKILL.md`;
  return { type: 'skills', authority, page: { revision: name, diagnostics: [], diagnostics_truncated: false, data: [{
    catalog: { name, description: name, location },
    provenance: { name, source: 'workspace', location, shadowed: [] },
    ...(authority === 'authoring' ? { operation_revision: name } : {}),
    metadata_revision: name, disable_model_invocation: false, enabled_by_default: true,
  }] } };
};


it('uses the authority of a follow-up Skills catalog without a manual refresh', async () => {
  let respond!: (result: MethodResult) => void;
  const followUp = new Promise<MethodResult>(resolve => { respond = resolve; });
  let requests = 0;
  const subject = cfg3Client(async op => {
    if (op.method !== 'skills/list' || op.params.target.kind !== 'workspace') return;
    expect(op.params).toMatchObject({ execution_session: first.id, target: { kind: 'workspace', directory: cwd } });
    return ++requests === 1 ? skillResult('authoring', 'authored-skill') : followUp;
  });
  const { associations, surface } = mountResources(subject);
  const stop = associations.start();
  try {
    const generation = subject.state.generation;
    const ui = render(surface('skills', first));
    const page = ui.container.querySelector('[data-resource-module="skills"]');
    await waitFor(() => expect(requests).toBe(2));
    expect(subject.request.mock.calls.some(([op]) =>
      op.method === 'configuration/sourcesRead' && op.params.target.kind === 'workspace')).toBe(true);
    expect(page?.getAttribute('data-lifecycle')).toBe('loading');
    expect(screen.getByRole('listitem', { name: 'authored-skill' })).toBeTruthy();
    expect(screen.getByRole('region', { name: 'Skill access' })).toBeTruthy();

    await act(async () => respond(skillResult('client', 'client-skill')));
    const clientSkill = await screen.findByRole('listitem', { name: 'client-skill' });
    expect(page?.getAttribute('data-lifecycle')).toBe('ready');
    expect(within(clientSkill).queryByRole('switch')).toBeNull();
    expect(within(clientSkill).queryByRole('button', { name: /Actions/ })).toBeNull();
    expect(screen.queryByRole('region', { name: 'Skill access' })).toBeNull();
    expect(screen.queryByRole('listitem', { name: 'authored-skill' })).toBeNull();
    expect(subject.state.generation).toBe(generation);
  } finally { stop(); }
});

it('tracks bound Skills data-lifecycle from the real skills/list response', async () => {
  let resolveSkills!: (result: MethodResult) => void;
  let rejectSkills!: (error: Error) => void;
  let attempt = 0;
  const subject = cfg3Client(async op => {
    if (op.method !== 'skills/list' || op.params.target.kind !== 'workspace') return;
    attempt += 1;
    if (attempt === 1) {
      return new Promise<MethodResult>((resolve, reject) => {
        resolveSkills = resolve;
        rejectSkills = reject;
      });
    }
    return new Promise<MethodResult>((resolve, reject) => {
      resolveSkills = resolve;
      rejectSkills = reject;
    });
  });
  const { associations, surface } = mountResources(subject, true);
  const stop = associations.start();
  try {
    const ui = render(surface('skills', first));
    await waitFor(() => expect(attempt).toBe(1));
    expect(document.querySelector('[data-resource-module="skills"]')?.getAttribute('data-lifecycle')).toBe('loading');
    const path = cwd + '/.agents/skills/live-skill/SKILL.md';
    await act(async () => resolveSkills({
      type: 'skills',
      authority: 'client',
      page: { revision: 'client-skills', diagnostics: [], diagnostics_truncated: false, data: [{
        catalog: { name: 'live-skill', description: 'Loaded from Client', location: path },
        provenance: { name: 'live-skill', source: 'workspace', location: path, shadowed: [] },
        metadata_revision: 'client-skill', disable_model_invocation: false, enabled_by_default: true,
      }] },
    }));
    await waitFor(() => expect(document.querySelector('[data-resource-module="skills"]')?.getAttribute('data-lifecycle')).toBe('ready'));
    expect(screen.getByRole('listitem', { name: 'live-skill' })).toBeTruthy();

    ui.rerender(surface('skills', second));
    await waitFor(() => expect(attempt).toBe(2));
    expect(document.querySelector('[data-resource-module="skills"]')?.getAttribute('data-lifecycle')).toBe('loading');
    await act(async () => rejectSkills(new Error('Client disconnected')));
    await waitFor(() => expect(document.querySelector('[data-resource-module="skills"]')?.getAttribute('data-lifecycle')).toBe('failed'));
    expect(screen.getByRole('alert')).toBeTruthy();
  } finally { stop(); }
});

it.each(['failed', 'client'] as const)('revokes a Skill transfer after the next scoped read is %s', async outcome => {
  let respond!: (result: MethodResult) => void;
  let fail!: (error: Error) => void;
  const next = new Promise<MethodResult>((resolve, reject) => { respond = resolve; fail = reject; });
  let rechecking = false;
  let requests = 0;
  const subject = cfg3Client(async op => {
    if (op.method !== 'skills/list' || op.params.target.kind !== 'workspace') return;
    expect(op.params.execution_session).toBe(first.id);
    requests += 1;
    return rechecking ? next : skillResult('authoring', 'authored-skill');
  });
  subject.publish({ authorityId: 'native-fixture' });
  const { associations, host, surface } = mountResources(subject);
  const configure = vi.spyOn(host, 'configureWorkspace');
  const stop = associations.start();
  try {
    const ui = render(surface('skills', first));
    const page = ui.container.querySelector('[data-resource-module="skills"]');
    const row = await screen.findByRole('listitem', { name: 'authored-skill' });
    await waitFor(() => expect(page?.getAttribute('data-lifecycle')).toBe('ready'));
    fireEvent.click(within(row).getByRole('button', { name: /Actions/ }));
    const action = await screen.findByRole('menuitem', { name: outcome === 'failed' ? 'Copy to Global' : 'Move to Global' });
    await waitFor(() => expect(action.getAttribute('aria-disabled')).not.toBe('true'));
    fireEvent.click(action);
    const dialog = await screen.findByRole('dialog');
    const formerSubmit = within(dialog).getByRole('button', { name: outcome === 'failed' ? 'Copy' : 'Move' });
    expect((formerSubmit as HTMLButtonElement).disabled).toBe(false);
    const generation = subject.state.generation;
    const before = requests;
    rechecking = true;

    // A new source observation triggers the scoped catalog on the same Session
    // without remounting the page through its manual Refresh.
    await act(async () => {
      configurationSystem(subject.client).settingsTarget(workspaceSettingsTarget('A', 'A'), () => host).send({ type: 'REFRESH' });
    });
    await waitFor(() => expect(requests).toBeGreaterThan(before));
    await act(async () => {
      if (outcome === 'client') respond(skillResult('client', 'client-skill'));
      else fail(new Error('ExecutionUnavailable'));
    });
    await waitFor(() => expect(page?.getAttribute('data-lifecycle')).toBe(outcome === 'client' ? 'ready' : 'failed'));
    expect(screen.queryByRole('dialog')).toBeNull();
    fireEvent.click(formerSubmit);
    expect(configure.mock.calls.filter(([, , operation]) => operation.kind === 'transfer')).toHaveLength(0);
    expect(subject.state.generation).toBe(generation);
  } finally { stop(); }
});

it('checks the transfer intent again before forwarding to the Host', async () => {
  const subject = cfg3Client();
  subject.publish({ authorityId: 'native-fixture' });
  const { authority, host } = mountResources(subject);
  const configure = vi.spyOn(host, 'configureWorkspace').mockResolvedValue({ kind: 'transfer' });
  await authority.observe();
  const owner = authority.capture();
  expect(owner?.current()).toBe(true);
  let current = true;
  const submit = bindResourceTransfer(subject.client, () => host, owner, 'A', () => current);
  const mutation = { kind: 'resource_transfer', from: 'workspace', operation: 'copy',
    resource: { family: 'skill', source: 'skill', destination: 'skill-copy' } } as const;
  await submit(cwd, 'current', mutation);
  expect(configure).toHaveBeenCalledTimes(1);
  current = false;
  await expect(submit(cwd, 'current', mutation)).rejects.toMatchObject({ kind: 'workspace_unavailable' });
  expect(configure).toHaveBeenCalledTimes(1);
});

async function openSkillCopy(name = 'authored-skill') {
  const row = await screen.findByRole('listitem', { name });
  fireEvent.click(within(row).getByRole('button', { name: /Actions/ }));
  fireEvent.click(await screen.findByRole('menuitem', { name: 'Copy to Global' }));
  return screen.findByRole('dialog', { name: 'Copy resource' });
}

it.each([
  [new WorkspaceHostError('transport lost the reply', undefined, true), 'The transfer may have committed.'],
  [new WorkspaceHostError('source changed', undefined, false, { kind: 'source_conflict', scope: 'workspace', expected: 'old', actual: 'new' }), 'The source changed.'],
] as const)('preserves a submitted Skill transfer result through its own refresh', async (error, notice) => {
  let reads = 0;
  const subject = cfg3Client(async op => {
    if (op.method === 'skills/list' && op.params.target.kind === 'workspace') {
      reads += 1;
      return skillResult('authoring', 'authored-skill');
    }
  });
  subject.publish({ authorityId: 'native-fixture' });
  const { associations, host, surface } = mountResources(subject);
  let fail!: (error: Error) => void;
  const transfer = new Promise<never>((_, reject) => { fail = reject; });
  const original = host.configureWorkspace!;
  const configure = vi.spyOn(host, 'configureWorkspace').mockImplementation((...args) =>
    args[2].kind === 'transfer' ? transfer : original(...args));
  const stop = associations.start();
  try {
    const ui = render(surface('skills', first));
    const page = ui.container.querySelector('[data-resource-module="skills"]');
    await waitFor(() => expect(page?.getAttribute('data-lifecycle')).toBe('ready'));
    const dialog = await openSkillCopy();
    const submit = within(dialog).getByRole('button', { name: 'Copy' });
    fireEvent.click(submit);
    const writes = () => configure.mock.calls.filter(([, , request]) => request.kind === 'transfer');
    expect(writes()).toHaveLength(1);
    const before = reads;
    await act(async () => fail(error));
    await waitFor(() => expect(reads).toBeGreaterThan(before));
    await waitFor(() => expect(page?.getAttribute('data-lifecycle')).toBe('ready'));
    expect(screen.getByRole('alert').textContent).toContain(notice);
    expect(screen.queryByRole('dialog')).toBeNull();
    fireEvent.click(submit);
    expect(writes()).toHaveLength(1);
  } finally { stop(); }
});

it('keeps a new Skill transfer open when the superseded request finishes', async () => {
  const subject = cfg3Client(async op => {
    if (op.method === 'skills/list' && op.params.target.kind === 'workspace') return skillResult('authoring', 'authored-skill');
  });
  subject.publish({ authorityId: 'native-fixture' });
  const { associations, host, surface } = mountResources(subject);
  let finish!: (result: { kind: 'transfer' }) => void;
  const transfer = new Promise<{ kind: 'transfer' }>(resolve => { finish = resolve; });
  const original = host.configureWorkspace!;
  const configure = vi.spyOn(host, 'configureWorkspace').mockImplementation((...args) =>
    args[2].kind === 'transfer' ? transfer : original(...args));
  const stop = associations.start();
  try {
    const ui = render(surface('skills', first));
    const ready = () => waitFor(() => expect(ui.container.querySelector('[data-resource-module="skills"]')?.getAttribute('data-lifecycle')).toBe('ready'));
    await ready();
    const previous = await openSkillCopy();
    fireEvent.click(within(previous).getByRole('button', { name: 'Copy' }));
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    await ready();
    expect(screen.queryByRole('dialog')).toBeNull();
    const latest = await openSkillCopy();
    const destination = within(latest).getByRole('textbox');
    fireEvent.change(destination, { target: { value: 'new-copy' } });
    await act(async () => finish({ kind: 'transfer' }));
    expect(screen.getByRole('dialog')).toBe(latest);
    expect((destination as HTMLInputElement).value).toBe('new-copy');
    expect(configure.mock.calls.filter(([, , request]) => request.kind === 'transfer')).toHaveLength(1);
  } finally { stop(); }
});

it('retains independent Skill transfer results arriving in reverse order', async () => {
  let releaseCatalog!: (value: MethodResult) => void;
  let catalogRead: Promise<MethodResult> | undefined;
  const subject = cfg3Client(async op => {
    if (op.method !== 'skills/list' || op.params.target.kind !== 'workspace') return;
    const firstSkill = skillResult('authoring', 'authored-skill');
    const secondSkill = skillResult('authoring', 'other-skill');
    if (firstSkill.type !== 'skills' || secondSkill.type !== 'skills') throw new Error('Skills fixture');
    firstSkill.page.data.push(...secondSkill.page.data);
    return await catalogRead ?? firstSkill;
  });
  subject.publish({ authorityId: 'native-fixture' });
  const { associations, host, surface } = mountResources(subject);
  const replies = new Map<string, (error: WorkspaceHostError) => void>();
  const configure = host.configureWorkspace!;
  const observed = vi.spyOn(host, 'configureWorkspace').mockImplementation((...args) => {
    const operation = args[2];
    if (operation.kind !== 'transfer') return configure(...args);
    return new Promise((_, reject) => replies.set(operation.mutation.resource.source, reject));
  });
  const stop = associations.start();
  try {
    const ui = render(surface('skills', first));
    const page = ui.container.querySelector('[data-resource-module="skills"]');
    const ready = () => waitFor(() => expect(page?.getAttribute('data-lifecycle')).toBe('ready'));
    await ready();
    const older = await openSkillCopy();
    fireEvent.click(within(older).getByRole('button', { name: 'Copy' }));
    expect(replies.has('authored-skill')).toBe(true);

    catalogRead = new Promise(resolve => { releaseCatalog = resolve; });
    await act(async () => {
      configurationSystem(subject.client).settingsTarget(workspaceSettingsTarget('A', 'A'), () => host).send({ type: 'REFRESH' });
    });
    await waitFor(() => expect(page?.getAttribute('data-lifecycle')).toBe('loading'));
    expect(screen.queryByRole('dialog')).toBeNull();
    const restored = skillResult('authoring', 'authored-skill');
    const other = skillResult('authoring', 'other-skill');
    if (restored.type !== 'skills' || other.type !== 'skills') throw new Error('Skills fixture');
    restored.page.data.push(...other.page.data);
    await act(async () => {
      catalogRead = undefined;
      releaseCatalog(restored);
    });
    await ready();

    const later = await openSkillCopy('other-skill');
    fireEvent.click(within(later).getByRole('button', { name: 'Copy' }));
    expect(replies.has('other-skill')).toBe(true);
    await act(async () => replies.get('other-skill')!(new WorkspaceHostError('source changed', undefined, false, {
      kind: 'source_conflict', scope: 'workspace', expected: 'old', actual: 'new',
    })));
    await ready();
    await act(async () => replies.get('authored-skill')!(new WorkspaceHostError('reply lost', undefined, true)));
    await ready();

    const notices = screen.getAllByRole('alert');
    expect(notices).toHaveLength(2);
    expect(notices[0].textContent).toContain('other-skill The source changed.');
    expect(notices[1].textContent).toContain('authored-skill The transfer may have committed.');
    fireEvent.click(within(notices[0]).getByRole('button', { name: 'Close' }));
    expect(screen.getByRole('alert').textContent).toContain('authored-skill The transfer may have committed.');
    expect(observed.mock.calls.filter(([, , operation]) => operation.kind === 'transfer')).toHaveLength(2);
  } finally { stop(); }
});
