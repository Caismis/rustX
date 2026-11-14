import { act, cleanup, fireEvent, render, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { useWorkflowContent } from '../src/app/agent/useWorkflowContent';
import { checkpoint, Server, snapshot } from './fixture';
import { workflowRun } from './workflow-data';
import { RpcFailure } from '../src/client/errors';
import { WorkflowRunPanel } from '../src/app/agent/WorkflowRunPanel';
afterEach(cleanup);

it('keeps the complete Workflow snapshot during a bounded refresh and publishes its replacement atomically', async () => {
  const server = new Server(); await server.attached('A');
  try {
    const original = workflowRun();
    const s = { ...snapshot(), workflows: { revision: '1', runs: [original], omitted_runs: 0 } };
    const view = { ...server.client.getSnapshot().views.A, snapshot: s, checkpoint: checkpoint(s) };
    const ui = renderHook(({ view }) => useWorkflowContent(server.client, view, true), { initialProps: { view } });
    const replacement = { ...original, instances: [...original.instances, { ...original.instances[1], node: 'new-node' }] };
    const next = { ...s, workflows: { revision: '2', runs: [{ ...replacement, instances: replacement.instances.slice(0, 1) }], omitted_runs: 0 } };
    const check = checkpoint(next, '2');
    check.collections = [{ domain: 'workflows.runs', revision: '2', total: '1', loaded: '1', next_offset: null }, { domain: 'workflows.runs.@0.instances', revision: '2', total: String(replacement.instances.length), loaded: '1', next_offset: '1' }];
    server.held.add('conversation/checkpointPage');
    ui.rerender({ view: { ...view, snapshot: next, checkpoint: check } });
    const request = await server.waitFor('conversation/checkpointPage', 1);
    expect(ui.result.current.snapshot).toBe(s.workflows);
    await act(async () => server.socket.success(request, { type: 'checkpoint_page', page: { domain: 'workflows.runs', cursor: check.read_cut.revision, read_cut: check.read_cut, total: '1', items: [{ offset: '0', identity: 'run', value: replacement as never, deferred: false, content: [] }], next_offset: null, collections: [] } }));
    expect(ui.result.current.snapshot?.revision).toBe('2');
    expect(ui.result.current.snapshot?.runs[0].instances).toEqual(replacement.instances);
    const empty = { ...s, workflows: { revision: '3', runs: [], omitted_runs: 0 } };
    ui.rerender({ view: { ...view, snapshot: empty, checkpoint: checkpoint(empty, '3') } });
    expect(ui.result.current.snapshot?.runs).toEqual([]);
  } finally { server.client.disconnect(); }
});

it('keeps a manually expanded completed Workflow open while deferred nodes hydrate', async () => {
  const server = new Server(); await server.attached('A'); server.held.add('conversation/checkpointPage');
  try {
    const original = workflowRun(); original.state = { type: 'settled', outcome: 'completed' };
    original.instances = original.instances.slice(0, 3).map(instance => ({ ...instance, state: { type: 'settled', outcome: 'completed' } }));
    const s = snapshot(); s.workflows = { revision: '1', runs: [{ ...original, instances: original.instances.slice(0, 2) }], omitted_runs: 0 };
    const check = checkpoint(s); check.collections = [{ domain: 'workflows.runs', revision: '1', total: '1', loaded: '1', next_offset: null }, { domain: 'workflows.runs.@0.instances', revision: '1', total: '3', loaded: '2', next_offset: '2' }];
    const view = { ...server.client.getSnapshot().views.A, snapshot: s, checkpoint: check };
    function Panel() { const content = useWorkflowContent(server.client, view, true); return <WorkflowRunPanel run={content.snapshot!.runs[0]}/>; }
    const ui = render(<Panel/>), request = await server.waitFor('conversation/checkpointPage', 1);
    const toggle = ui.container.querySelector('[data-workflow-run-id] [data-disclosure-row]')!; fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    await act(async () => server.socket.success(request, { type: 'checkpoint_page', page: { domain: 'workflows.runs', cursor: check.read_cut.revision, read_cut: check.read_cut, total: '1', items: [{ offset: '0', identity: 'run', value: original as never, deferred: false, content: [] }], next_offset: null, collections: [] } }));
    await waitFor(() => expect(toggle.textContent).toContain('Nodes 2'));
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
  } finally { server.client.dispose(); }
});

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

it('keeps workflows.runs.@0.instances unhydrated in closed Chat, suppresses stale currentSnapshot while loading, and restores boundedly when openRunId is scoped', async () => {
  const server = new Server(); await server.attached('A');
  try {
    const original = workflowRun();
    const allInstances = Array.from({ length: 40 }, (_, index) => ({ ...original.instances[1], node: `node-${index}` }));
    const s = snapshot();
    s.workflows = {
      revision: '1',
      runs: [{ ...original, instances: allInstances.slice(0, 32), omitted_instances: 8 }],
      omitted_runs: 0,
    };
    const check = checkpoint(s);
    check.collections = [
      { domain: 'workflows.runs', revision: '1', total: '1', loaded: '1', next_offset: null },
      { domain: 'workflows.runs.@0.instances', revision: '1', total: '40', loaded: '32', next_offset: '32' },
    ];
    const view = { ...server.client.getSnapshot().views.A, snapshot: s, checkpoint: check };
    let instancePageCalls = 0;
    server.held.add('conversation/checkpointPage');
    const deferredJson = JSON.stringify(allInstances[32]);
    const deferredBytes = new TextEncoder().encode(deferredJson);
    server.handlers.set('conversation/checkpointValueRead', () => ({
      type: 'checkpoint_value_chunk',
      chunk: {
        representation: 'canonical_json_v1',
        cut: check.read_cut,
        domain: 'workflows.runs.@0.instances',
        item_offset: 32,
        path: [],
        offset: 0,
        bytes_base64: btoa(String.fromCharCode(...deferredBytes)),
        next_offset: null,
        readable_end: deferredBytes.length,
      },
    }));

    // 1. Default Chat hook with canvas closed must not eagerly pull workflows.runs.@0.instances.
    const chatHook = renderHook(
      ({ view }) => useWorkflowContent(server.client, view, true),
      { initialProps: { view } },
    );
    const runsReq = await server.waitFor('conversation/checkpointPage', 1);
    expect((runsReq.params as { domain: string }).domain).toBe('workflows.runs');
    expect(chatHook.result.current.isCurrentCut).toBe(false);
    expect(chatHook.result.current.currentSnapshot).toBeUndefined();
    await act(async () => server.socket.success(runsReq, {
      type: 'checkpoint_page',
      page: {
        domain: 'workflows.runs',
        cursor: check.read_cut.revision,
        read_cut: check.read_cut,
        total: '1',
        items: [{ offset: '0', identity: 'run', value: s.workflows.runs[0] as never, deferred: false, content: [] }],
        next_offset: null,
        collections: [],
      },
    }));
    expect(chatHook.result.current.isCurrentCut).toBe(true);
    expect(instancePageCalls).toBe(0);

    // 2. While loading a replacement cut, retained prior snapshot NEVER leaks through currentSnapshot.
    const s2 = snapshot();
    const settledRun = { ...original, state: { type: 'settled' as const, outcome: 'completed' as const }, instances: allInstances.slice(0, 32), omitted_instances: 8 };
    s2.workflows = { revision: '2', runs: [settledRun], omitted_runs: 0 };
    const check2 = checkpoint(s2, '2');
    check2.collections = [
      { domain: 'workflows.runs', revision: '2', total: '1', loaded: '1', next_offset: null },
      { domain: 'workflows.runs.@0.instances', revision: '2', total: '40', loaded: '32', next_offset: '32' },
    ];
    chatHook.rerender({ view: { ...view, snapshot: s2, checkpoint: check2 } });
    expect(chatHook.result.current.isCurrentCut).toBe(false);
    expect(chatHook.result.current.currentSnapshot).toBeUndefined();
    const runsReq2 = await server.waitFor('conversation/checkpointPage', 2);
    await act(async () => server.socket.success(runsReq2, {
      type: 'checkpoint_page',
      page: {
        domain: 'workflows.runs',
        cursor: check2.read_cut.revision,
        read_cut: check2.read_cut,
        total: '1',
        items: [{ offset: '0', identity: 'run', value: settledRun as never, deferred: false, content: [] }],
        next_offset: null,
        collections: [],
      },
    }));
    chatHook.unmount();

    // 3. View-scoped open canvas hydration fetches bounded workflows.runs.@0.instances pages for the opened run only.
    const openHook = renderHook(
      ({ view }) => useWorkflowContent(server.client, view, true, undefined, { hydrateRunId: original.id, maxInstancePages: 4 }),
      { initialProps: { view } },
    );
    const instReq = await server.waitFor('conversation/checkpointPage', 3);
    instancePageCalls += 1;
    expect(instReq.params).toMatchObject({ domain: 'workflows.runs.@0.instances', offset: 32 });
    await act(async () => server.socket.success(instReq, {
      type: 'checkpoint_page',
      page: {
        domain: 'workflows.runs.@0.instances',
        cursor: check.read_cut.revision,
        read_cut: check.read_cut,
        total: '40',
        items: allInstances.slice(32).map((inst, idx) => ({
          offset: String(32 + idx),
          identity: `node-${32 + idx}`,
          value: idx === 0 ? null : (inst as never),
          deferred: idx === 0,
          content: [],
        })),
        next_offset: null,
        collections: [],
      },
    }));
    await waitFor(() => expect(openHook.result.current.loading).toBe(false));
    expect(openHook.result.current.error).toBeUndefined();
    expect(openHook.result.current.snapshot?.runs[0].instances).toHaveLength(40);
    expect(openHook.result.current.snapshot?.runs[0].omitted_instances).toBe(0);
    expect(openHook.result.current.isCurrentCut).toBe(true);
    expect(openHook.result.current.currentSnapshot?.runs[0].instances[32].node).toBe('node-32');
    expect(openHook.result.current.currentSnapshot?.runs[0].instances[39].node).toBe('node-39');
    expect(instancePageCalls).toBe(1);
  } finally { server.client.disconnect(); }
});

it('retains display headers after a failed refresh without treating the old cut as a live owner', async () => {
  const server = new Server(); await server.attached('A');
  const original = workflowRun();
  const s = { ...snapshot(), workflows: { revision: '1', runs: [original], omitted_runs: 0 } };
  const view = { ...server.client.getSnapshot().views.A, snapshot: s, checkpoint: checkpoint(s) };
  const hook = renderHook(({ view }) => useWorkflowContent(server.client, view, true), { initialProps: { view } });
  try {
    expect(hook.result.current.isCurrentCut).toBe(true);
    const next = { ...s, workflows: { ...s.workflows, revision: '2' } };
    const check = checkpoint(next, '2');
    check.collections = [{ domain: 'workflows.runs', revision: '2', total: '2', loaded: '1', next_offset: '1' }];
    server.held.add('conversation/checkpointPage');
    server.handlers.set('conversation/checkpointPage', () => { throw new RpcFailure({ code: -32000, message: 'expired', data: { kind: 'read_cut_expired' } }); });
    hook.rerender({ view: { ...view, snapshot: next, checkpoint: check } });
    const request = await server.waitFor('conversation/checkpointPage', 1);
    await act(async () => server.reply(request));
    await waitFor(() => expect(hook.result.current.loading).toBe(false));
    expect(hook.result.current.error).toBeTruthy();
    expect(hook.result.current.snapshot).toBe(s.workflows);
    expect(hook.result.current.currentSnapshot).toBeUndefined();
    expect(hook.result.current.isCurrentCut).toBe(false);
  } finally { hook.unmount(); server.client.disconnect(); }
});
