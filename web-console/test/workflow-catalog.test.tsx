import { act, cleanup, fireEvent, render, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { SubagentHeader, SubagentScope } from '../src/app/agent/Subagents';
import { WorkflowRunPanel } from '../src/app/agent/WorkflowRunPanel';
import { useSubagents } from '../src/app/agent/subagent-context';
import { workflowChildren } from '../src/bindings/workflow-children';
import { checkpoint, Server } from './fixture';
import { installWorkflowChildStatus, workflowCatalogSnapshot, workflowChildHeader } from './workflow-catalog-data';

const servers: Server[] = [];
afterEach(() => { cleanup(); servers.splice(0).forEach(server => server.client.disconnect()); });
async function setup(value = workflowCatalogSnapshot()) {
  const server = new Server(); servers.push(server); server.snapshots.set('A', value);
  await server.attached('A'); installWorkflowChildStatus(server); return server;
}
function Controls() {
  const scope = useSubagents()!;
  return <><output data-testid="selection">{scope.selected ? `${scope.selected.title}:${scope.oneShot?.has(scope.selected.agent_id)}` : 'parent'}</output>
    <button onClick={() => scope.open()}>Return to parent</button>
    <button onClick={() => scope.selected && scope.close(scope.selected.agent_id)}>Close inspected record</button>
    <button onClick={() => void scope.openWorkflow?.('workflow-agent-0', 'workflow-activation-0', 'explore')}>Open workflow node</button></>;
}
const surface = (server: Server, sessionId = 'A') => <SubagentScope client={server.client} sessionId={sessionId}><SubagentHeader title="Parent"/><Controls/></SubagentScope>;

it('lists three settled workflow children before inspection; opening, closing and remounting never change membership', async () => {
  const server = await setup(), ui = render(surface(server));
  expect(ui.getByRole('button', { name: 'Subagents' }).textContent).toContain('3 subagents');
  expect(server.requests.filter(row => row.request.method === 'agent/status')).toHaveLength(0);
  fireEvent.click(ui.getByRole('button', { name: 'Subagents' }));
  expect(ui.getAllByRole('treeitem')).toHaveLength(3);
  expect(ui.getAllByText('Workflow · code_quality_gate · Read-only')).toHaveLength(3);
  for (const node of ['explore', 'review', 'test']) {
    fireEvent.click(ui.getByRole('treeitem', { name: `${node} completed` }));
    await waitFor(() => expect(ui.getByTestId('selection').textContent).toBe(`${node}:true`));
    fireEvent.click(ui.getByText('Close inspected record'));
    expect(ui.getByRole('button', { name: 'Subagents' }).textContent).toContain('3 subagents');
    fireEvent.click(ui.getByRole('button', { name: 'Subagents' }));
    expect(ui.getAllByRole('treeitem')).toHaveLength(3);
  }
  ui.unmount(); const reopened = render(surface(server));
  expect(reopened.getByRole('button', { name: 'Subagents' }).textContent).toContain('3 subagents');
  fireEvent.click(reopened.getByText('Open workflow node'));
  await waitFor(() => expect(reopened.getByTestId('selection').textContent).toBe('explore:true'));
  fireEvent.click(reopened.getByText('Return to parent'));
  expect(reopened.getByRole('button', { name: 'Subagents' }).textContent).toContain('3 subagents');
});

it('mixes normal and workflow children without double counting inspected records and updates native states', async () => {
  const value = workflowCatalogSnapshot(); value.agents = [{ ...workflowChildHeader(9), agent_id: 'normal', title: 'Normal agent' }];
  value.workflows.runs[0].instances[0].state = { type: 'running' };
  const server = await setup(value), ui = render(surface(server));
  expect(ui.getByRole('button', { name: 'Subagents' }).textContent).toContain('4 subagents');
  fireEvent.click(ui.getByRole('button', { name: 'Subagents' }));
  expect(ui.getAllByRole('treeitem')).toHaveLength(4);
  expect(ui.getByRole('treeitem', { name: 'explore running' })).toBeTruthy();
  const next = structuredClone(value); next.workflows.revision = '2'; next.workflows.runs[0].instances[0].state = { type: 'settled', outcome: 'failed' };
  await act(async () => server.update('A', next));
  expect(ui.getByRole('treeitem', { name: 'explore failed' })).toBeTruthy();
  expect(ui.getByRole('button', { name: 'Subagents' }).textContent).toContain('4 subagents');
});

it('counts only exact admitted child identities, including repeated loop nodes, and excludes other conversations', () => {
  const value = workflowCatalogSnapshot(), run = value.workflows.runs[0];
  run.instances.push(run.instances[0], { ...run.instances[0], child: 'loop-activation', child_agent_id: 'loop-agent', visit: 2 },
    { ...run.instances[1], child: null }, { ...run.instances[1], child_agent_id: null },
    { ...run.instances[1], child: 'pending', child_agent_id: 'pending', state: { type: 'pending' } }, { ...run.instances[1], kind: 'tool' });
  value.workflows.runs.push({ ...run, id: { ...run.id, conversation_id: 'other' } });
  expect(workflowChildren(value.workflows.runs, value.conversation_id).map(child => child.activationId)).toEqual(['workflow-activation-0', 'workflow-activation-1', 'workflow-activation-2', 'loop-activation']);
});

it('never promotes an inspected workflow record into a normal Agent tree branch', async () => {
  const value = workflowCatalogSnapshot(); value.agents = [{ ...workflowChildHeader(9), agent_id: 'normal', title: 'Normal agent' }];
  const server = await setup(value); server.handlers.set('agent/status', () => ({ type: 'domain_page', page: {
    cursor: '1', read_cut: server.client.getSnapshot().views.A.checkpoint!.read_cut, domain: 'workflow_children.workflow-activation-0', total: '1',
    items: [{ offset: '0', identity: 'workflow-agent-0', value: { ...workflowChildHeader(0), parent_agent_id: 'normal' } as never, content: [], deferred: false }], next_offset: null,
  } }));
  const ui = render(surface(server)); fireEvent.click(ui.getByText('Open workflow node'));
  await waitFor(() => expect(ui.getByTestId('selection').textContent).toBe('explore:true'));
  fireEvent.click(ui.getByText('Return to parent')); fireEvent.click(ui.getByRole('button', { name: 'Subagents' }));
  expect(ui.getAllByRole('treeitem')).toHaveLength(4);
  expect(ui.queryByRole('button', { name: 'Expand Normal agent' })).toBeNull();
});

it('hydrates children beyond the finite header at the same native cut, without status prefetch or exact partial counts', async () => {
  const server = await setup(), value = workflowCatalogSnapshot(), run = value.workflows.runs[0];
  const complete = structuredClone(run); run.instances = run.instances.slice(0, 1);
  const cut = checkpoint(value); cut.collections = [
    { domain: 'workflows.runs', revision: '1', total: '1', loaded: '1', next_offset: null },
    { domain: 'workflows.runs.@0.instances', revision: '1', total: '3', loaded: '1', next_offset: '1' },
  ];
  server.handlers.set('session/checkpoint', () => ({ type: 'checkpoint', target: server.client.getSnapshot().views.A.target!, checkpoint: cut }));
  server.handlers.set('conversation/checkpointPage', () => ({ type: 'checkpoint_page', page: { domain: 'workflows.runs', cursor: '0', read_cut: cut.read_cut,
    total: '1', items: [{ offset: '0', identity: 'run', value: complete as never, deferred: false, content: [] }], next_offset: null, collections: [] } }));
  await server.client.refresh('A'); server.held.add('conversation/checkpointPage');
  const ui = render(surface(server));
  expect(ui.getByRole('button', { name: 'Subagents' }).textContent).toContain('At least 1 subagents');
  const read = await server.waitFor('conversation/checkpointPage', 1);
  await act(async () => server.reply(read));
  await waitFor(() => expect(ui.getByRole('button', { name: 'Subagents' }).textContent).toContain('3 subagents'));
  expect(server.requests.filter(row => row.request.method === 'agent/status')).toHaveLength(0);
  expect(server.requests.filter(row => row.request.method === 'conversation/checkpointPage')).toHaveLength(1);
});

it('keeps a failed child selection retryable and fences late replies after a session switch', async () => {
  const server = await setup(); await server.attached('B'); server.held.add('agent/status');
  const ui = render(surface(server)); fireEvent.click(ui.getByRole('button', { name: 'Subagents' }));
  const row = ui.getByRole('treeitem', { name: 'explore completed' }); fireEvent.click(row);
  expect(row.getAttribute('aria-busy')).toBe('true'); fireEvent.click(row);
  const request = await server.waitFor('agent/status', 1);
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'Record unavailable' } }));
  expect(await ui.findByRole('alert')).toHaveProperty('textContent', 'Could not open this record. Select it again to retry.');
  fireEvent.click(row); const retry = await server.waitFor('agent/status', 2);
  ui.rerender(surface(server, 'B'));
  await act(async () => server.reply(retry));
  expect(within(ui.getByTestId('selection')).getByText('parent')).toBeTruthy();
  expect(ui.queryByRole('button', { name: 'Subagents' })).toBeNull();
});

