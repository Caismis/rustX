import { cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { useWorkflowContent } from '../src/app/agent/useWorkflowContent';
import { checkpoint, Server, snapshot } from './fixture';
import { workflowRun } from './workflow-data';
afterEach(cleanup);

it('reads all retained nodes beyond the finite display header at one cut and retires hydrated values when reading history', async () => {
  const server = new Server(); await server.attached('A');
  try {
    const original = workflowRun();
    original.instances = Array.from({ length: 40 }, (_, index) => ({ ...original.instances[1], node: `node-${index}` }));
    const s = snapshot(); s.workflows = { revision: '1', runs: [{ ...original, instances: original.instances.slice(0, 32) }], omitted_runs: 0 };
    const check = checkpoint(s); check.collections = [
      { domain: 'workflows.runs', revision: '1', total: '1', loaded: '1', next_offset: null },
      { domain: 'workflows.runs.@0.instances', revision: '1', total: '40', loaded: '32', next_offset: '32' },
    ];
    const view = { ...server.client.getSnapshot().views.A, snapshot: s, checkpoint: check };
    server.handlers.set('conversation/checkpointPage', () => ({ type: 'checkpoint_page', page: { domain: 'workflows.runs', cursor: check.read_cut.revision, read_cut: check.read_cut, total: '1', items: [{ offset: '0', identity: 'run', value: original as never, deferred: false, content: [] }], next_offset: null, collections: [] } }));
    const ui = renderHook(({ view, active }) => useWorkflowContent(server.client, view, active), { initialProps: { view, active: true } });
    await waitFor(() => expect(ui.result.current.snapshot?.runs[0].instances).toHaveLength(40));
    const request = server.requests.find(row => row.request.method === 'conversation/checkpointPage')!.request;
    expect(request.params).toMatchObject({ cut: check.read_cut, domain: 'workflows.runs', agent_id: null });
    ui.rerender({ view, active: false });
    expect(ui.result.current.snapshot?.runs[0].instances).toHaveLength(32);
    expect(s.workflows.runs[0].instances).toHaveLength(32);
  } finally { server.client.disconnect(); }
});