it.each(['message', 'catalog'] as const)('keeps retained Workflow records non-actionable until reattachment, then opens once from the %s', async entry => {
  const value = workflowCatalogSnapshot(), run = value.workflows.runs[0];
  run.state = { type: 'running' };
  const server = await setup(value);
  const onMode = vi.fn();
  const ui = render(<SubagentScope client={server.client} sessionId="A" onMode={onMode}><SubagentHeader title="Parent"/><Controls/><WorkflowRunPanel run={run}/></SubagentScope>);
  const node = ui.getByRole('button', { name: 'View explore' });
  act(() => node.focus());
  expect(document.activeElement).toBe(node);
  const statusReads = () => server.requests.filter(row => row.request.method === 'agent/status');
  expect(statusReads()).toHaveLength(0);

  await act(async () => { await server.client.disconnect(); });
  const expectRetainedWithoutAuthority = () => {
    // Historical content remains readable. Losing its attachment must not
    // leave a button that silently accepts and then drops an inspection.
    expect(ui.container.querySelector('[data-workflow-node="explore"]')).toBe(node);
    expect(node.textContent).toContain('explore');
    expect(document.activeElement).toBe(node);
    expect(node.getAttribute('aria-disabled')).toBe('true');
    fireEvent.click(node);
    expect(ui.getByTestId('selection').textContent).toBe('parent');
    expect(statusReads()).toHaveLength(0);
    // The catalog has no current Conversation target until attach completes.
    expect(ui.queryByRole('button', { name: 'Subagents' })).toBeNull();
    expect(ui.queryAllByRole('treeitem')).toHaveLength(0);
  };
  expectRetainedWithoutAuthority();

  server.held.add('session/attach');
  await act(async () => { await server.connect(); });
  const attach = await server.waitFor('session/attach', 2);
  expect(server.client.getSnapshot().connection).toBe('connected');
  expect(server.client.getSnapshot().views.A.attachmentObservation).toBeUndefined();
  expectRetainedWithoutAuthority();

  await act(async () => { server.reply(attach); });
  await waitFor(() => expect(node.getAttribute('aria-disabled')).toBeNull());
  expect(statusReads()).toHaveLength(0);
  expect(ui.getByRole('button', { name: 'Subagents' }).textContent).toContain('3 subagents');
  if (entry === 'catalog') {
    fireEvent.click(ui.getByRole('button', { name: 'Subagents' }));
    const row = ui.getByRole('treeitem', { name: 'explore completed' });
    expect(row.getAttribute('aria-disabled')).toBeNull();
    fireEvent.click(row);
  } else fireEvent.click(node);
  await waitFor(() => expect(ui.getByTestId('selection').textContent).toBe('explore:true'));
  expect(statusReads()[0].request).toMatchObject({ method: 'agent/status', params: { agent_id: 'workflow-agent-0' } });
  expect(onMode).toHaveBeenCalledTimes(1);
  expect(onMode).toHaveBeenCalledWith('chat');
});

it('marks retired records as a lower bound and does not count opened views after their native records retire', async () => {
  const server = await setup(), ui = render(surface(server));
  fireEvent.click(ui.getByText('Open workflow node')); await waitFor(() => expect(ui.getByTestId('selection').textContent).toBe('explore:true'));
  const next = workflowCatalogSnapshot(); next.workflows.runs = []; next.workflows.omitted_runs = 1; next.workflows.revision = '2';
  await act(async () => server.update('A', next)); fireEvent.click(ui.getByText('Return to parent'));
  expect(ui.getByRole('button', { name: 'Subagents' }).textContent).toContain('Subagent records');
  fireEvent.click(ui.getByRole('button', { name: 'Subagents' }));
  expect(ui.queryAllByRole('treeitem')).toHaveLength(0);
  expect(ui.getByText(/Only retained workflow records/).getAttribute('role')).toBe('status');
});
