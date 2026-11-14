import { useContext } from 'react';
import { createHash } from 'node:crypto';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createActor } from 'xstate';
import type {
  CheckpointPage,
  CheckpointReadCut,
  CheckpointValueChunk,
  ContentChunk,
  ContentRef,
  MethodResult,
  RuntimeClientInteractionHeader,
  TraceRecord,
  WorkflowFileDocument,
  WorkflowInstanceView,
  WorkflowLibrary,
  WorkflowRunId,
  WorkflowRunView,
  WorkflowTraceNode,
} from '../../protocol/app-server/v78';
import { METHOD_POLICY } from '../../protocol/app-server/method-policy';
import type { WorkflowSource } from '../../protocol/app-server/workflow-documents';
import type { Observation } from '../src/client/session-lifecycle/port';
import { WorkspaceHostError } from '../src/workspaces/host';
import { RpcFailure } from '../src/client/errors';
import type { TraceCache, TraceFieldReader } from '../src/client/trace';
import { Server } from './fixture';
import { statusSource } from './workflow-fixtures';
import { runBoundsDocument } from './workflow-run-bounds';
import { SubagentContext } from '../src/app/agent/subagent-context';
import { WorkflowRunPanel } from '../src/app/agent/WorkflowRunPanel';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { boundedDocument } from '../src/app/workflows/document/commands';
import { nodeField, setField } from '../src/app/workflows/document/fields';
import { workflowSurfaceMachine } from '../src/app/workflows/machines/surface';
import WorkflowEditor from '../src/app/workflows/ui/editor/WorkflowEditor';
import { organizeDocument } from '../src/app/workflows/ui/editor/layout';
import { ConversationRunCanvasHost } from '../src/app/workflows/run-canvas/ConversationRunCanvasHost';
import { WorkflowRunCanvasContext, type WorkflowRunCanvasTarget } from '../src/app/workflows/run-canvas/context';
import type {
  WorkflowRunSnapshotChunk,
  WorkflowRunSnapshotHeader,
  WorkflowRunSnapshotTransport,
} from '../src/app/workflows/run-canvas/contract';
import { WorkflowRunCanvas } from '../src/app/workflows/run-canvas/WorkflowRunCanvas';
import { visitIdentityKey } from '../src/app/workflows/run-canvas/identity';
import { viewId } from '../src/app/workflows/ui/editor/selection';
import { workflowEditorMachine } from '../src/app/workflows/machines/editor';
import { USER_SCOPE, type WorkflowScopePort } from '../src/app/workflows/port';

afterEach(() => {
  cleanup();
});

function baseRunId(invocation = '1', conversationId = 'conv-ui'): WorkflowRunId {
  return { conversation_id: conversationId, origin: { type: 'tool', attempt_id: 'attempt-ui' }, invocation };
}

function makeNestedDoc(): WorkflowFileDocument {
  const base = statusSource().document!;
  const doc: WorkflowFileDocument = structuredClone(base);
  doc.block.nodes = {
    workflow_start: { type: 'start' },
    outer: {
      type: 'loop',
      loop_count: 3,
      break_condition: null,
      body: {
        nodes: {
          loop_start: { type: 'start' },
          worker: { type: 'template', variables: {}, template: 'run recorded visit' },
          childStep: {
            type: 'agent',
            profile: 'reviewer',
            task: [{ type: 'literal', value: 'check' }],
            output: { type: 'text' },
          },
        },
        edges: [
          { from: 'loop_start', port: 'next', to: 'worker' },
          { from: 'worker', port: 'next', to: 'childStep' },
        ],
      },
      loop_variables: [],
    },
  };
  doc.block.nodes.done = { type: 'return', outputs: [] };
  doc.block.edges = [{ from: 'workflow_start', port: 'next', to: 'outer' }, { from: 'outer', to: 'done' }];
  return doc;
}

function makeTransport(
  doc: WorkflowFileDocument,
  runId: WorkflowRunId,
  options?: {
    hasSourcePresentation?: boolean;
    workflowId?: string;
    corruptSha?: boolean;
    unavailable?: boolean;
    error?: unknown;
  },
): WorkflowRunSnapshotTransport {
  const bytes = Buffer.from(JSON.stringify(doc), 'utf8');
  const sha = options?.corruptSha
    ? '0'.repeat(64)
    : createHash('sha256').update(bytes).digest('hex');
  const header: WorkflowRunSnapshotHeader = {
    run_id: runId,
    workflow_id: options?.workflowId ?? 'nested_flow',
    program_digest: 'sha256:prog-1',
    source_revision: 'rev-frozen-1',
    snapshot_digest: 'sha256:snap-1',
    has_source_presentation: options?.hasSourcePresentation ?? true,
    byte_length: bytes.length,
    sha256: sha,
  };
  return {
    async readHeader() {
      if (options?.error) throw options.error;
      if (options?.unavailable) return null;
      return header;
    },
    async readChunk(offset, limit): Promise<WorkflowRunSnapshotChunk | null> {
      const slice = bytes.subarray(offset, offset + limit);
      const next = offset + slice.length;
      return {
        offset,
        data: slice.toString('base64'),
        next_offset: next < bytes.length ? next : null,
      };
    },
  };
}

function makeVisitInstance(
  runId: WorkflowRunId,
  iteration: number,
  visit = 0,
  overrides?: Partial<WorkflowInstanceView>,
): WorkflowInstanceView {
  return {
    block: { run: runId, definition: { workflow_id: 'nested_flow', blocks: ['outer', 'body'] }, invocations: [0, iteration] },
    node: 'worker',
    visit,
    kind: 'template',
    state: iteration === 1 ? { type: 'settled', outcome: 'completed' } : { type: 'waiting', reason: 'review' },
    iteration,
    iterations_max: 3,
    child_agent_id: null,
    child: null,
    checks_passed: iteration === 1 ? true : null,
    loop_exit: null,
    ...overrides,
  };
}

const nodeBody = (domain: string) => JSON.stringify({ loadedFor: domain, iterationPayload: 'lazy-value' });
const nodeBodyBytes = (domain: string) => Buffer.byteLength(JSON.stringify(nodeBody(domain)));

function makeTraceNode(
  id: string,
  runId: WorkflowRunId,
  iteration: number,
  visit: number,
  state: WorkflowTraceNode['state'],
  node = 'worker',
  blocks: string[] = ['outer', 'body'],
): WorkflowTraceNode {
  return {
    id,
    instance: {
      block: { run: runId, definition: { workflow_id: 'nested_flow', blocks }, invocations: blocks.length ? [0, iteration] : [0] },
      node,
      visit,
    },
    kind: node === 'loop_start' || node === 'workflow_start' ? 'start' : 'template',
    state,
    timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '42' },
    input: { type: 'value', field: 'arguments', domain: `in-${id}`, item_offset: 0, path: [], encoded_bytes: String(nodeBodyBytes(`in-${id}`)) },
    output: { type: 'value', field: 'result', domain: `out-${id}`, item_offset: 0, path: [], encoded_bytes: String(nodeBodyBytes(`out-${id}`)) },
    input_preview: JSON.stringify({ iteration, prompt: `iter-${iteration}` }),
    output_preview: null,
    input_omitted: false,
    output_omitted: false,
  };
}

describe('WorkflowRunCanvas UI and integration', () => {
  it('renders nested run canvas with arbitrary Start ID, preserves selected visit, lazily reads fields, and avoids relayout/focus theft on live status updates', async () => {
    const doc = makeNestedDoc();
    const runId = baseRunId('1');
    const cut: CheckpointReadCut = { conversation_id: 'conv-ui', revision: '1', through: '1', domain_revisions: {}, collection_cuts: {} };
    const valueSpy = vi.fn(async (_cut: CheckpointReadCut, source: Parameters<TraceFieldReader['value']>[1]): Promise<CheckpointValueChunk> => {
      const bytes = Buffer.from(JSON.stringify(nodeBody(source.domain)), 'utf8');
      return {
        representation: 'canonical_json_v1',
        cut,
        domain: source.domain,
        item_offset: source.item_offset,
        path: [...source.path],
        offset: 0,
        bytes_base64: bytes.toString('base64'),
        next_offset: null,
        readable_end: bytes.length,
      };
    });
    const reader: TraceFieldReader = {
      value: valueSpy,
      content: vi.fn(async (_cut: CheckpointReadCut, reference: ContentRef): Promise<ContentChunk> => ({
        content_ref: reference,
        offset: 0,
        data_base64: '',
        end_offset: 0,
        readable_end: 0,
        next_offset: null,
        complete: true,
      })),
      page: vi.fn(),
    };
    const startVisit = makeTraceNode('wn-start', runId, 0, 0, { type: 'settled', outcome: 'completed' }, 'workflow_start', []);
    const visit1 = makeTraceNode('wn-1', runId, 1, 0, { type: 'settled', outcome: 'completed' });
    const visit2 = makeTraceNode('wn-2', runId, 2, 0, { type: 'waiting', reason: 'review' });

    const target: WorkflowRunCanvasTarget = {
      runId,
      workflowId: 'nested_flow',
      conversationId: 'conv-ui',
      selectedVisit: visit2.instance,
      traceNodes: [startVisit, visit1, visit2],
      traceReader: reader,
      traceCut: cut,
      sourceMode: 'live',
      transport: makeTransport(doc, runId, { hasSourcePresentation: false }),
    };
    const initialRun: WorkflowRunView = {
      id: runId,
      workflow_id: 'nested_flow',
      program_digest: 'sha256:prog-1',
      resource_revision: '1',
      tool_call_id: 'call-1',
      state: { type: 'waiting', reason: 'review' },
      instances: [makeVisitInstance(runId, 1), makeVisitInstance(runId, 2)],
      omitted_instances: 0,
      steps_consumed: 2,
      steps_max: 10,
      agents_consumed: 0,
      model_calls_consumed: 0,
    };
    const organizeFn = vi.fn(async (input: WorkflowFileDocument) => input);
    const pendingInteractions: RuntimeClientInteractionHeader[] = [];

    const ui = render(
      <WorkflowRunCanvas
        target={target}
        liveRun={initialRun}
        pendingInteractions={pendingInteractions}
        onClose={() => {}}
        organizeFn={organizeFn}
      />,
    );

    await waitFor(() => expect(organizeFn).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByTestId('rf__wrapper')).toBeTruthy());

    // Arbitrary executable Start (`workflow_start`) received its completed run badge rather than being dropped.
    const startId = viewId([], 'start');
    const startCard = document.body.querySelector(`[data-id="${CSS.escape(startId)}"] [data-run-node-status="completed"]`);
    expect(startCard).toBeTruthy();

    // Exact clicked visit (iteration 2 waiting) is selected, not hidden by earlier completed visit 1.
    const visitButtons = document.body.querySelectorAll('[data-run-visit-key]');
    expect(visitButtons).toHaveLength(2);
    const visit2Key = visitIdentityKey(visit2.instance);
    expect(document.body.querySelector(`[data-run-visit-key="${CSS.escape(visit2Key)}"]`)?.getAttribute('aria-selected')).toBe('true');

    // Breadcrumb shows Root and containing Loop block.
    const breadcrumbBar = screen.getByRole('navigation', { name: 'Block path' });
    expect(breadcrumbBar.textContent).toContain('Root');
    expect(breadcrumbBar.textContent).toContain('outer');

    // No eager value reads occurred while rendering overview.
    expect(valueSpy).not.toHaveBeenCalled();

    // Switching to Output tab triggers a single lazy read for visit 2's output only.
    const outputTab = screen.getByRole('tab', { name: 'Output' });
    outputTab.focus();
    fireEvent.click(outputTab);
    await waitFor(() => expect(valueSpy).toHaveBeenCalledTimes(1));
    expect(valueSpy.mock.calls[0]?.[1].domain).toBe('out-wn-2');
    await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('lazy-value'));

    // Live status update finishes iteration 2: organizeFn must NOT re-run and focus must remain on Output tab.
    const updatedRun: WorkflowRunView = {
      ...initialRun,
      state: { type: 'settled', outcome: 'completed' },
      instances: [
        makeVisitInstance(runId, 1),
        { ...makeVisitInstance(runId, 2), state: { type: 'settled', outcome: 'completed' } },
      ],
    };
    ui.rerender(
      <WorkflowRunCanvas
        target={target}
        liveRun={updatedRun}
        pendingInteractions={pendingInteractions}
        onClose={() => {}}
        organizeFn={organizeFn}
      />,
    );
    expect(organizeFn).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(outputTab);

    // Disconnecting/removing the authoritative liveRun never leaves Running/Waiting stuck; drops to non-live state.
    ui.rerender(
      <WorkflowRunCanvas
        target={target}
        liveRun={undefined}
        pendingInteractions={pendingInteractions}
        onClose={() => {}}
        organizeFn={organizeFn}
      />,
    );
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('disconnected'));
    expect(document.body.querySelector('[data-run-node-status="running"]')).toBeNull();
    expect(document.body.querySelector('[data-run-node-status="waiting"]')).toBeNull();
  });

  it('shows unavailable/corrupt/expired/denied states safely while preserving authorized trace inspection and child link', async () => {
    const doc = makeNestedDoc();
    const runId = baseRunId('2');
    const openWorkflowSpy = vi.fn(async () => {});
    const traceChild: WorkflowTraceNode = { ...makeTraceNode('wn-child', runId, 1, 0, { type: 'settled', outcome: 'completed' }, 'childStep'), kind: null, child: 'act-1', child_agent_id: 'agent-child-1' };

    const ui = render(
      <SubagentContext.Provider value={{
        client: new Server().client,
        agents: [],
        metrics: {},
        metricErrors: {},
        opened: [],
        open: () => {},
        close: () => {},
        openWorkflow: openWorkflowSpy,
      }}>
        <WorkflowRunCanvas
          target={{
            runId,
            workflowId: 'nested_flow',
            conversationId: 'conv-ui',
            selectedVisit: traceChild.instance,
            traceNodes: [traceChild],
            sourceMode: 'history',
            transport: makeTransport(doc, runId, { unavailable: true }),
          }}
          onClose={() => {}}
        />
      </SubagentContext.Provider>,
    );

    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('snapshot is unavailable'));
    // Authorized visit inspector and child conversation button remain functional even without snapshot graph.
    fireEvent.click(screen.getByRole('button', { name: 'Open subagent conversation' }));
    expect(openWorkflowSpy).toHaveBeenCalledTimes(1);

    // Corrupt snapshot refuses graph rendering.
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          runId,
          workflowId: 'nested_flow',
          conversationId: 'conv-ui',
          sourceMode: 'history',
          transport: makeTransport(doc, runId, { corruptSha: true }),
        }}
        onClose={() => {}}
      />,
    );
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('integrity verification'));
    expect(document.body.querySelector('[data-testid="rf__wrapper"]')).toBeNull();

    // Expired cut error classification renders safe expired status notice.
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          runId,
          workflowId: 'nested_flow',
          conversationId: 'conv-ui',
          sourceMode: 'history',
          transport: makeTransport(doc, runId, { error: new RpcFailure({ code: -32000, message: 'read cut expired', data: { kind: 'read_cut_expired' } }) }),
        }}
        onClose={() => {}}
      />,
    );
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('expired'));

    // Access denied classification.
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          runId,
          workflowId: 'nested_flow',
          conversationId: 'conv-ui',
          sourceMode: 'history',
          transport: makeTransport(doc, runId, { error: new RpcFailure({ code: -32000, message: 'cross-conversation access denied' }) }),
        }}
        onClose={() => {}}
      />,
    );
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('permission'));
  });

  it('synchronously fences stale renders, rejects retained callbacks, and closes on conversation switch or ownership revocation', async () => {
    const doc = makeNestedDoc();
    const runA = baseRunId('59', 'conv-1');
    const runB = baseRunId('58', 'conv-1');

    let resolveRunAHeader!: (value: WorkflowRunSnapshotHeader | null) => void;
    const staleTransport: WorkflowRunSnapshotTransport = {
      readHeader: () => new Promise(resolve => { resolveRunAHeader = resolve; }),
      readChunk: async () => null,
    };
    const fastTransport = makeTransport(doc, runB, { hasSourcePresentation: true });

    let latestOpen!: (target: WorkflowRunCanvasTarget) => void;
    let capturedConv1Open!: (target: WorkflowRunCanvasTarget) => void;
    function TriggerHarness({ captureOld }: { captureOld?: boolean }) {
      const ctx = WorkflowRunCanvasContext;
      return (
        <ctx.Consumer>
          {value => {
            if (value) {
              latestOpen = value.openRunCanvas;
              if (captureOld) capturedConv1Open = value.openRunCanvas;
            }
            return <button type="button">trigger</button>;
          }}
        </ctx.Consumer>
      );
    }

    const obs1: Observation = { generation: 1, nodeId: 'node-1', intentRevision: 1, target: { session_id: 'sess-1', conversation_id: 'conv-1', runtime_incarnation: '1', attachment_id: 'att-1' } };
    const host = render(
      <ConversationRunCanvasHost sessionId="sess-1" conversationId="conv-1" attachmentObservation={obs1} ownerCurrent>
        <TriggerHarness captureOld />
      </ConversationRunCanvasHost>,
    );

    act(() => capturedConv1Open({
      runId: runA,
      workflowId: 'flow_a',
      conversationId: 'conv-1',
      sourceMode: 'live',
      transport: staleTransport,
    }));
    await waitFor(() => expect(screen.getByRole('dialog')).toBeTruthy());

    // Switch target to run B before run A's header arrives.
    act(() => latestOpen({
      runId: runB,
      workflowId: 'flow_b',
      conversationId: 'conv-1',
      sourceMode: 'live',
      transport: fastTransport,
    }));
    await waitFor(() => expect(screen.getByText('Workflow run · flow_b')).toBeTruthy());

    // Resolve stale run A header late: run B stays mounted cleanly.
    await act(async () => {
      resolveRunAHeader({
        run_id: runA,
        workflow_id: 'flow_a',
        program_digest: 'sha256:old',
        source_revision: 'old',
        snapshot_digest: 'sha256:old',
        has_source_presentation: true,
        byte_length: 10,
        sha256: 'a'.repeat(64),
      });
    });
    expect(screen.queryByText('Workflow run · flow_a')).toBeNull();
    expect(screen.getByText('Workflow run · flow_b')).toBeTruthy();

    // Switching conversation synchronously unmounts the dialog (zero stale frames) AND rejects the retained conv-1 callback!
    host.rerender(
      <ConversationRunCanvasHost sessionId="sess-1" conversationId="conv-2" ownerCurrent>
        <TriggerHarness />
      </ConversationRunCanvasHost>,
    );
    expect(screen.queryByRole('dialog')).toBeNull();

    const rejectedSpy = vi.fn(async () => null);
    act(() => capturedConv1Open({
      runId: runA,
      workflowId: 'flow_a',
      conversationId: 'conv-1',
      sourceMode: 'live',
      transport: { readHeader: rejectedSpy, readChunk: async () => null },
    }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(rejectedSpy).not.toHaveBeenCalled();

    // Ownership revocation (`ownerCurrent={false}`) synchronously blocks new opens as well.
    let revokedOpen!: (target: WorkflowRunCanvasTarget) => void;
    host.rerender(
      <ConversationRunCanvasHost sessionId="sess-1" conversationId="conv-2" ownerCurrent={false}>
        <WorkflowRunCanvasContext.Consumer>
          {value => { if (value) revokedOpen = value.openRunCanvas; return null; }}
        </WorkflowRunCanvasContext.Consumer>
      </ConversationRunCanvasHost>,
    );
    act(() => revokedOpen({
      runId: runB,
      workflowId: 'flow_b',
      conversationId: 'conv-2',
      sourceMode: 'live',
      transport: { readHeader: rejectedSpy, readChunk: async () => null },
    }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(rejectedSpy).not.toHaveBeenCalled();
  });

  it('supports bounded incremental trace paging and preserves selected visits beyond the initial loaded prefix', async () => {
    const doc = makeNestedDoc();
    const runId = baseRunId('57');
    const cut: CheckpointReadCut = { conversation_id: 'conv-ui', revision: 'cut-40', through: '40', domain_revisions: {}, collection_cuts: {} };
    const total = 45;
    const allNodes: WorkflowTraceNode[] = Array.from({ length: total }, (_, idx) =>
      makeTraceNode(`wn-p-${idx}`, runId, idx + 1, 0, { type: 'settled', outcome: 'completed' }),
    );
    const traceRoot: TraceRecord = {
      id: 'trace:40',
      position: '40',
      location: { attempt_id: 'a1', step_id: 's1' },
      kind: 'workflow',
      state: 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:02Z', duration_ms: '2000' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: 'trace.records.40.workflow.nodes', total: String(total), loaded: '0', next_offset: '0', revision: 'cut-40' }],
      native_id: null,
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    };
    const pageSpy = vi.fn(async (_cut: CheckpointReadCut, domain: string, offset: number): Promise<CheckpointPage> => {
      const slice = allNodes.slice(offset, offset + 32);
      const next = offset + slice.length;
      return {
        cursor: 'cut-40',
        read_cut: cut,
        domain,
        total: String(total),
        collections: [],
        next_offset: next < total ? String(next) : null,
        items: slice.map((node, i) => ({
          offset: String(offset + i),
          identity: node.id,
          value: node,
          content: [],
          deferred: false,
          body_complete: true,
        })),
      };
    });
    const reader: TraceFieldReader = { value: vi.fn(), content: vi.fn(), page: pageSpy };

    render(
      <WorkflowRunCanvas
        target={{
          runId,
          workflowId: 'nested_flow',
          conversationId: 'conv-ui',
          selectedVisit: allNodes[44]!.instance,
          selectedTraceNode: allNodes[44],
          traceRoot,
          traceReader: reader,
          traceCut: cut,
          sourceMode: 'history',
          transport: makeTransport(doc, runId),
        }}
        onClose={() => {}}
      />,
    );

    // Graph renders immediately and pageSpy ran boundedly; selected visit #44 (beyond page 1 if total > 64) is preserved!
    await waitFor(() => expect(screen.getByTestId('rf__wrapper')).toBeTruthy());
    const visit44Key = visitIdentityKey(allNodes[44]!.instance);
    await waitFor(() => expect(document.querySelector(`[data-run-visit-key="${CSS.escape(visit44Key)}"]`)?.getAttribute('aria-selected')).toBe('true'));
    expect(pageSpy.mock.calls.length).toBeLessThanOrEqual(2);
  });

  it('wires Chat and Trajectory actions with exact run, definition and visit identities', async () => {
    const runId = baseRunId('56', 'conv-chat');
    const openSpy = vi.fn();
    const inst = makeVisitInstance(runId, 2, 1);
    const run: WorkflowRunView = {
      id: runId,
      workflow_id: 'implement_and_review',
      program_digest: 'sha256:p1',
      resource_revision: '1',
      state: { type: 'running' },
      instances: [inst],
      omitted_instances: 0,
      steps_consumed: 1,
      steps_max: 5,
      agents_consumed: 0,
      model_calls_consumed: 0,
    };

    render(
      <WorkflowRunCanvasContext.Provider value={{ openRunCanvas: openSpy }}>
        <WorkflowRunPanel run={run} />
      </WorkflowRunCanvasContext.Provider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'View run graph' }));
    expect(openSpy).toHaveBeenCalledTimes(1);
    expect(openSpy.mock.calls[0]?.[0].runId).toEqual(runId);

    fireEvent.click(screen.getByRole('button', { name: 'Inspect worker on run graph' }));
    expect(openSpy).toHaveBeenCalledTimes(2);
    expect(openSpy.mock.calls[1]?.[0].selectedVisit).toEqual({
      block: inst.block,
      node: inst.node,
      visit: inst.visit,
    });

    // Trajectory double-click uses exact `definition.workflow_id` (not serialized `native_id`) and preserves `selectedTraceNode`.
    openSpy.mockClear();
    const cut: CheckpointReadCut = { conversation_id: 'conv-chat', revision: 'c1', through: '10', domain_revisions: {}, collection_cuts: {} };
    const traceRoot: TraceRecord = {
      id: 'trace:10',
      position: '10',
      location: { attempt_id: 'a1', step_id: 's1' },
      kind: 'workflow',
      state: 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:02Z', duration_ms: '2000' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: 'trace.records.10.workflow.nodes', total: '1', loaded: '1', next_offset: null, revision: 'r1' }],
      native_id: '{"serialized":"run-json"}',
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    };
    const traceNode = makeTraceNode('wn-tr-1', runId, 2, 1, { type: 'settled', outcome: 'completed' });
    const traceReader: TraceFieldReader = {
      value: vi.fn(),
      content: vi.fn(),
      page: vi.fn(async (): Promise<CheckpointPage> => ({
        cursor: 'r1',
        domain: 'trace.records.10.workflow.nodes',
        read_cut: cut,
        total: '1',
        collections: [],
        next_offset: null,
        items: [{ offset: '0', identity: traceNode.id, value: traceNode as unknown as Record<string, unknown>, content: [], deferred: false, body_complete: true }],
      })),
    };
    const cache: TraceCache = {
      epoch: 1, page: { records: [traceRoot], next_cursor: null, read_cut: cut },
      readCuts: { 'trace:10': cut }, fields: traceReader, details: {},
    };

    const traj = render(
      <WorkflowRunCanvasContext.Provider value={{ openRunCanvas: openSpy }}>
        <Trajectory cache={cache} loadEarlier={() => {}} onSelect={() => {}} onLoadDetail={() => {}} />
      </WorkflowRunCanvasContext.Provider>,
    );

    fireEvent.click(traj.container.querySelector('[data-trace-id="trace:10"] button[aria-expanded]')!);
    await waitFor(() => expect(traj.container.querySelector('[data-workflow-node="wn-tr-1"]')).toBeTruthy());
    fireEvent.doubleClick(traj.container.querySelector('[data-workflow-node="wn-tr-1"]')!);
    expect(openSpy).toHaveBeenCalledTimes(1);
    expect(openSpy.mock.calls[0]?.[0].workflowId).toBe('nested_flow');
    expect(openSpy.mock.calls[0]?.[0].selectedVisit).toEqual(traceNode.instance);
    expect(openSpy.mock.calls[0]?.[0].selectedTraceNode).toEqual(traceNode);


  });

  const cutFor = (conversationId = 'conv-ui'): CheckpointReadCut => ({
    conversation_id: conversationId,
    revision: '1',
    through: '1',
    domain_revisions: {},
    collection_cuts: {},
  });

  it('interpolates TrajectoryRow workflow inspect label cleanly without raw {p0} placeholder', async () => {
    const run = baseRunId('55', 'conv-interp');
    const traceNode: WorkflowTraceNode = {
      id: 'wn-interp',
      instance: {
        block: { run, definition: { workflow_id: 'nested_flow', blocks: ['outer', 'body'] }, invocations: [0, 1] },
        node: 'worker',
        visit: 0,
      },
      kind: 'tool',
      state: { type: 'settled', outcome: 'completed' },
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '10' },
      input: null,
      output: null,
      input_preview: null,
      output_preview: null,
      input_omitted: false,
      output_omitted: false,
    };
    const root: TraceRecord = {
      id: 'trace:interp',
      position: '1',
      location: { attempt_id: 'attempt-1', step_id: 'step-1' },
      kind: 'workflow',
      state: 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '10' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: 'trace.records.interp.workflow.nodes', revision: '1', total: '1', loaded: '0', next_offset: '0' }],
      native_id: JSON.stringify(run),
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    };
    const cut = cutFor('conv-interp');
    const cache: TraceCache = {
      epoch: 1,
      page: { read_cut: cut, records: [root], next_cursor: null },
      readCuts: { [root.id]: cut },
      details: {},
      fields: {
        page: vi.fn(async (): Promise<CheckpointPage> => ({
          cursor: '1',
          domain: 'trace.records.interp.workflow.nodes',
          read_cut: cut,
          total: '1',
          collections: [],
          next_offset: null,
          items: [{ offset: '0', identity: traceNode.id, value: traceNode as unknown as Record<string, unknown>, content: [], deferred: false, body_complete: true }],
        })),
        value: vi.fn(),
        content: vi.fn(),
      },
    };
    const traj = render(
      <WorkflowRunCanvasContext.Provider value={{ openRunCanvas: vi.fn() }}>
        <Trajectory cache={cache} onSelect={vi.fn()} onLoadDetail={vi.fn()} loadEarlier={vi.fn()} />
      </WorkflowRunCanvasContext.Provider>,
    );
    fireEvent.click(traj.container.querySelector('[data-trace-id="trace:interp"] button[aria-expanded]')!);
    const inspectBtn = await screen.findByRole('button', { name: 'Inspect worker on run graph' });
    expect(inspectBtn).toBeTruthy();
    expect(inspectBtn!.getAttribute('aria-label')).toBe('Inspect worker on run graph');
    expect(inspectBtn!.getAttribute('aria-label')).not.toContain('{p0}');
  });

  it('keeps React Flow mounted, skips relayout/snapshot reload, and preserves focus when paging trace visits', async () => {
    const doc = makeNestedDoc();
    const run = baseRunId('54', 'conv-page');
    const cut = cutFor('conv-page');
    const transport = makeTransport(doc, run);
    const readHeaderSpy = vi.spyOn(transport, 'readHeader');
    const organizeSpy = vi.fn(async (d: WorkflowFileDocument) => d);
    const visits: WorkflowTraceNode[] = Array.from({ length: 70 }, (_, idx) => ({
      id: `wn-${idx}`,
      instance: { block: { run, definition: { workflow_id: 'nested_flow', blocks: ['outer', 'body'] }, invocations: [0, idx + 1] }, node: 'worker', visit: 0 },
      kind: 'template',
      state: { type: 'settled', outcome: 'completed' },
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '5' },
      input: null,
      output: null,
      input_preview: null,
      output_preview: null,
      input_omitted: false,
      output_omitted: false,
    }));
    const traceDomain = 'trace.records.p70.workflow.nodes';
    const reader: TraceFieldReader = {
      value: vi.fn(),
      content: vi.fn(),
      page: vi.fn(async (_cut, domain, offset) => {
        const slice = visits.slice(offset, offset + 32);
        const next = offset + slice.length;
        return {
          domain,
          cursor: '1',
          read_cut: cut,
          total: '70',
          collections: [],
          next_offset: next < 70 ? String(next) : null,
          items: slice.map((node, i) => ({
            offset: String(offset + i),
            identity: node.id,
            value: node as unknown as Record<string, unknown>,
            content: [],
            deferred: false,
            body_complete: true,
          })),
        } satisfies CheckpointPage;
      }),
    };
    const traceRoot: TraceRecord = {
      id: 'trace:p70',
      position: '1',
      location: { attempt_id: 'a', step_id: 's' },
      kind: 'workflow',
      state: 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '50' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: traceDomain, revision: '1', total: '70', loaded: '0', next_offset: '0' }],
      native_id: JSON.stringify(run),
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    };

    render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-page',
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          selectedVisit: visits[0].instance,
          traceRoot,
          traceCut: cut,
          traceReader: reader,
          transport,
        }}
        onClose={vi.fn()}
        organizeFn={organizeSpy}
      />,
    );

    const loadMoreBtn = await screen.findByRole('button', { name: 'Load more trace visits' });
    const flowBefore = document.querySelector('.react-flow');
    expect(flowBefore).toBeTruthy();
    expect(readHeaderSpy).toHaveBeenCalledTimes(1);
    const organizeCallsBefore = organizeSpy.mock.calls.length;

    loadMoreBtn.focus();
    fireEvent.click(loadMoreBtn);

    await waitFor(() => expect(screen.queryByRole('button', { name: 'Load more trace visits' })).toBeNull());
    expect(document.querySelector('.react-flow')).toBe(flowBefore);
    expect(readHeaderSpy).toHaveBeenCalledTimes(1);
    expect(organizeSpy).toHaveBeenCalledTimes(organizeCallsBefore);
    expect(screen.getByRole('dialog').contains(document.activeElement)).toBe(true);
  });

  it('enforces historical unsettled -> incomplete, resolves unexpanded Trajectory liveRun, handles child nav, fences VisitFieldPanel cut/reader switches, and delivers live trace output without reload', async () => {
    const doc = makeNestedDoc();
    doc.block.nodes.outer = {
      type: 'loop',
      loop_count: 2,
      break_condition: null,
      body: {
        nodes: {
          loop_start: { type: 'start' },
          worker: { type: 'agent', profile: 'reviewer', task: [{ type: 'literal', value: 'Review' }], output: { type: 'text' } },
        },
        edges: [{ from: 'loop_start', port: 'next', to: 'worker' }],
      },
    };
    const run = baseRunId('53', 'conv-hist');
    const cut1 = cutFor('conv-hist');
    const cut2: CheckpointReadCut = { ...cut1, revision: '2', through: '2' };
    const transport = makeTransport(doc, run);
    const organizeSpy = vi.fn(async (d: WorkflowFileDocument) => d);
    const openWorkflowSpy = vi.fn(async () => {});

    const visit1Cut1: WorkflowTraceNode = {
      id: 'wn-ag-1',
      instance: { block: { run, definition: { workflow_id: 'nested_flow', blocks: ['outer', 'body'] }, invocations: [0, 1] }, node: 'worker', visit: 0 },
      kind: 'agent',
      state: { type: 'running' },
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: null, duration_ms: null },
      input: null,
      output: null,
      input_preview: null,
      output_preview: null,
      input_omitted: false,
      output_omitted: false,
    };
    const visit1Cut2: WorkflowTraceNode = {
      ...visit1Cut1,
      state: { type: 'settled', outcome: 'completed' },
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:02Z', duration_ms: '1200' },
      output: { type: 'value', field: 'result', domain: 'out-cut2', item_offset: 0, path: [], encoded_bytes: String(new TextEncoder().encode(JSON.stringify('{"ok":"arrived"}')).length) },
      output_preview: '{"ok":"arrived"}',
    };
    const workflowDetailPage = (readCut: CheckpointReadCut, rootRecord: TraceRecord, targetRun: WorkflowRunId, wfId: string): CheckpointPage => ({
      domain: 'trace.records.101.detail',
      cursor: readCut.revision,
      read_cut: readCut,
      total: '1',
      collections: [],
      next_offset: null,
      items: [{
        offset: '0',
        identity: rootRecord.id,
        value: {
          id: rootRecord.id,
          kind: 'workflow',
          record: rootRecord,
          content: { type: 'workflow', run_id: targetRun, workflow_id: wfId },
        } as unknown as Record<string, unknown>,
        content: [],
        deferred: true,
        body_complete: false,
      }],
    });
    const traceRoot: TraceRecord = {
      id: 'trace:101',
      position: '101',
      location: { attempt_id: 'a', step_id: 's' },
      kind: 'workflow',
      state: 'running',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: null, duration_ms: null },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: 'trace.records.101.workflow.nodes', revision: '1', total: '1', loaded: '0', next_offset: '0' }],
      native_id: null,
      originating_tool_call_id: null,
      has_detail: true,
      truncated: false,
    };
    const readerCut1: TraceFieldReader = {
      page: vi.fn(async (_cut, domain) => {
        if (domain === 'trace.records.101.detail') return workflowDetailPage(cut1, traceRoot, run, 'nested_flow');
        return {
          domain: 'trace.records.101.workflow.nodes',
          cursor: '1',
          read_cut: cut1,
          total: '1',
          collections: [],
          next_offset: null,
          items: [{ offset: '0', identity: visit1Cut1.id, value: visit1Cut1 as unknown as Record<string, unknown>, content: [], deferred: false, body_complete: true }],
        };
      }),
      value: vi.fn(async () => { throw new RpcFailure({ code: -32000, message: 'cut1 value error' }); }),
      content: vi.fn(),
    };
    const readerCut2: TraceFieldReader = {
      page: vi.fn(async (_cut, domain) => {
        if (domain === 'trace.records.101.detail') return workflowDetailPage(cut2, traceRoot, run, 'nested_flow');
        return {
          domain: 'trace.records.101.workflow.nodes',
          cursor: '2',
          read_cut: cut2,
          total: '1',
          collections: [],
          next_offset: null,
          items: [{ offset: '0', identity: visit1Cut2.id, value: visit1Cut2 as unknown as Record<string, unknown>, content: [], deferred: false, body_complete: true }],
        };
      }),
      value: vi.fn(async () => {
        const bytes = new TextEncoder().encode(JSON.stringify('{"ok":"arrived"}'));
        let bin = '';
        for (const b of bytes) bin += String.fromCharCode(b);
        return {
          representation: 'canonical_json_v1' as const,
          cut: cut2,
          domain: 'out-cut2',
          item_offset: 0,
          path: [],
          offset: 0,
          bytes_base64: btoa(bin),
          next_offset: null,
          readable_end: bytes.length,
        };
      }),
      content: vi.fn(),
    };

    // Case A: History sourceMode with unsettled trace state (no live owner) must show Incomplete!
    const histView = render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-hist',
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          selectedVisit: visit1Cut1.instance,
          traceNodes: [visit1Cut1],
          traceCut: cut1,
          traceReader: readerCut1,
          transport,
        }}
        onClose={vi.fn()}
        organizeFn={organizeSpy}
      />,
    );
    await waitFor(() => expect(document.querySelectorAll('[data-run-node-status="incomplete"]').length).toBeGreaterThan(0));
    expect(document.querySelectorAll('[data-run-node-status="running"]')).toHaveLength(0);
    histView.unmount();

    // Case B: Unexpanded Trajectory root (no target.runId) resolves runId & matches liveRuns,
    // opens durable child via scope.openWorkflow, and receives cut2 output arrival without reload.
    const liveRunWithChild: WorkflowRunView = {
      id: run,
      workflow_id: 'nested_flow',
      program_digest: 'sha256:d',
      resource_revision: '1',
      state: { type: 'running' },
      omitted_instances: 0,
      steps_consumed: 1,
      steps_max: 8,
      agents_consumed: 1,
      model_calls_consumed: 0,
      instances: [{
        block: visit1Cut1.instance.block,
        node: 'worker',
        visit: 0,
        kind: 'agent',
        state: { type: 'waiting', reason: 'agent' },
        iteration: 1,
        iterations_max: 2,
        loop_exit: null,
        checks_passed: null,
        child: 'sub-99',
        child_agent_id: 'agent-99',
      }],
    };
    const subScope = {
      client: new Server().client,
      agents: [],
      metrics: {},
      metricErrors: {},
      opened: [],
      openWorkflow: openWorkflowSpy,
      close: vi.fn(),
      open: vi.fn(),
    };
    const ui = render(
      <SubagentContext.Provider value={subScope}>
        <WorkflowRunCanvas
          target={{
            conversationId: 'conv-hist',
            workflowId: 'nested_flow',
            sourceMode: 'live',
            traceRoot,
            traceCut: cut1,
            traceReader: readerCut1,
            transport,
          }}
          liveRuns={[liveRunWithChild]}
          onClose={vi.fn()}
          organizeFn={organizeSpy}
        />
      </SubagentContext.Provider>,
    );

    const agentViewId = viewId([{ node: 'outer' }], 'node', 'worker');
    await waitFor(() => expect(document.querySelector(`.react-flow__node[data-id="${agentViewId}"]`)).not.toBeNull());
    fireEvent.click(document.querySelector(`.react-flow__node[data-id="${agentViewId}"]`)!);
    const openChildBtn = await screen.findByRole('button', { name: 'Open subagent conversation' });
    const flowBefore = document.querySelector('.react-flow');
    expect(screen.getAllByText(/Waiting for agent/).length).toBeGreaterThan(0);

    // Clicking Open subagent conversation invokes durable child ref via scope.openWorkflow without name guessing.
    fireEvent.click(openChildBtn);
    expect(openWorkflowSpy).toHaveBeenCalledWith('agent-99', 'sub-99', 'worker');

    // Re-render with advancing trace cut2 (selected output arrives) + settled live owner:
    const settledRun: WorkflowRunView = {
      ...liveRunWithChild,
      state: { type: 'settled', outcome: 'completed' },
      instances: [{ ...liveRunWithChild.instances[0], state: { type: 'settled', outcome: 'completed' } }],
    };
    ui.rerender(
      <SubagentContext.Provider value={subScope}>
        <WorkflowRunCanvas
          target={{
            conversationId: 'conv-hist',
            runId: run,
            workflowId: 'nested_flow',
            sourceMode: 'live',
            selectedVisit: visit1Cut2.instance,
            traceRoot,
            traceCut: cut1,
            traceReader: readerCut1,
            transport,
          }}
          liveRuns={[settledRun]}
          resolvedTrace={{ traceRoot, traceCut: cut2, traceReader: readerCut2 }}
          traceStatus="ready"
          onClose={vi.fn()}
          organizeFn={organizeSpy}
        />
      </SubagentContext.Provider>,
    );

    fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
    await waitFor(() => expect(screen.getByText(/arrived/)).toBeTruthy());
    expect(document.querySelector('.react-flow')).toBe(flowBefore);
    ui.unmount();

    // Case C: Missing trace context (hasTraceNode=false & traceStatus='unavailable') renders no-trace-field instead of "not recorded".
    render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-hist',
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'live',
          transport,
        }}
        liveRuns={[settledRun]}
        traceStatus="unavailable"
        onClose={vi.fn()}
        organizeFn={organizeSpy}
      />,
    );
    const workerNode = await waitFor(() => {
      const node = document.querySelector<HTMLElement>(`.react-flow__node[data-id="${agentViewId}"]`);
      expect(node).not.toBeNull();
      return node!;
    });
    fireEvent.click(workerNode!);
    fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
    expect(screen.getByText('Trace input and output are unavailable at this read cut.')).toBeTruthy();
    expect(screen.queryByText('Output not recorded.')).toBeNull();
  });

  it('prevents stale retained liveRuns from impersonating Running ownership across ConversationRunCanvasHost while updating live cut smoothly', async () => {
    const doc = makeNestedDoc();
    const run = baseRunId('52', 'conv-A');
    const server = new Server();
    await server.attached('A');
    try {
      const target = server.client.getSnapshot().views.A!.target!;
      const cut1 = cutFor('conv-A');
      const liveRunRunning: WorkflowRunView = {
        id: run,
        workflow_id: 'nested_flow',
        program_digest: 'sha256:host',
        resource_revision: '1',
        state: { type: 'running' },
        omitted_instances: 0,
        steps_consumed: 1,
        steps_max: 8,
        agents_consumed: 0,
        model_calls_consumed: 0,
        instances: [{
          block: { run, definition: { workflow_id: 'nested_flow', blocks: ['outer', 'body'] }, invocations: [0, 1] },
          node: 'worker',
          visit: 0,
          kind: 'template',
          state: { type: 'running' },
          iteration: 1,
          iterations_max: 2,
          loop_exit: null,
          checks_passed: null,
          child: null,
          child_agent_id: null,
        }],
      };
      const inlineSettledRun: WorkflowRunView = {
        ...liveRunRunning,
        state: { type: 'settled', outcome: 'completed' },
        instances: [{ ...liveRunRunning.instances[0], state: { type: 'settled', outcome: 'completed' } }],
      };

      const bytes = new TextEncoder().encode(JSON.stringify(doc));
      let bin = '';
      for (const b of bytes) bin += String.fromCharCode(b);
      const b64 = btoa(bin);
      const sha = createHash('sha256').update(bytes).digest('hex');

      server.handlers.set('session/traceLocateWorkflow', () => ({ type: 'trace_workflow_location' as const, location: null }));
      server.handlers.set('conversation/workflowRunSnapshotHeader', () => ({
        type: 'workflow_run_snapshot_header' as const,
        header: {
          run_id: run,
          workflow_id: 'nested_flow',
          program_digest: 'sha256:host',
          source_revision: '1',
          snapshot_digest: 'snap-1',
          has_source_presentation: true,
          byte_length: bytes.length,
          sha256: sha,
        },
      }));
      server.handlers.set('conversation/workflowRunSnapshotChunk', () => ({
        type: 'workflow_run_snapshot_chunk' as const,
        chunk: { offset: 0, data: b64, next_offset: null },
      }));

      // Render host where hydrated liveRuns still holds stale prior cut (Running),
      // while liveRunsCurrentCut=false and current inlineRuns already settled!
      const ui = render(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="A"
          conversationId="conv-A"
          attachmentTarget={target}
          liveCheckpointCut={cut1}
          inlineRuns={[inlineSettledRun]}
          liveRuns={[liveRunRunning]}
          liveRunsCurrentCut={false}
          ownerCurrent
        >
          <WorkflowRunPanel run={inlineSettledRun} />
        </ConversationRunCanvasHost>,
      );

      fireEvent.click(ui.getByRole('button', { name: 'View run graph' }));
      await screen.findByRole('dialog');
      await waitFor(() => expect(document.querySelectorAll('[data-run-node-status="completed"]').length).toBe(1));
      expect(document.querySelectorAll('[data-run-node-status="running"]')).toHaveLength(0);
    } finally {
      server.client.disconnect();
    }
  });

  it('preserves rendered WorkflowEditor drafts, undo/redo stacks, selection, and conflict notice, and mounts 256-node depth-8 Run Canvas with 0 value reads', async () => {
    const bigDoc = runBoundsDocument('maximum');
    expect(boundedDocument(bigDoc)).toBe(bigDoc);

    // 2. Render live authoring <WorkflowEditor> with unsaved invalid draft, undo & redo stacks, active UI node selection, and notice
    const writes: unknown[] = [];
    const authorSrc: WorkflowSource = { ...statusSource(), id: 'author_flow', revision: 'sha256:base', relative_path: '.agents/workflows/author_flow.yaml' };
    const authorPort: WorkflowScopePort = {
      endpoint: 'ws://127.0.0.1:1/',
      scope: USER_SCOPE,
      enable: async () => { throw Error('Unexpected authoring enable'); },
      analyze: async () => { throw Error('Unexpected authoring analysis'); },
      toolCatalog: async () => ({ sources: [] }),
      modelChoices: async () => ({ type: 'models', revision: 'catalog-1', models: [], next_offset: null }),
      list: async () => ({ target: { kind: 'user' }, directory: '/fixture', absent_revision: 'missing', diagnostic: null, files: [] } satisfies WorkflowLibrary),
      read: async () => structuredClone(authorSrc),
      check: async () => ({ revision: 'sha256:base', structure: { status: 'valid', outputs: {} }, dependencies: { status: 'not_checked' }, explanation: null }),
      write: (_id, _expected, edit) => { writes.push(edit); return new Promise(() => {}); },
    };
    const surface = createActor(workflowSurfaceMachine, { input: { ports: () => authorPort } }).start();
    const editor = createActor(workflowEditorMachine, {
      input: { port: authorPort, target: { endpoint: authorPort.endpoint, scope: authorPort.scope, id: authorSrc.id } },
    }).start();
    await waitFor(() => expect(editor.getSnapshot().matches('ready')).toBe(true));

    const field = nodeField([], 'ready', ['arguments', 'fields', 'status', 'value']);
    act(() => {
      editor.send({ type: 'EDIT', apply: document => ({ ...document, description: 'edit-1' }) });
      editor.send({ type: 'EDIT', apply: document => setField(document, field, 'edited') });
      editor.send({ type: 'DRAFT', address: field, text: '"unclosed' });
      editor.send({ type: 'UNDO' });
      editor.send({ type: 'SAVE' });
    });

    const ctxBefore = editor.getSnapshot().context;
    expect(ctxBefore.past).toHaveLength(1);
    expect(ctxBefore.future).toHaveLength(1);
    expect(Object.keys(ctxBefore.drafts).length).toBeGreaterThanOrEqual(1);
    expect(ctxBefore.notice).toEqual({ kind: 'pending_fields', count: 1 });

    const editorUi = render(<WorkflowEditor surface={surface} editor={editor} scopeLabel="Global" />);
    const authorNode = Array.from(editorUi.container.querySelectorAll<HTMLElement>('.react-flow__node'))
      .find(node => node.dataset.id === viewId([], 'node', 'ready'))!;
    expect(authorNode).toBeTruthy();
    fireEvent.click(authorNode);
    await within(editorUi.container).findByRole('complementary', { name: 'Inspector' });
    expect(authorNode.classList.contains('selected')).toBe(true);
    expect(Object.values(ctxBefore.drafts)[0]).toMatchObject({ text: '"unclosed', conflict: 'changed' });

    // 3. Open mounted 256-node depth-8 WorkflowRunCanvas alongside <WorkflowEditor>, stream live update, close
    const run = baseRunId('51', 'conv-scale');
    const bigTransport = makeTransport(bigDoc, run, { hasSourcePresentation: false, workflowId: 'w256' });
    const cut = cutFor('conv-scale');
    const valueSpy = vi.fn();
    const traceReader: TraceFieldReader = {
      value: valueSpy,
      content: vi.fn(),
      page: vi.fn(),
    };
    let finishLayout!: () => void;
    const layoutReady = new Promise<void>(resolve => { finishLayout = resolve; });
    const organizeSpy = vi.fn(async (document: WorkflowFileDocument) => {
      const result = await organizeDocument(document);
      finishLayout();
      return result;
    });
    const deepestBlocks = [
      'loop_d0', 'body', 'loop_d1', 'body', 'loop_d2', 'body', 'loop_d3', 'body',
      'loop_d4', 'body', 'loop_d5', 'body', 'loop_d6', 'body', 'loop_d7', 'body',
    ];
    const runningRun: WorkflowRunView = {
      id: run,
      workflow_id: 'w256',
      program_digest: 'sha256:w256',
      resource_revision: '1',
      state: { type: 'running' },
      omitted_instances: 0,
      steps_consumed: 1,
      steps_max: 4096,
      agents_consumed: 0,
      model_calls_consumed: 0,
      instances: [{
        block: { run, definition: { workflow_id: 'w256', blocks: deepestBlocks }, invocations: [0, 1, 1, 1, 1, 1, 1, 1, 1] },
        node: 'work_d8_0',
        visit: 0,
        kind: 'template',
        state: { type: 'running' },
        iteration: 1,
        iterations_max: 4,
        loop_exit: null,
        checks_passed: null,
        child: null,
        child_agent_id: null,
      }],
    };
    const closeSpy = vi.fn();
    const canvasUi = render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-scale',
          runId: run,
          workflowId: 'w256',
          selectedVisit: { block: runningRun.instances[0].block, node: 'work_d8_0', visit: 0 },
          sourceMode: 'live',
          traceCut: cut,
          traceReader,
          transport: bigTransport,
        }}
        liveRuns={[runningRun]}
        onClose={closeSpy}
        organizeFn={organizeSpy}
      />,
    );

    await screen.findByRole('dialog');
    // Observe the actual layout completion rather than racing Testing Library's
    // one-second DOM wait against a loaded full-suite worker.
    await act(async () => { await layoutReady; });
    expect(screen.getByRole('dialog').querySelectorAll('.react-flow__node')).toHaveLength(256);
    expect(organizeSpy).toHaveBeenCalledTimes(1);
    expect(within(screen.getByRole('dialog')).getByRole('navigation', { name: 'Block path' }).textContent).toContain('loop_d7');
    expect(within(screen.getByRole('dialog')).getByRole('navigation', { name: 'Block path' }).textContent).toContain('work_d8_0');
    const mountedFlow = screen.getByRole('dialog').querySelector('.react-flow');
    const tLive0 = performance.now();
    canvasUi.rerender(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-scale',
          runId: run,
          workflowId: 'w256',
          selectedVisit: { block: runningRun.instances[0].block, node: 'work_d8_0', visit: 0 },
          sourceMode: 'live',
          traceCut: cut,
          traceReader,
          transport: bigTransport,
        }}
        liveRuns={[{
          ...runningRun,
          state: { type: 'settled', outcome: 'completed' },
          instances: [{ ...runningRun.instances[0], state: { type: 'settled', outcome: 'completed' } }],
        }]}
        onClose={closeSpy}
        organizeFn={organizeSpy}
      />,
    );
    const liveUpdateMs = performance.now() - tLive0;
    expect(organizeSpy).toHaveBeenCalledTimes(1);
    expect(valueSpy).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog').querySelector('.react-flow')).toBe(mountedFlow);
    console.info(`[bench] mounted 256-node depth-8 WorkflowRunCanvas live update: ${liveUpdateMs.toFixed(1)}ms, organizeCalls=${organizeSpy.mock.calls.length}, valueReads=${valueSpy.mock.calls.length}`);

    fireEvent.click(screen.getByRole('button', { name: 'Close run graph' }));
    canvasUi.unmount();

    // Assert authoring editor state is 100% untouched
    const ctxAfter = editor.getSnapshot().context;
    expect(writes).toHaveLength(0);
    expect(ctxAfter.document).toEqual(ctxBefore.document);
    expect(ctxAfter.past).toEqual(ctxBefore.past);
    expect(ctxAfter.future).toEqual(ctxBefore.future);
    expect(ctxAfter.drafts).toEqual(ctxBefore.drafts);
    expect(ctxAfter.notice).toEqual(ctxBefore.notice);
    expect(within(editorUi.container).getByRole('complementary', { name: 'Inspector' }).isConnected).toBe(true);
    expect(authorNode.classList.contains('selected')).toBe(true);

    editor.stop();
    surface.stop();
  });
  it.each(['uncertain', 'conflict'] as const)('preserves a rendered authoring %s save while inspecting and closing a run', async kind => {
    const source = { ...statusSource(), revision: 'sha256:base' };
    let reads = 0, writes = 0;
    const port: WorkflowScopePort = {
      endpoint: 'ws://127.0.0.1:1/', scope: USER_SCOPE,
      enable: async () => { throw Error('Unexpected enable'); }, analyze: async () => { throw Error('Unexpected analysis'); },
      toolCatalog: async () => ({ sources: [] }), modelChoices: async () => ({ type: 'models', revision: '1', models: [], next_offset: null }),
      list: async () => ({ target: { kind: 'user' }, directory: '/fixture', absent_revision: 'missing', diagnostic: null, files: [] }),
      read: async () => { reads++; return structuredClone(source); },
      check: async () => ({ revision: source.revision, structure: { status: 'valid', outputs: {} }, dependencies: { status: 'not_checked' }, explanation: null }),
      write: async () => {
        writes++;
        throw kind === 'uncertain' ? new WorkspaceHostError('acknowledgement lost', undefined, true)
          : new WorkspaceHostError('source changed', 'source_conflict', false, { kind: 'source_conflict', scope: 'workspace', expected: 'sha256:base', actual: 'sha256:external' });
      },
    };
    const surface = createActor(workflowSurfaceMachine, { input: { ports: () => port } }).start();
    const editor = createActor(workflowEditorMachine, { input: { port, target: { endpoint: port.endpoint, scope: port.scope, id: source.id } } }).start();
    await waitFor(() => expect(editor.getSnapshot().matches('ready')).toBe(true));
    editor.send({ type: 'EDIT', apply: document => ({ ...document, description: 'unsaved local change' }) });
    editor.send({ type: 'SAVE' });
    await waitFor(() => expect(editor.getSnapshot().context.notice).toMatchObject({ kind: 'failure', operation: 'save', failure: { kind } }));
    const author = render(<WorkflowEditor surface={surface} editor={editor} scopeLabel="Global" />);
    const before = editor.getSnapshot().context;
    const run = baseRunId();
    const canvas = render(<WorkflowRunCanvas target={{ runId: run, conversationId: run.conversation_id, sourceMode: 'history', transport: makeTransport(makeNestedDoc(), run) }} onClose={() => {}} />);
    try {
      await waitFor(() => expect(screen.getByRole('dialog').querySelectorAll('.react-flow__node').length).toBeGreaterThan(0));
      fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: /^Zoom in$/ }));
      fireEvent.click(screen.getByRole('button', { name: 'Close run graph' }));
      canvas.unmount();
      const after = editor.getSnapshot().context;
      expect(after.document).toBe(before.document);
      expect(after.past).toBe(before.past); expect(after.future).toBe(before.future);
      expect(after.drafts).toBe(before.drafts); expect(after.notice).toEqual(before.notice);
      expect(after.uncertain).toBe(before.uncertain);
      expect(reads).toBe(1); expect(writes).toBe(1);
    } finally { author.unmount(); editor.stop(); surface.stop(); }
  });

  it('uses the production Chat host and fences held visit values and expired-cut errors without reloading the graph', async () => {
    const server = new Server();
    await server.attached('A');
    const target = server.client.getSnapshot().views.A!.target!;
    const run = baseRunId('41', 'conv-A');
    const firstCut = { ...cutFor('conv-A'), through: '10' }, nextCut = { ...cutFor('conv-A'), revision: '2', through: '20' };
    const transport = makeTransport(makeNestedDoc(), run);
    const header = await transport.readHeader(() => true), chunk = await transport.readChunk(0, 65536, () => true);
    let currentCut: CheckpointReadCut = firstCut;
    const bodies: Record<string, string> = {
      'journal.7': JSON.stringify({ answer: 'obsolete selected visit' }),
      'journal.9': JSON.stringify({ answer: 'current payload' }),
    };
    const node = (iteration: number): WorkflowTraceNode => {
      const domain = iteration === 1 ? 'journal.7' : 'journal.9';
      return { ...makeTraceNode(`workflow-node:${iteration + 1}`, run, iteration, 0, { type: 'settled', outcome: 'completed' }),
        output_preview: '{"answer":"truncated preview',
        output: { type: 'value', field: 'result', domain, item_offset: 0, path: ['event', 'value'], encoded_bytes: String(Buffer.byteLength(JSON.stringify(bodies[domain]))) },
      };
    };
    const root = (): TraceRecord => ({
      id: 'trace:1', position: 'trace:1', kind: 'workflow', state: 'completed', location: {}, timing: node(1).timing,
      calls: [], attachments: [], fields: [], has_detail: false, truncated: false,
      collections: [{ domain: 'trace.records.1.workflow.nodes', revision: currentCut.revision, total: '2', loaded: '0', next_offset: '0' }],
      native_id: null,
    });
    server.handlers.set('conversation/workflowRunSnapshotHeader', () => ({ type: 'workflow_run_snapshot_header', header }));
    server.handlers.set('conversation/workflowRunSnapshotChunk', () => ({ type: 'workflow_run_snapshot_chunk', chunk }));
    server.handlers.set('session/traceLocateWorkflow', request => {
      if (request.method !== 'session/traceLocateWorkflow') throw Error('wrong locator method');
      expect(request.params.run_id).toEqual(run);
      return { type: 'trace_workflow_location', location: { record_id: 'trace:1', page: { read_cut: currentCut, records: [root()], updates: [], next_cursor: null } } };
    });
    server.handlers.set('conversation/checkpointPage', request => {
      if (request.method !== 'conversation/checkpointPage') throw Error('wrong page method');
      return { type: 'checkpoint_page', page: { domain: request.params.domain, read_cut: request.params.cut, cursor: request.params.cut.revision, total: '2', collections: [], next_offset: null,
        items: [node(1), node(2)].map((visit, index) => ({ offset: String(index), identity: visit.id, value: visit as unknown as Record<string, unknown>, content: [], deferred: false, body_complete: true })),
      } };
    });
    server.handlers.set('conversation/checkpointValueRead', request => {
      if (request.method !== 'conversation/checkpointValueRead') throw Error('wrong value method');
      if (request.params.cut.revision === '1' && request.params.domain === 'journal.9') {
        throw new RpcFailure({ code: -32000, message: 'stale retained cut', data: { kind: 'read_cut_expired' } });
      }
      const bytes = Buffer.from(JSON.stringify(bodies[request.params.domain]));
      return { type: 'checkpoint_value_chunk', chunk: { representation: 'canonical_json_v1', cut: request.params.cut, domain: request.params.domain, item_offset: request.params.item_offset, path: request.params.path, offset: 0, readable_end: bytes.length, bytes_base64: bytes.toString('base64'), next_offset: null } };
    });
    server.held.add('conversation/checkpointValueRead');
    const liveRun: WorkflowRunView = {
      id: run, workflow_id: 'nested_flow', program_digest: 'sha256:fixture', resource_revision: '1', state: { type: 'settled', outcome: 'completed' },
      instances: [1, 2].map(iteration => makeVisitInstance(run, iteration, 0, { state: { type: 'settled', outcome: 'completed' } })), omitted_instances: 0,
      steps_consumed: 2, steps_max: 4096, agents_consumed: 0, model_calls_consumed: 0,
    };
    const surface = (cut: CheckpointReadCut, ownerCurrent = true) => <ConversationRunCanvasHost
      client={server.client} sessionId="A" conversationId="conv-A" attachmentTarget={target} ownerCurrent={ownerCurrent}
      liveCheckpointCut={cut} liveRuns={[liveRun]}><WorkflowRunPanel run={liveRun} /></ConversationRunCanvasHost>;
    const before = server.requests.length, ui = render(surface(firstCut));
    try {
      const opener = ui.getByRole('button', { name: 'View run graph' });
      fireEvent.click(opener);
      const workerId = viewId([{ node: 'outer' }], 'node', 'worker');
      const workerNode = await waitFor(() => {
        const element = document.querySelector(`.react-flow__node[data-id="${workerId}"]`);
        expect(element).not.toBeNull(); return element!;
      });
      fireEvent.click(workerNode);
      const choose = (iteration: number) => {
        const key = visitIdentityKey(node(iteration).instance);
        const option = Array.from(screen.getByRole('dialog').querySelectorAll<HTMLElement>('[data-run-visit-key]')).find(element => element.dataset.runVisitKey === key);
        expect(option).toBeTruthy(); fireEvent.click(option!);
      };
      choose(1);
      expect(server.requests.filter(item => item.request.method === 'conversation/checkpointValueRead')).toHaveLength(0);
      fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
      const oldVisit = await server.waitFor('conversation/checkpointValueRead', 1);
      choose(2);
      const oldCut = await server.waitFor('conversation/checkpointValueRead', 2);
      const flow = screen.getByRole('dialog').querySelector('.react-flow');
      currentCut = nextCut; ui.rerender(surface(nextCut));
      const currentRead = await server.waitFor('conversation/checkpointValueRead', 3);
      await act(async () => { server.reply(currentRead); });
      await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('current payload'));
      await act(async () => { server.reply(oldVisit); server.reply(oldCut); });
      expect(screen.getByRole('tabpanel').textContent).not.toContain('obsolete selected visit');
      expect(screen.getByRole('tabpanel').textContent).not.toContain('stale retained cut');
      expect(screen.getByRole('dialog').querySelector('.react-flow')).toBe(flow);
      const observed = server.requests.slice(before).map(item => item.request);
      expect(observed.filter(request => request.method === 'session/traceLocateWorkflow')).toHaveLength(2);
      expect(observed.filter(request => request.method === 'conversation/workflowRunSnapshotHeader')).toHaveLength(1);
      expect(observed.filter(request => request.method === 'conversation/workflowRunSnapshotChunk')).toHaveLength(1);
      expect(observed.filter(request => request.method === 'session/trace' || request.method === 'workflow/read')).toHaveLength(0);
      expect(observed.filter(request => METHOD_POLICY[request.method].effect === 'side_effecting')).toHaveLength(0);
      act(() => { fireEvent.click(screen.getByRole('button', { name: 'Close run graph' })); ui.rerender(surface(nextCut, false)); });
      await act(async () => {});
      expect(screen.queryByRole('dialog')).toBeNull(); expect(document.activeElement).not.toBe(opener);
    } finally { ui.unmount(); server.client.disconnect(); }
  });

  it('looks up an exact selected visit beyond the initial two-page prefix across advancing cuts without reusing old-cut headers or leaking held errors/results', async () => {
    const doc = makeNestedDoc();
    if (doc.block.nodes.outer.type === 'loop') doc.block.nodes.outer.loop_count = 69;
    const run = baseRunId('69', 'conv-paged');
    const cut1: CheckpointReadCut = { conversation_id: 'conv-paged', revision: '1', through: '1', domain_revisions: {}, collection_cuts: {}, activation_id: 'act-1' };
    const cut2: CheckpointReadCut = { conversation_id: 'conv-paged', revision: '2', through: '2', domain_revisions: {}, collection_cuts: {}, activation_id: 'act-1' };
    const cut3: CheckpointReadCut = { conversation_id: 'conv-paged', revision: '3', through: '3', domain_revisions: {}, collection_cuts: {}, activation_id: 'act-2' };
    const transport = makeTransport(doc, run, { hasSourcePresentation: false });

    const makeNode = (iter: number, state: WorkflowTraceNode['state'], outputDomain: string | null, rev = '1'): WorkflowTraceNode => {
      const encodedBytes = outputDomain
        ? String(new TextEncoder().encode(JSON.stringify(JSON.stringify({ domain: outputDomain, rev }))).length)
        : '0';
      return {
        id: `wn-${iter}`,
        instance: {
          block: { run, definition: { workflow_id: 'nested_flow', blocks: ['outer', 'body'] }, invocations: [0, iter] },
          node: 'worker',
          visit: 0,
        },
        kind: 'template',
        state,
        timing: { started_at: '2026-10-20T00:00:00Z', ended_at: state.type === 'settled' ? '2026-10-20T00:00:01Z' : null, duration_ms: '10' },
        input: null,
        output: outputDomain ? { type: 'value', field: 'result', domain: outputDomain, item_offset: 0, path: [], encoded_bytes: encodedBytes } : null,
        input_preview: null,
        output_preview: null,
        input_omitted: false,
        output_omitted: false,
      };
    };
    const iter69Cut1 = makeNode(69, { type: 'running' }, null, '1');
    const iter69Cut2 = makeNode(69, { type: 'settled', outcome: 'completed' }, 'out-69-cut2', '2');
    const iter69Cut3 = makeNode(69, { type: 'settled', outcome: 'completed' }, 'out-69-cut3', '3');
    const traceRoot: TraceRecord = {
      id: 'trace:70',
      position: '70',
      location: { attempt_id: 'att-69', step_id: 'step-1' },
      kind: 'workflow',
      state: 'running',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: null, duration_ms: null },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: 'trace.records.70.workflow.nodes', revision: '1', total: '70', loaded: '0', next_offset: '0' }],
      native_id: null,
      originating_tool_call_id: null,
      has_detail: true,
      truncated: false,
    };
    const makePrefixPage = (cut: CheckpointReadCut, offset: number): CheckpointPage => {
      const start = offset === 0 ? 1 : 33;
      const slice = Array.from({ length: 32 }, (_, idx) => makeNode(start + idx, { type: 'settled', outcome: 'completed' }, `out-${start + idx}`));
      return {
        domain: 'trace.records.70.workflow.nodes',
        cursor: cut.revision,
        read_cut: cut,
        total: '70',
        collections: [],
        next_offset: offset === 0 ? '32' : '64',
        items: slice.map((node, i) => ({
          offset: String(offset + i),
          identity: node.id,
          value: node as unknown as Record<string, unknown>,
          content: [],
          deferred: false,
          body_complete: true,
        })),
      };
    };

    const valueCalls: string[] = [];
    let rejectCut1Lookup!: (err: Error) => void;
    let resolveCut2Lookup!: (node: WorkflowTraceNode | null) => void;
    const reader1: TraceFieldReader = {
      page: vi.fn(async (cut, _domain, offset) => makePrefixPage(cut, offset)),
      workflowNode: vi.fn(async (cut) => {
        if (cut.revision === '1') {
          return new Promise<WorkflowTraceNode | null>((_resolve, reject) => {
            rejectCut1Lookup = reject;
          });
        }
        return iter69Cut2;
      }),
      value: vi.fn(async (_cut, field) => {
        valueCalls.push(`${field.domain}:${_cut.revision}`);
        const payload = JSON.stringify({ domain: field.domain, rev: _cut.revision });
        const bytes = new TextEncoder().encode(JSON.stringify(payload));
        let bin = '';
        for (const b of bytes) bin += String.fromCharCode(b);
        return {
          representation: 'canonical_json_v1' as const,
          cut: _cut,
          ...field,
          offset: 0,
          bytes_base64: btoa(bin),
          next_offset: null,
          readable_end: bytes.length,
        };
      }),
      content: vi.fn(),
    };
    const reader2: TraceFieldReader = {
      ...reader1,
      page: vi.fn(async (cut, _domain, offset) => makePrefixPage(cut, offset)),
      workflowNode: vi.fn(async (cut) => {
        if (cut.revision === '2') {
          return new Promise<WorkflowTraceNode | null>((resolve) => {
            resolveCut2Lookup = resolve;
          });
        }
        return iter69Cut3;
      }),
    };

    const ui = render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-paged',
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'live',
          selectedVisit: iter69Cut1.instance,
          selectedTraceNode: iter69Cut1,
          traceRoot,
          traceCut: cut1,
          traceReader: reader1,
          transport,
        }}
        liveRuns={[{
          id: run,
          workflow_id: 'nested_flow',
          program_digest: 'sha256:p',
          resource_revision: '1',
          state: { type: 'running' },
          instances: [makeVisitInstance(run, 69, 0, { state: { type: 'running' } })],
          omitted_instances: 69,
          steps_consumed: 70,
          steps_max: 4096,
          agents_consumed: 0,
          model_calls_consumed: 0,
        }]}
        onClose={vi.fn()}
        organizeFn={organizeDocument}
      />,
    );

    await waitFor(() => expect(reader1.page).toHaveBeenCalledTimes(2));
    // Old cut1 had target.selectedTraceNode (output: null); zero eager value reads on Overview.
    expect(valueCalls).toHaveLength(0);

    // Advance to cut2 (new initial 2 pages omit iter 69); cut1's target.selectedTraceNode must NOT be reused!
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-paged',
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          selectedVisit: iter69Cut1.instance,
          selectedTraceNode: iter69Cut1,
          traceRoot,
          traceCut: cut1,
          traceReader: reader1,
          transport,
        }}
        resolvedTrace={{ traceRoot, traceCut: cut2, traceReader: reader2 }}
        traceStatus="ready"
        onClose={vi.fn()}
        organizeFn={organizeDocument}
      />,
    );
    await waitFor(() => expect(resolveCut2Lookup).toBeTypeOf('function'));
    expect(valueCalls).toHaveLength(0);

    await act(async () => {
      resolveCut2Lookup(iter69Cut2);
    });
    // Switch to Output tab: reads lazy output of exact visit 69 at cut2!
    fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
    await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('out-69-cut2'));
    expect(valueCalls).toEqual(['out-69-cut2:2']);

    // Now test held error fence + activation fence: advance to cut3 (act-2) while a slow failure rejects later.
    let rejectSlowCut2Error!: (err: Error) => void;
    const errorReader: TraceFieldReader = {
      ...reader2,
      workflowNode: vi.fn(async () => new Promise<WorkflowTraceNode | null>((_res, rej) => {
        rejectSlowCut2Error = rej;
      })),
    };
    const cut2b: CheckpointReadCut = { ...cut2, revision: '2b', through: '2b' };
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-paged',
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          selectedVisit: iter69Cut1.instance,
          traceRoot,
          traceCut: cut1,
          traceReader: reader1,
          transport,
        }}
        resolvedTrace={{ traceRoot, traceCut: cut2b, traceReader: errorReader }}
        traceStatus="ready"
        onClose={vi.fn()}
        organizeFn={organizeDocument}
      />,
    );
    await waitFor(() => expect(rejectSlowCut2Error).toBeTypeOf('function'));
    // Switch activation to cut3 before cut2b's lookup rejects:
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-paged',
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          selectedVisit: iter69Cut1.instance,
          traceRoot,
          traceCut: cut1,
          traceReader: reader1,
          transport,
        }}
        resolvedTrace={{ traceRoot, traceCut: cut3, traceReader: reader2 }}
        traceStatus="ready"
        onClose={vi.fn()}
        organizeFn={organizeDocument}
      />,
    );
    await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('out-69-cut3'));
    await act(async () => {
      rejectSlowCut2Error(new Error('stale cut2b lookup error'));
    });
    expect(screen.getByRole('tabpanel').textContent).not.toContain('stale cut2b lookup error');
    expect(screen.getByRole('tabpanel').textContent).toContain('out-69-cut3');
    void rejectCut1Lookup;
  });

  it('opens a WorkflowStarted-only historical root via typed trace detail even with empty nodes, absent native_id, and long escaped IDs', async () => {
    const doc = makeNestedDoc();
    const convId = 'conv-"escaped"\\start-only';
    const longRun: WorkflowRunId = {
      conversation_id: convId,
      origin: { type: 'tool', attempt_id: `att"\\${'z'.repeat(200)}` },
      invocation: '9',
    };
    const cut: CheckpointReadCut = {
      conversation_id: convId,
      revision: '12',
      through: '12',
      domain_revisions: {},
      collection_cuts: {},
    };
    const transport = makeTransport(doc, longRun, { workflowId: 'start_only_flow', hasSourcePresentation: false });

    const traceRoot: TraceRecord = {
      id: 'trace:202',
      position: '202',
      location: { attempt_id: 'att-start-only', step_id: 'step-1' },
      kind: 'workflow',
      state: 'failed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '50' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: 'trace.records.202.workflow.nodes', revision: '12', total: '0', loaded: '0', next_offset: null }],
      native_id: null,
      originating_tool_call_id: null,
      has_detail: true,
      truncated: false,
    };
    const requestedDomains: string[] = [];
    const onResolveRunId = vi.fn();
    const reader: TraceFieldReader = {
      page: vi.fn(async (reqCut, domain) => {
        requestedDomains.push(domain);
        if (domain !== 'trace.records.202.detail') throw new Error(`Unexpected domain ${domain}`);
        return {
          domain,
          cursor: reqCut.revision,
          read_cut: reqCut,
          total: '1',
          collections: [],
          next_offset: null,
          items: [{
            offset: '0',
            identity: traceRoot.id,
            value: {
              id: traceRoot.id,
              kind: 'workflow',
              record: traceRoot,
              content: { type: 'workflow', run_id: longRun, workflow_id: 'start_only_flow' },
            } as unknown as Record<string, unknown>,
            content: [],
            deferred: true,
            body_complete: false,
          }],
        };
      }),
      value: vi.fn(),
      content: vi.fn(),
    };

    const ui = render(
      <WorkflowRunCanvas
        target={{
          conversationId: convId,
          sourceMode: 'history',
          traceRoot,
          traceNodes: [],
          traceCut: cut,
          traceReader: reader,
          transport,
        }}
        onResolveRunId={onResolveRunId}
        onClose={vi.fn()}
        organizeFn={organizeDocument}
      />,
    );

    await waitFor(() => expect(document.querySelectorAll('.react-flow__node').length).toBe(6));
    expect(requestedDomains).toEqual(['trace.records.202.detail']);
    expect(onResolveRunId).toHaveBeenCalledWith(longRun);
    expect(screen.getByRole('heading', { level: 2 }).textContent).toContain('start_only_flow');
    ui.unmount();

    // Foreign conversation in detail must be rejected cleanly without falling back.
    const foreignReader: TraceFieldReader = {
      ...reader,
      page: vi.fn(async (reqCut, domain) => ({
        domain,
        cursor: reqCut.revision,
        read_cut: reqCut,
        total: '1',
        collections: [],
        next_offset: null,
        items: [{
          offset: '0',
          identity: traceRoot.id,
          value: {
            id: traceRoot.id,
            kind: 'workflow',
            record: traceRoot,
            content: {
              type: 'workflow',
              run_id: { ...longRun, conversation_id: 'foreign-conv' },
              workflow_id: 'start_only_flow',
            },
          } as unknown as Record<string, unknown>,
          content: [],
          deferred: true,
          body_complete: false,
        }],
      })),
    };
    render(
      <WorkflowRunCanvas
        target={{
          conversationId: convId,
          sourceMode: 'history',
          traceRoot,
          traceCut: cut,
          traceReader: foreignReader,
          transport,
        }}
        onClose={vi.fn()}
        organizeFn={organizeDocument}
      />,
    );
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('conversation changed'));
  });

  it('retains user-selected visit2 over opener visit1 across 128/192/256 nested-loop paging without camera or focus steal', async () => {
    const base = statusSource().document!;
    const doc: WorkflowFileDocument = structuredClone(base);
    doc.block.nodes = {
      workflow_start: { type: 'start' },
      outer: {
        type: 'loop',
        loop_count: 3,
        break_condition: null,
        body: {
          nodes: {
            outer_start: { type: 'start' },
            inner: {
              type: 'loop',
              loop_count: 100,
              break_condition: null,
              body: {
                nodes: {
                  inner_start: { type: 'start' },
                  worker: { type: 'template', variables: {}, template: 'nested visit' },
                },
                edges: [{ from: 'inner_start', port: 'next', to: 'worker' }],
              },
              loop_variables: [],
            },
          },
          edges: [{ from: 'outer_start', port: 'next', to: 'inner' }],
        },
        loop_variables: [],
      },
      done: { type: 'return', outputs: [] },
    };
    doc.block.edges = [
      { from: 'workflow_start', port: 'next', to: 'outer' },
      { from: 'outer', port: 'next', to: 'done' },
    ];

    const runId = baseRunId('404', 'conv-retain');
    const cut = cutFor('conv-retain');
    const transport = makeTransport(doc, runId, { workflowId: 'nested_3x100' });
    const organizeSpy = vi.fn(async (d: WorkflowFileDocument) => d);

    const blocks = ['outer', 'body', 'inner', 'body'];
    const allVisits: WorkflowTraceNode[] = [];
    for (let o = 1; o <= 3; o++) {
      for (let i = 1; i <= 100; i++) {
        const ordinal = (o - 1) * 100 + i;
        const id = `wn-nested-${ordinal}`;
        allVisits.push({
          id,
          instance: {
            block: { run: runId, definition: { workflow_id: 'nested_3x100', blocks }, invocations: [0, o, i] },
            node: 'worker',
            visit: 0,
          },
          kind: 'template',
          state: { type: 'settled', outcome: 'completed' },
          timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '5' },
          input: { type: 'value', field: 'arguments', domain: `in-${id}`, item_offset: 0, path: [], encoded_bytes: String(nodeBodyBytes(`in-${id}`)) },
          output: { type: 'value', field: 'result', domain: `out-${id}`, item_offset: 0, path: [], encoded_bytes: String(nodeBodyBytes(`out-${id}`)) },
          input_preview: null,
          output_preview: null,
          input_omitted: false,
          output_omitted: false,
        });
      }
    }

    const traceDomain = 'trace.records.404.workflow.nodes';
    const valueSpy = vi.fn(async (_cut: CheckpointReadCut, source: Parameters<TraceFieldReader['value']>[1]): Promise<CheckpointValueChunk> => {
      const bytes = Buffer.from(JSON.stringify(nodeBody(source.domain)), 'utf8');
      return {
        representation: 'canonical_json_v1',
        cut,
        domain: source.domain,
        item_offset: source.item_offset,
        path: [...source.path],
        offset: 0,
        bytes_base64: bytes.toString('base64'),
        next_offset: null,
        readable_end: bytes.length,
      };
    });
    const reader: TraceFieldReader = {
      value: valueSpy,
      content: vi.fn(),
      page: vi.fn(async (_cut, domain, offset): Promise<CheckpointPage> => {
        const slice = allVisits.slice(offset, offset + 32);
        const next = offset + slice.length;
        return {
          domain,
          cursor: cut.revision,
          read_cut: cut,
          total: String(allVisits.length),
          collections: [],
          next_offset: next < allVisits.length ? String(next) : null,
          items: slice.map((node, idx) => ({
            offset: String(offset + idx),
            identity: node.id,
            value: node as unknown as Record<string, unknown>,
            content: [],
            deferred: false,
            body_complete: true,
          })),
        };
      }),
    };
    const traceRoot: TraceRecord = {
      id: 'trace:404',
      position: '404',
      location: { attempt_id: 'a404', step_id: 's1' },
      kind: 'workflow',
      state: 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '100' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: traceDomain, revision: cut.revision, total: String(allVisits.length), loaded: '0', next_offset: '0' }],
      native_id: null,
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    };

    const visit1 = allVisits[0]!;
    const visit2 = allVisits[1]!;
    const visit1Key = visitIdentityKey(visit1.instance);
    const visit2Key = visitIdentityKey(visit2.instance);

    render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-retain',
          runId,
          workflowId: 'nested_3x100',
          sourceMode: 'history',
          selectedVisit: visit1.instance,
          traceRoot,
          traceCut: cut,
          traceReader: reader,
          transport,
        }}
        onClose={vi.fn()}
        organizeFn={organizeSpy}
      />,
    );

    const dialog = await screen.findByRole('dialog');
    await waitFor(() => expect(dialog.querySelectorAll('[data-run-visit-key]')).toHaveLength(64));
    const flowBefore = dialog.querySelector('.react-flow');
    expect(flowBefore).toBeTruthy();

    // Explicitly choose visit 2 (overriding opener visit 1).
    const visit2Button = dialog.querySelector<HTMLButtonElement>(`[data-run-visit-key="${CSS.escape(visit2Key)}"]`)!;
    expect(visit2Button).toBeTruthy();
    fireEvent.click(visit2Button);
    expect(visit2Button.getAttribute('aria-selected')).toBe('true');

    // Page to 128, 192, and 256 visits (> 200 per-node retention cap).
    const loadMoreBtn = screen.getByRole('button', { name: 'Load more trace visits' });
    loadMoreBtn.focus();
    fireEvent.click(loadMoreBtn);
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('128 of 300'));
    expect(dialog.querySelectorAll('[data-run-visit-key]').length).toBe(128);

    fireEvent.click(screen.getByRole('button', { name: 'Load more trace visits' }));
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('192 of 300'));
    expect(dialog.querySelectorAll('[data-run-visit-key]').length).toBe(192);

    fireEvent.click(screen.getByRole('button', { name: 'Load more trace visits' }));
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('256 of 300'));

    // Per-node list is bounded <= 200, visit 2 remains retained and selected, opener visit 1 is not forced over visit 2.
    const retainedButtons = dialog.querySelectorAll('[data-run-visit-key]');
    expect(retainedButtons.length).toBe(200);
    const retainedVisit2 = dialog.querySelector<HTMLElement>(`[data-run-visit-key="${CSS.escape(visit2Key)}"]`);
    expect(retainedVisit2).toBeTruthy();
    expect(retainedVisit2?.getAttribute('aria-selected')).toBe('true');
    expect(dialog.querySelector(`[data-run-visit-key="${CSS.escape(visit1Key)}"]`)).toBeNull();

    // No camera / layout / focus steal.
    expect(dialog.querySelector('.react-flow')).toBe(flowBefore);
    expect(organizeSpy).toHaveBeenCalledTimes(0);
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Load more trace visits' }));

    // Details / I/O identity remains visit 2.
    expect(valueSpy).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
    await waitFor(() => expect(valueSpy).toHaveBeenCalledTimes(1));
    expect(valueSpy.mock.calls[0]?.[1].domain).toBe('out-wn-nested-2');
    await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('out-wn-nested-2'));
  });

  it('recovers cleanly on retry after page 96 fails mid-batch without false duplicate errors', async () => {
    const doc = makeNestedDoc();
    const runId = baseRunId('405', 'conv-retry-page');
    const cut = cutFor('conv-retry-page');
    const transport = makeTransport(doc, runId);
    const total = 128;
    const allVisits = Array.from({ length: total }, (_, idx) =>
      makeTraceNode(`wn-batch-${idx}`, runId, idx + 1, 0, { type: 'settled', outcome: 'completed' }),
    );
    const traceDomain = 'trace.records.405.workflow.nodes';
    const traceRoot: TraceRecord = {
      id: 'trace:405',
      position: '405',
      location: { attempt_id: 'a405', step_id: 's1' },
      kind: 'workflow',
      state: 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '50' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: traceDomain, revision: cut.revision, total: String(total), loaded: '0', next_offset: '0' }],
      native_id: JSON.stringify(runId),
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    };

    let failAt96 = true;
    const pageOffsets: number[] = [];
    const reader: TraceFieldReader = {
      value: vi.fn(),
      content: vi.fn(),
      page: vi.fn(async (_cut, domain, offset): Promise<CheckpointPage> => {
        pageOffsets.push(offset);
        if (offset === 96 && failAt96) throw new Error('injected page 96 error');
        const slice = allVisits.slice(offset, offset + 32);
        const next = offset + slice.length;
        return {
          domain,
          cursor: cut.revision,
          read_cut: cut,
          total: String(total),
          collections: [],
          next_offset: next < total ? String(next) : null,
          items: slice.map((node, i) => ({
            offset: String(offset + i),
            identity: node.id,
            value: node as unknown as Record<string, unknown>,
            content: [],
            deferred: false,
            body_complete: true,
          })),
        };
      }),
    };

    render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-retry-page',
          runId,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          selectedVisit: allVisits[0]!.instance,
          traceRoot,
          traceCut: cut,
          traceReader: reader,
          transport,
        }}
        onClose={vi.fn()}
      />,
    );

    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('64 of 128'));
    expect(pageOffsets).toEqual([0, 32]);

    // Trigger batch 2: offset 64 succeeds, offset 96 fails -> surfaces trace error alert and leaves committed offset at 64.
    fireEvent.click(screen.getByRole('button', { name: 'Load more trace visits' }));
    await waitFor(() => expect(pageOffsets).toEqual([0, 32, 64, 96]));
    const batchAlert = await screen.findByRole('alert');
    expect(batchAlert.textContent).toContain('injected page 96 error');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Load more trace visits' }).hasAttribute('disabled')).toBe(false));
    expect(screen.getByRole('status').textContent).toContain('64 of 128');

    // Recover network and retry: starts cleanly at 64, 96, clears alert, and commits all 128 visits with no false duplicate error.
    failAt96 = false;
    fireEvent.click(screen.getByRole('button', { name: 'Load more trace visits' }));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Load more trace visits' })).toBeNull());
    expect(screen.queryByRole('alert')).toBeNull();
    expect(pageOffsets).toEqual([0, 32, 64, 96, 64, 96]);
    expect(document.body.querySelectorAll('[data-run-visit-key]').length).toBe(128);
  });

  it('recovers visits and paging through real Trajectory + ConversationRunCanvasHost when opened before initial read completes', async () => {
    const doc = makeNestedDoc();
    if (doc.block.nodes.outer.type === 'loop') doc.block.nodes.outer.loop_count = 70;
    const server = new Server();
    await server.attached('A');
    const attTarget = server.client.getSnapshot().views.A!.target!;
    const convId = attTarget.conversation_id;
    const runId = baseRunId('406', convId);
    const cut = cutFor(convId);
    const transport = makeTransport(doc, runId, { workflowId: 'nested_flow' });
    const header = await transport.readHeader(() => true);
    const chunk = await transport.readChunk(0, 65536, () => true);
    server.handlers.set('conversation/workflowRunSnapshotHeader', () => ({ type: 'workflow_run_snapshot_header', header }));
    server.handlers.set('conversation/workflowRunSnapshotChunk', () => ({ type: 'workflow_run_snapshot_chunk', chunk }));

    const total = 70;
    const allNodes = Array.from({ length: total }, (_, idx) =>
      makeTraceNode(`wn-held-${idx}`, runId, idx + 1, 0, { type: 'settled', outcome: 'completed' }),
    );
    const traceDomain = 'trace.records.406.workflow.nodes';
    const detailDomain = 'trace.records.406.detail';
    const traceRoot: TraceRecord = {
      id: 'trace:406',
      position: '406',
      location: { attempt_id: 'a406', step_id: 's1' },
      kind: 'workflow',
      state: 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '50' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: traceDomain, revision: cut.revision, total: String(total), loaded: '0', next_offset: '0' }],
      native_id: null,
      originating_tool_call_id: null,
      has_detail: true,
      truncated: false,
    };

    const heldReads: Array<() => void> = [];
    let holdInitialReads = true;
    const reader: TraceFieldReader = {
      value: vi.fn(),
      content: vi.fn(),
      page: vi.fn(async (reqCut, domain, offset): Promise<CheckpointPage> => {
        if (domain === detailDomain) {
          return {
            domain,
            cursor: reqCut.revision,
            read_cut: reqCut,
            total: '1',
            collections: [],
            next_offset: null,
            items: [{
              offset: '0',
              identity: traceRoot.id,
              value: {
                id: traceRoot.id,
                kind: 'workflow',
                record: traceRoot,
                content: { type: 'workflow', run_id: runId, workflow_id: 'nested_flow' },
              } as unknown as Record<string, unknown>,
              content: [],
              deferred: true,
              body_complete: false,
            }],
          };
        }
        const slice = allNodes.slice(offset, offset + 32);
        const next = offset + slice.length;
        const page: CheckpointPage = {
          domain,
          cursor: reqCut.revision,
          read_cut: reqCut,
          total: String(total),
          collections: [],
          next_offset: next < total ? String(next) : null,
          items: slice.map((node, i) => ({
            offset: String(offset + i),
            identity: node.id,
            value: node as unknown as Record<string, unknown>,
            content: [],
            deferred: false,
            body_complete: true,
          })),
        };
        if (holdInitialReads && offset === 0) {
          return new Promise(resolve => {
            heldReads.push(() => resolve(page));
          });
        }
        return page;
      }),
    };

    const cache: TraceCache = {
      epoch: 1,
      page: { records: [traceRoot], next_cursor: null, read_cut: cut },
      readCuts: { [traceRoot.id]: cut },
      fields: reader,
      details: {},
    };

    try {
      const ui = render(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="A"
          conversationId={convId}
          attachmentTarget={attTarget}
          ownerCurrent
        >
          <Trajectory cache={cache} loadEarlier={vi.fn()} onSelect={vi.fn()} onLoadDetail={vi.fn()} />
        </ConversationRunCanvasHost>,
      );

      // Expand workflow row so Trajectory starts loading nodes (held at offset 0).
      fireEvent.click(ui.container.querySelector('[data-trace-id="trace:406"] button[aria-expanded]')!);
      await waitFor(() => expect(heldReads.length).toBeGreaterThanOrEqual(1));

      // Open root run canvas BEFORE the Trajectory read completes.
      fireEvent.click(screen.getByRole('button', { name: 'View run graph' }));
      const dialog = await screen.findByRole('dialog');
      await waitFor(() => expect(dialog.querySelectorAll('.react-flow__node').length).toBeGreaterThan(0));

      // Release held reads and confirm canvas recovers visits and paging (not stuck at empty []).
      holdInitialReads = false;
      await act(async () => {
        for (const release of heldReads.splice(0)) release();
      });

      const workerViewId = viewId([{ node: 'outer' }], 'node', 'worker');
      await waitFor(() => expect(dialog.querySelector(`[data-id="${CSS.escape(workerViewId)}"] [data-run-node-status="completed"]`)).toBeTruthy());
      fireEvent.click(dialog.querySelector(`.react-flow__node[data-id="${CSS.escape(workerViewId)}"]`)!);
      await waitFor(() => expect(dialog.querySelectorAll('[data-run-visit-key]')).toHaveLength(64));

      // Remaining page (64..69) is also recoverable via Load more.
      const loadMore = within(dialog).getByRole('button', { name: 'Load more trace visits' });
      fireEvent.click(loadMore);
      await waitFor(() => expect(dialog.querySelectorAll('[data-run-visit-key]')).toHaveLength(70));
    } finally {
      server.client.disconnect();
    }
  });

  it('surfaces cold trace failure with independent trace retry and preserves coherent cut/total on refresh failure while fencing held responses', async () => {
    const doc = makeNestedDoc();
    const runId = baseRunId('407', 'conv-trace-err');
    const cut1: CheckpointReadCut = { conversation_id: 'conv-trace-err', revision: '1', through: '1', domain_revisions: {}, collection_cuts: {}, activation_id: 'act-1' };
    const cut2: CheckpointReadCut = { ...cut1, revision: '2', through: '2', activation_id: 'act-1' };
    const cut3: CheckpointReadCut = { ...cut1, revision: '3', through: '3', activation_id: 'act-2' };
    const transport = makeTransport(doc, runId, { hasSourcePresentation: false });
    const readHeaderSpy = vi.spyOn(transport, 'readHeader');
    const organizeSpy = vi.fn(async (d: WorkflowFileDocument) => d);

    const total = 70;
    const allNodes = Array.from({ length: total }, (_, idx) =>
      makeTraceNode(`wn-err-${idx}`, runId, idx + 1, 0, { type: 'settled', outcome: 'completed' }),
    );
    const traceDomain = 'trace.records.407.workflow.nodes';
    const traceRoot = (rev: string): TraceRecord => ({
      id: 'trace:407',
      position: '407',
      location: { attempt_id: 'a407', step_id: 's1' },
      kind: 'workflow',
      state: 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '50' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: traceDomain, revision: rev, total: String(total), loaded: '0', next_offset: '0' }],
      native_id: JSON.stringify(runId),
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    });

    let failCut1 = true;
    const valueSpy = vi.fn(async (reqCut: CheckpointReadCut, field: Parameters<TraceFieldReader['value']>[1]): Promise<CheckpointValueChunk> => {
      const bytes = Buffer.from(JSON.stringify({ domain: field.domain, cutRev: reqCut.revision }), 'utf8');
      return {
        representation: 'canonical_json_v1',
        cut: reqCut,
        domain: field.domain,
        item_offset: field.item_offset,
        path: [...field.path],
        offset: 0,
        bytes_base64: bytes.toString('base64'),
        next_offset: null,
        readable_end: bytes.length,
      };
    });
    const reader1: TraceFieldReader = {
      value: valueSpy,
      content: vi.fn(),
      page: vi.fn(async (reqCut, domain, offset): Promise<CheckpointPage> => {
        if (failCut1) throw new Error('trace page read failed at cut 1');
        const slice = allNodes.slice(offset, offset + 32);
        const next = offset + slice.length;
        return {
          domain,
          cursor: reqCut.revision,
          read_cut: reqCut,
          total: String(total),
          collections: [],
          next_offset: next < total ? String(next) : null,
          items: slice.map((node, i) => ({
            offset: String(offset + i),
            identity: node.id,
            value: node as unknown as Record<string, unknown>,
            content: [],
            deferred: false,
            body_complete: true,
          })),
        };
      }),
    };

    // Part A: Cold root with snapshot success + initial trace rejection shows root-visible error + Retry,
    // and Retry reruns ONLY trace (no snapshot reload, no relayout, no eager value reads).
    const ui = render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-trace-err',
          runId,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          traceRoot: traceRoot('1'),
          traceCut: cut1,
          traceReader: reader1,
          transport,
        }}
        onClose={vi.fn()}
        organizeFn={organizeSpy}
      />,
    );

    const dialog = await screen.findByRole('dialog');
    await waitFor(() => expect(dialog.querySelectorAll('.react-flow__node').length).toBeGreaterThan(0));
    const flowBefore = dialog.querySelector('.react-flow');
    expect(readHeaderSpy).toHaveBeenCalledTimes(1);
    expect(organizeSpy).toHaveBeenCalledTimes(1);
    expect(valueSpy).not.toHaveBeenCalled();

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('trace page read failed at cut 1');
    expect(alert.textContent).toContain('70');
    const retryTraceBtn = within(alert).getByRole('button', { name: 'Retry trace' });

    failCut1 = false;
    fireEvent.click(retryTraceBtn);
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('64 of 70'));
    expect(readHeaderSpy).toHaveBeenCalledTimes(1);
    expect(organizeSpy).toHaveBeenCalledTimes(1);
    expect(dialog.querySelector('.react-flow')).toBe(flowBefore);
    expect(valueSpy).not.toHaveBeenCalled();

    // Part B: Refresh failure on cut2 preserves last committed coherent cut1/reader1/nodes1 and total 70.
    const failingCut2Reader: TraceFieldReader = {
      value: vi.fn(async () => { throw new Error('must not mix cut2 reader with cut1 nodes'); }),
      content: vi.fn(),
      page: vi.fn(async () => { throw new Error('cut 2 refresh failed'); }),
    };
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-trace-err',
          runId,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          traceRoot: traceRoot('1'),
          traceCut: cut1,
          traceReader: reader1,
          transport,
        }}
        resolvedTrace={{ traceRoot: traceRoot('2'), traceCut: cut2, traceReader: failingCut2Reader }}
        traceStatus="ready"
        onClose={vi.fn()}
        organizeFn={organizeSpy}
      />,
    );

    const refreshAlert = await screen.findByRole('alert');
    expect(refreshAlert.textContent).toContain('cut 2 refresh failed');
    // Committed partial coverage from cut1 (64 of 70) is preserved without synthesizing total '0'.
    expect(screen.getByRole('status').textContent).toContain('64 of 70');

    // Selecting a node and opening Output uses coherent cut1 + reader1 (never mixes cut2).
    const workerViewId = viewId([{ node: 'outer' }], 'node', 'worker');
    fireEvent.click(dialog.querySelector(`.react-flow__node[data-id="${CSS.escape(workerViewId)}"]`)!);
    fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
    await waitFor(() => expect(valueSpy).toHaveBeenCalledTimes(1));
    expect(valueSpy.mock.calls[0]?.[0]).toEqual(cut1);

    // Part C: Held old cut/activation rejection AND held old cut success remain fenced when cut3 supersedes held cut2.
    fireEvent.click(screen.getByRole('tab', { name: 'Summary' }));
    const heldResponses: Array<{ reject: (err: Error) => void; resolve: (page: CheckpointPage) => void }> = [];
    const heldCut2Reader: TraceFieldReader = {
      value: valueSpy,
      content: vi.fn(),
      page: vi.fn(async () => new Promise<CheckpointPage>((resolve, reject) => {
        heldResponses.push({ resolve, reject });
      })),
    };
    const cut3Total = 72;
    const cut3Nodes = Array.from({ length: cut3Total }, (_, idx) =>
      makeTraceNode(`wn-err-cut3-${idx}`, runId, idx + 1, 0, { type: 'settled', outcome: 'completed' }),
    );
    const cut3Root: TraceRecord = {
      ...traceRoot('3'),
      collections: [{ domain: traceDomain, revision: '3', total: String(cut3Total), loaded: '0', next_offset: '0' }],
    };
    const cut3Reader: TraceFieldReader = {
      value: valueSpy,
      content: vi.fn(),
      page: vi.fn(async (reqCut, domain, offset): Promise<CheckpointPage> => {
        const slice = cut3Nodes.slice(offset, offset + 32);
        const next = offset + slice.length;
        return {
          domain,
          cursor: reqCut.revision,
          read_cut: reqCut,
          total: String(cut3Total),
          collections: [],
          next_offset: next < cut3Total ? String(next) : null,
          items: slice.map((node, i) => ({
            offset: String(offset + i),
            identity: node.id,
            value: node as unknown as Record<string, unknown>,
            content: [],
            deferred: false,
            body_complete: true,
          })),
        };
      }),
    };

    const cut2b: CheckpointReadCut = { ...cut2, revision: '2b', through: '2b' };
    const cut2c: CheckpointReadCut = { ...cut2, revision: '2c', through: '2c' };
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-trace-err',
          runId,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          traceRoot: traceRoot('1'),
          traceCut: cut1,
          traceReader: reader1,
          transport,
        }}
        resolvedTrace={{ traceRoot: traceRoot('2b'), traceCut: cut2b, traceReader: heldCut2Reader }}
        traceStatus="ready"
        onClose={vi.fn()}
        organizeFn={organizeSpy}
      />,
    );
    await waitFor(() => expect(heldResponses).toHaveLength(1));

    // Queue a second superseded cut whose held request resolves AFTER cut3 succeeds.
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-trace-err',
          runId,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          traceRoot: traceRoot('1'),
          traceCut: cut1,
          traceReader: reader1,
          transport,
        }}
        resolvedTrace={{ traceRoot: traceRoot('2c'), traceCut: cut2c, traceReader: heldCut2Reader }}
        traceStatus="ready"
        onClose={vi.fn()}
        organizeFn={organizeSpy}
      />,
    );
    await waitFor(() => expect(heldResponses).toHaveLength(2));

    // Supersede both with cut3 (act-2) before either cut2b or cut2c settles.
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-trace-err',
          runId,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          traceRoot: traceRoot('1'),
          traceCut: cut1,
          traceReader: reader1,
          transport,
        }}
        resolvedTrace={{ traceRoot: cut3Root, traceCut: cut3, traceReader: cut3Reader }}
        traceStatus="ready"
        onClose={vi.fn()}
        organizeFn={organizeSpy}
      />,
    );
    await waitFor(() => expect(screen.getByRole('status').textContent).toContain('64 of 72'));
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());

    // Late rejection from cut2b and late success from cut2c must both be ignored, preserving cut3 state.
    await act(async () => {
      heldResponses[0]!.reject(new Error('stale cut2b failure'));
      heldResponses[1]!.resolve({
        domain: traceDomain,
        cursor: cut2c.revision,
        read_cut: cut2c,
        total: String(total),
        collections: [],
        next_offset: null,
        items: allNodes.slice(0, 10).map((node, i) => ({
          offset: String(i),
          identity: node.id,
          value: node as unknown as Record<string, unknown>,
          content: [],
          deferred: false,
          body_complete: true,
        })),
      });
    });
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByRole('status').textContent).toContain('64 of 72');
  });

  it('omits stale selectedTraceNode when opening a late node row during held cut2 refresh so cut2 exact lookup resolves updated header and lazy output', async () => {
    const doc = makeNestedDoc();
    if (doc.block.nodes.outer.type === 'loop') doc.block.nodes.outer.loop_count = 70;
    const server = new Server();
    await server.attached('A');
    const attTarget = server.client.getSnapshot().views.A!.target!;
    const convId = attTarget.conversation_id;
    const runId = baseRunId('408', convId);
    const cut1: CheckpointReadCut = { conversation_id: convId, revision: '1', through: '1', domain_revisions: {}, collection_cuts: {}, activation_id: 'act-1' };
    const cut2: CheckpointReadCut = { ...cut1, revision: '2', through: '2', activation_id: 'act-1' };
    const transport = makeTransport(doc, runId, { workflowId: 'nested_flow' });
    const header = await transport.readHeader(() => true);
    const chunk = await transport.readChunk(0, 65536, () => true);
    const snapHeaderSpy = vi.fn(() => ({ type: 'workflow_run_snapshot_header' as const, header }));
    server.handlers.set('conversation/workflowRunSnapshotHeader', snapHeaderSpy);
    server.handlers.set('conversation/workflowRunSnapshotChunk', () => ({ type: 'workflow_run_snapshot_chunk', chunk }));

    const total = 70;
    const targetIdx = 68;
    const cut1Nodes: WorkflowTraceNode[] = Array.from({ length: total }, (_, idx) => {
      const base = makeTraceNode(`wn-ref-${idx}`, runId, idx + 1, 0, { type: 'settled', outcome: 'completed' });
      if (idx !== targetIdx) return base;
      return {
        ...base,
        state: { type: 'waiting', reason: 'review' },
        output: null,
        output_preview: null,
        output_omitted: true,
      };
    });
    const cut2OutputRef = {
      type: 'value' as const,
      field: 'result' as const,
      domain: 'out-wn-ref-68-cut2',
      item_offset: 68,
      path: [],
      encoded_bytes: String(nodeBodyBytes('out-wn-ref-68-cut2')),
    };
    const cut2Nodes: WorkflowTraceNode[] = cut1Nodes.map((node, idx) => (idx === targetIdx
      ? {
        ...node,
        state: { type: 'settled', outcome: 'completed' },
        output: cut2OutputRef,
        output_preview: null,
        output_omitted: false,
      }
      : node));

    const traceDomain = 'trace.records.408.workflow.nodes';
    const detailDomain = 'trace.records.408.detail';
    const makeRoot = (rev: string): TraceRecord => ({
      id: 'trace:408',
      position: '408',
      location: { attempt_id: 'a408', step_id: 's1' },
      kind: 'workflow',
      state: rev === '1' ? 'running' : 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: null, duration_ms: '50' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: traceDomain, revision: rev, total: String(total), loaded: '0', next_offset: '0' }],
      native_id: null,
      originating_tool_call_id: null,
      has_detail: true,
      truncated: false,
    });
    const traceRoot1 = makeRoot('1');
    const traceRoot2 = makeRoot('2');

    let refreshHeld = false;
    let releaseCut2Refresh = null as (() => void) | null;
    const cut2PageOffsets: number[] = [];
    const exactLookupCalls: Array<{ cut: CheckpointReadCut; visitKey: string }> = [];
    const valueSpy = vi.fn(async (reqCut: CheckpointReadCut, source: Parameters<TraceFieldReader['value']>[1]): Promise<CheckpointValueChunk> => {
      const bytes = Buffer.from(JSON.stringify(nodeBody(source.domain)), 'utf8');
      return {
        representation: 'canonical_json_v1',
        cut: reqCut,
        domain: source.domain,
        item_offset: source.item_offset,
        path: [...source.path],
        offset: 0,
        bytes_base64: bytes.toString('base64'),
        next_offset: null,
        readable_end: bytes.length,
      };
    });

    const reader: TraceFieldReader = {
      value: valueSpy,
      content: vi.fn(),
      workflowNode: vi.fn(async (reqCut, instance) => {
        exactLookupCalls.push({ cut: reqCut, visitKey: visitIdentityKey(instance) });
        if (reqCut.revision === '2' && visitIdentityKey(instance) === visitIdentityKey(cut2Nodes[targetIdx]!.instance)) {
          return cut2Nodes[targetIdx]!;
        }
        return null;
      }),
      page: vi.fn(async (reqCut, domain, offset): Promise<CheckpointPage> => {
        if (domain === detailDomain) {
          const root = reqCut.revision === '2' ? traceRoot2 : traceRoot1;
          return {
            domain,
            cursor: reqCut.revision,
            read_cut: reqCut,
            total: '1',
            collections: [],
            next_offset: null,
            items: [{
              offset: '0',
              identity: root.id,
              value: {
                id: root.id,
                kind: 'workflow',
                record: root,
                content: { type: 'workflow', run_id: runId, workflow_id: 'nested_flow' },
              } as unknown as Record<string, unknown>,
              content: [],
              deferred: true,
              body_complete: false,
            }],
          };
        }
        const sourceNodes = reqCut.revision === '2' ? cut2Nodes : cut1Nodes;
        const slice = sourceNodes.slice(offset, offset + 32);
        const next = offset + slice.length;
        const page: CheckpointPage = {
          domain,
          cursor: reqCut.revision,
          read_cut: reqCut,
          total: String(total),
          collections: [],
          next_offset: next < total ? String(next) : null,
          items: slice.map((node, i) => ({
            offset: String(offset + i),
            identity: node.id,
            value: node as unknown as Record<string, unknown>,
            content: [],
            deferred: false,
            body_complete: true,
          })),
        };
        if (reqCut.revision === '2' && offset === 0 && !refreshHeld) {
          // First cut2 offset=0 read comes from Trajectory useWorkflowNodes refresh; hold it so old cut1 rows stay visible.
          refreshHeld = true;
          return new Promise(resolve => {
            releaseCut2Refresh = () => resolve(page);
          });
        }
        if (reqCut.revision === '2') cut2PageOffsets.push(offset);
        return page;
      }),
    };

    const cache1: TraceCache = {
      epoch: 1,
      page: { records: [traceRoot1], next_cursor: null, read_cut: cut1 },
      readCuts: { [traceRoot1.id]: cut1 },
      fields: reader,
      details: {},
    };
    const cache2: TraceCache = {
      epoch: 1,
      page: { records: [traceRoot2], next_cursor: null, read_cut: cut2 },
      readCuts: { [traceRoot2.id]: cut2 },
      fields: reader,
      details: {},
    };

    try {
      const ui = render(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="A"
          conversationId={convId}
          attachmentTarget={attTarget}
          ownerCurrent
        >
          <Trajectory cache={cache1} loadEarlier={vi.fn()} onSelect={vi.fn()} onLoadDetail={vi.fn()} />
        </ConversationRunCanvasHost>,
      );

      // Expand workflow row and wait for all 70 cut1 node rows to populate.
      fireEvent.click(ui.container.querySelector('[data-trace-id="trace:408"] button[aria-expanded]')!);
      await waitFor(() => expect(ui.container.querySelector('[data-workflow-node="wn-ref-68"] button')).toBeTruthy());

      // Advance readCut to cut2 and hold Trajectory useWorkflowNodes refresh while old cut1 rows stay rendered.
      ui.rerender(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="A"
          conversationId={convId}
          attachmentTarget={attTarget}
          ownerCurrent
        >
          <Trajectory cache={cache2} loadEarlier={vi.fn()} onSelect={vi.fn()} onLoadDetail={vi.fn()} />
        </ConversationRunCanvasHost>,
      );
      await waitFor(() => expect(releaseCut2Refresh).toBeTypeOf('function'));

      // Open graph directly from late old node row #69 while useWorkflowNodes refresh is still held.
      const lateRowButton = ui.container.querySelector<HTMLElement>('[data-workflow-node="wn-ref-68"] button')!;
      fireEvent.click(lateRowButton);

      const dialog = await screen.findByRole('dialog');
      // Canvas loads only bounded initial cut2 batch (0, 32) and performs exact workflowNode lookup for visit 69 at cut2.
      await waitFor(() => expect(exactLookupCalls).toHaveLength(1));
      expect(cut2PageOffsets).toEqual([0, 32]);
      const lateVisitKey = visitIdentityKey(cut2Nodes[targetIdx]!.instance);
      expect(exactLookupCalls[0]).toEqual({ cut: cut2, visitKey: lateVisitKey });

      // Old cut1 unsettled/omitted header was not retagged at cut2; selected visit stays #69 with completed cut2 header.
      const selectedOption = dialog.querySelector(`[data-run-visit-key="${CSS.escape(lateVisitKey)}"]`);
      expect(selectedOption?.getAttribute('aria-selected')).toBe('true');
      expect(within(dialog).queryByText(/Waiting for/i)).toBeNull();
      expect(valueSpy).not.toHaveBeenCalled();

      // Opening Output reads cut2 + new reference lazily and renders cut2 value (never cut1 output_omitted).
      fireEvent.click(within(dialog).getByRole('tab', { name: 'Output' }));
      await waitFor(() => expect(valueSpy).toHaveBeenCalledTimes(1));
      expect(valueSpy.mock.calls[0]?.[0]).toEqual(cut2);
      expect(valueSpy.mock.calls[0]?.[1]).toEqual(cut2OutputRef);
      await waitFor(() => expect(dialog.textContent).toContain('out-wn-ref-68-cut2'));
      expect(snapHeaderSpy).toHaveBeenCalledTimes(1);
    } finally {
      const finishRefresh = releaseCut2Refresh as (() => void) | null;
      finishRefresh?.();
      server.client.disconnect();
    }
  });

  it('keeps keyboard focus inside the modal when focused Retry trace button vanishes on retry', async () => {
    const doc = makeNestedDoc();
    const runId = baseRunId('409', 'conv-trace-focus');
    const cut: CheckpointReadCut = { conversation_id: 'conv-trace-focus', revision: '1', through: '1', domain_revisions: {}, collection_cuts: {} };
    const transport = makeTransport(doc, runId, { hasSourcePresentation: false });
    const traceDomain = 'trace.records.409.workflow.nodes';
    const traceRoot: TraceRecord = {
      id: 'trace:409',
      position: '409',
      location: { attempt_id: 'a409', step_id: 's1' },
      kind: 'workflow',
      state: 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '50' },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: traceDomain, revision: '1', total: '2', loaded: '0', next_offset: '0' }],
      native_id: JSON.stringify(runId),
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    };
    const nodes = [
      makeTraceNode('wn-focus-0', runId, 1, 0, { type: 'settled', outcome: 'completed' }),
      makeTraceNode('wn-focus-1', runId, 2, 0, { type: 'settled', outcome: 'completed' }),
    ];

    let attempt = 0;
    let releaseRetry!: (page: CheckpointPage) => void;
    const reader: TraceFieldReader = {
      value: vi.fn(),
      content: vi.fn(),
      page: vi.fn(async (reqCut, domain) => {
        attempt += 1;
        if (attempt === 1) throw new Error('initial trace failure');
        return new Promise<CheckpointPage>(resolve => {
          releaseRetry = resolve;
          void reqCut;
          void domain;
        });
      }),
    };

    render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-trace-focus',
          runId,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          traceRoot,
          traceCut: cut,
          traceReader: reader,
          transport,
        }}
        onClose={vi.fn()}
      />,
    );

    const dialog = await screen.findByRole('dialog');
    const retryTraceBtn = await screen.findByRole('button', { name: 'Retry trace' });
    retryTraceBtn.focus();
    expect(document.activeElement).toBe(retryTraceBtn);

    fireEvent.click(retryTraceBtn);
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Retry trace' })).toBeNull());
    // While retrying (loading state after Retry trace unmounts), focus must remain inside the modal, not body.
    expect(document.activeElement).not.toBe(document.body);
    expect(dialog.contains(document.activeElement)).toBe(true);

    await act(async () => {
      releaseRetry({
        domain: traceDomain,
        cursor: cut.revision,
        read_cut: cut,
        total: '2',
        collections: [],
        next_offset: null,
        items: nodes.map((node, i) => ({
          offset: String(i),
          identity: node.id,
          value: node as unknown as Record<string, unknown>,
          content: [],
          deferred: false,
          body_complete: true,
        })),
      });
    });
    expect(document.activeElement).not.toBe(document.body);
    expect(dialog.contains(document.activeElement)).toBe(true);
  });

  it('preserves explicit late selected visit without falling back to sibling I/O, selection, or child ref across pending, error, and null exact lookups', async () => {
    const doc = makeNestedDoc();
    if (doc.block.nodes.outer.type === 'loop') doc.block.nodes.outer.loop_count = 70;
    const run = baseRunId('70', 'conv-strict-visit');
    const cut1: CheckpointReadCut = { conversation_id: 'conv-strict-visit', revision: '1', through: '1', domain_revisions: {}, collection_cuts: {}, activation_id: 'act-1' };
    const cut2: CheckpointReadCut = { conversation_id: 'conv-strict-visit', revision: '2', through: '2', domain_revisions: {}, collection_cuts: {}, activation_id: 'act-1' };
    const cut3: CheckpointReadCut = { conversation_id: 'conv-strict-visit', revision: '3', through: '3', domain_revisions: {}, collection_cuts: {}, activation_id: 'act-1' };
    const cut4: CheckpointReadCut = { conversation_id: 'conv-strict-visit', revision: '4', through: '4', domain_revisions: {}, collection_cuts: {}, activation_id: 'act-1' };
    const transport = makeTransport(doc, run, { hasSourcePresentation: false });

    const makeReviewNode = (
      iter: number,
      rev: string,
      opts?: { childAgentId?: string; childActivationId?: string },
    ): WorkflowTraceNode => {
      const outputDomain = `out-review-${iter}`;
      const encodedBytes = String(new TextEncoder().encode(JSON.stringify(`${outputDomain}/rev${rev}`)).length);
      return {
        id: `wn-${iter}`,
        instance: {
          block: { run, definition: { workflow_id: 'nested_flow', blocks: ['outer', 'body'] }, invocations: [0, iter] },
          node: 'worker',
          visit: 0,
        },
        kind: 'template',
        state: { type: 'settled', outcome: 'completed' },
        timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '10' },
        input: null,
        output: { type: 'value', field: 'result', domain: outputDomain, item_offset: 0, path: [], encoded_bytes: encodedBytes },
        input_preview: null,
        output_preview: null,
        input_omitted: false,
        output_omitted: false,
        ...(opts?.childActivationId ? { child: opts.childActivationId, child_agent_id: opts.childAgentId } : {}),
      };
    };

    const visit69Cut1 = makeReviewNode(68, '1');
    const visit69Cut2 = makeReviewNode(68, '2', { childAgentId: 'child-agent-68', childActivationId: 'child-act-68' });
    const traceRoot: TraceRecord = {
      id: 'trace:700',
      position: '700',
      location: { attempt_id: 'att-70', step_id: 'step-1' },
      kind: 'workflow',
      state: 'running',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: null, duration_ms: null },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: 'trace.records.700.workflow.nodes', revision: '1', total: '70', loaded: '0', next_offset: '0' }],
      native_id: null,
      originating_tool_call_id: null,
      has_detail: true,
      truncated: false,
    };
    const makePrefixPage = (cut: CheckpointReadCut, offset: number): CheckpointPage => {
      const start = offset === 0 ? 0 : 32;
      const slice = Array.from({ length: 32 }, (_, idx) => {
        const iter = start + idx;
        return makeReviewNode(
          iter,
          cut.revision,
          iter === 63 ? { childAgentId: 'child-agent-63', childActivationId: 'child-act-63' } : undefined,
        );
      });
      return {
        domain: 'trace.records.700.workflow.nodes',
        cursor: cut.revision,
        read_cut: cut,
        total: '70',
        collections: [],
        next_offset: offset === 0 ? '32' : '64',
        items: slice.map((node, i) => ({
          offset: String(offset + i),
          identity: node.id,
          value: node as unknown as Record<string, unknown>,
          content: [],
          deferred: false,
          body_complete: true,
        })),
      };
    };

    const valueCalls: string[] = [];
    const exactLookupCalls: Array<{ rev: string; visitKey: string }> = [];
    let releaseCut2Lookup!: (node: WorkflowTraceNode | null) => void;
    let rejectCut3Lookup!: (err: Error) => void;
    let releaseCut4Lookup!: (node: WorkflowTraceNode | null) => void;

    const makeReader = (): TraceFieldReader => ({
      page: vi.fn(async (cut, _domain, offset) => makePrefixPage(cut, offset)),
      workflowNode: vi.fn(async (cut, instance) => {
        exactLookupCalls.push({ rev: cut.revision, visitKey: visitIdentityKey(instance) });
        if (cut.revision === '1') return visit69Cut1;
        if (cut.revision === '2') {
          return new Promise<WorkflowTraceNode | null>(resolve => { releaseCut2Lookup = resolve; });
        }
        if (cut.revision === '3') {
          return new Promise<WorkflowTraceNode | null>((_resolve, reject) => { rejectCut3Lookup = reject; });
        }
        return new Promise<WorkflowTraceNode | null>(resolve => { releaseCut4Lookup = resolve; });
      }),
      value: vi.fn(async (cut, field) => {
        valueCalls.push(`${field.domain}:${cut.revision}`);
        const payload = `${field.domain}/rev${cut.revision}`;
        const bytes = new TextEncoder().encode(JSON.stringify(payload));
        let bin = '';
        for (const b of bytes) bin += String.fromCharCode(b);
        return {
          representation: 'canonical_json_v1' as const,
          cut,
          ...field,
          offset: 0,
          bytes_base64: btoa(bin),
          next_offset: null,
          readable_end: bytes.length,
        };
      }),
      content: vi.fn(),
    });
    const reader1 = makeReader();
    const reader2 = makeReader();
    const reader3 = makeReader();
    const reader4 = makeReader();
    const openWorkflowSpy = vi.fn();
    const subScope = {
      client: new Server().client,
      agents: [],
      metrics: {},
      metricErrors: {},
      opened: [],
      openWorkflow: openWorkflowSpy,
      close: vi.fn(),
      open: vi.fn(),
    };

    try {
      const ui = render(
        <SubagentContext.Provider value={subScope}>
          <WorkflowRunCanvas
            target={{
              conversationId: 'conv-strict-visit',
              runId: run,
              workflowId: 'nested_flow',
              sourceMode: 'history',
              selectedVisit: visit69Cut1.instance,
              selectedTraceNode: visit69Cut1,
              traceRoot,
              traceCut: cut1,
              traceReader: reader1,
              transport,
            }}
            onClose={vi.fn()}
            organizeFn={organizeDocument}
          />
        </SubagentContext.Provider>,
      );

      await waitFor(() => expect(reader1.page).toHaveBeenCalledTimes(2));
      fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
      await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('out-review-68/rev1'));
      expect(valueCalls).toEqual(['out-review-68:1']);

      // Advance to cut2 (initial 64 loaded, missing iter 68) while exact workflowNode(cut2, visit69) is HELD.
      ui.rerender(
        <SubagentContext.Provider value={subScope}>
          <WorkflowRunCanvas
            target={{
              conversationId: 'conv-strict-visit',
              runId: run,
              workflowId: 'nested_flow',
              sourceMode: 'history',
              selectedVisit: visit69Cut1.instance,
              selectedTraceNode: visit69Cut1,
              traceRoot,
              traceCut: cut1,
              traceReader: reader1,
              transport,
            }}
            resolvedTrace={{ traceRoot, traceCut: cut2, traceReader: reader2 }}
            traceStatus="ready"
            onClose={vi.fn()}
            organizeFn={organizeDocument}
          />
        </SubagentContext.Provider>,
      );

      await waitFor(() => expect(reader2.page).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(releaseCut2Lookup).toBeTypeOf('function'));
      expect(exactLookupCalls).toContainEqual({ rev: '2', visitKey: visitIdentityKey(visit69Cut1.instance) });

      // Sibling body out-review-63 MUST NOT be requested, and NO sibling visit option may be marked selected.
      expect(valueCalls).toEqual(['out-review-68:1']);
      expect(screen.getByRole('tabpanel').textContent).not.toContain('out-review-63');
      const selectedOptionsDuringPending = screen
        .getAllByRole('option')
        .filter(el => el.getAttribute('aria-selected') === 'true');
      expect(selectedOptionsDuringPending).toHaveLength(0);
      expect(screen.getByRole('tab', { name: 'Output' }).getAttribute('aria-selected')).toBe('true');
      expect(screen.queryByRole('button', { name: 'Open subagent conversation' })).toBeNull();

      // Release exact lookup for visit 69 at cut2 -> ONLY out-review-68:2 read & displayed.
      await act(async () => { releaseCut2Lookup(visit69Cut2); });
      await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('out-review-68/rev2'));
      expect(valueCalls).toEqual(['out-review-68:1', 'out-review-68:2']);
      const visit69Option = document.querySelector(`[data-run-visit-key="${CSS.escape(visitIdentityKey(visit69Cut1.instance))}"]`);
      expect(visit69Option?.getAttribute('aria-selected')).toBe('true');

      // Summary shows exact visit 69 childRef (never sibling 64).
      fireEvent.click(screen.getByRole('tab', { name: 'Summary' }));
      const openChildBtn = await screen.findByRole('button', { name: 'Open subagent conversation' });
      fireEvent.click(openChildBtn);
      expect(openWorkflowSpy).toHaveBeenLastCalledWith('child-agent-68', 'child-act-68', 'worker');

      // Exact lookup ERROR at cut3 while Output tab is open -> no sibling fallback, no sibling childRef, alert visible.
      fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
      await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('out-review-68/rev2'));
      const callsBeforeCut3 = valueCalls.length;
      ui.rerender(
        <SubagentContext.Provider value={subScope}>
          <WorkflowRunCanvas
            target={{
              conversationId: 'conv-strict-visit',
              runId: run,
              workflowId: 'nested_flow',
              sourceMode: 'history',
              selectedVisit: visit69Cut1.instance,
              selectedTraceNode: visit69Cut1,
              traceRoot,
              traceCut: cut1,
              traceReader: reader1,
              transport,
            }}
            resolvedTrace={{ traceRoot, traceCut: cut3, traceReader: reader3 }}
            traceStatus="ready"
            onClose={vi.fn()}
            organizeFn={organizeDocument}
          />
        </SubagentContext.Provider>,
      );
      await waitFor(() => expect(rejectCut3Lookup).toBeTypeOf('function'));
      await act(async () => { rejectCut3Lookup(new Error('Exact node lookup failed')); });
      await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('Exact node lookup failed'));
      expect(valueCalls.slice(callsBeforeCut3)).toEqual([]);
      expect(screen.getAllByRole('option').filter(el => el.getAttribute('aria-selected') === 'true')).toHaveLength(0);
      expect(screen.getByRole('tab', { name: 'Output' }).getAttribute('aria-selected')).toBe('true');
      fireEvent.click(screen.getByRole('tab', { name: 'Summary' }));
      expect(screen.queryByRole('button', { name: 'Open subagent conversation' })).toBeNull();

      // Exact lookup NULL at cut4 while Output tab is open -> no sibling fallback, unavailable shown, no sibling childRef.
      fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
      const callsBeforeCut4 = valueCalls.length;
      ui.rerender(
        <SubagentContext.Provider value={subScope}>
          <WorkflowRunCanvas
            target={{
              conversationId: 'conv-strict-visit',
              runId: run,
              workflowId: 'nested_flow',
              sourceMode: 'history',
              selectedVisit: visit69Cut1.instance,
              selectedTraceNode: visit69Cut1,
              traceRoot,
              traceCut: cut1,
              traceReader: reader1,
              transport,
            }}
            resolvedTrace={{ traceRoot, traceCut: cut4, traceReader: reader4 }}
            traceStatus="ready"
            onClose={vi.fn()}
            organizeFn={organizeDocument}
          />
        </SubagentContext.Provider>,
      );
      await waitFor(() => expect(releaseCut4Lookup).toBeTypeOf('function'));
      await act(async () => { releaseCut4Lookup(null); });
      await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('unavailable at this read cut'));
      expect(valueCalls.slice(callsBeforeCut4)).toEqual([]);
      expect(valueCalls.some(v => v.includes('out-review-63'))).toBe(false);
      expect(screen.getAllByRole('option').filter(el => el.getAttribute('aria-selected') === 'true')).toHaveLength(0);
      expect(screen.getByRole('tab', { name: 'Output' }).getAttribute('aria-selected')).toBe('true');
      fireEvent.click(screen.getByRole('tab', { name: 'Summary' }));
      expect(screen.queryByRole('button', { name: 'Open subagent conversation' })).toBeNull();
    } finally {
      subScope.client.disconnect();
    }
  });

  it('retains pinned explicit late selectedVisit when clicking the same qualified canvas node before overlay commits while allowing switches to other nodes', async () => {
    const doc = makeNestedDoc();
    if (doc.block.nodes.outer.type === 'loop') doc.block.nodes.outer.loop_count = 70;
    const run = baseRunId('71', 'conv-same-node-click');
    const cut: CheckpointReadCut = { conversation_id: 'conv-same-node-click', revision: '1', through: '1', domain_revisions: {}, collection_cuts: {}, activation_id: 'act-1' };
    const transport = makeTransport(doc, run, { hasSourcePresentation: false });

    const makeNode = (iter: number, nodeId = 'worker'): WorkflowTraceNode => {
      const outputDomain = `out-${nodeId}-${iter}`;
      const encodedBytes = String(new TextEncoder().encode(JSON.stringify(outputDomain)).length);
      return {
        id: `wn-${nodeId}-${iter}`,
        instance: {
          block: { run, definition: { workflow_id: 'nested_flow', blocks: ['outer', 'body'] }, invocations: [0, iter] },
          node: nodeId,
          visit: 0,
        },
        kind: nodeId === 'childStep' ? 'agent' : 'template',
        state: { type: 'settled', outcome: 'completed' },
        timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '10' },
        input: null,
        output: { type: 'value', field: 'result', domain: outputDomain, item_offset: 0, path: [], encoded_bytes: encodedBytes },
        input_preview: null,
        output_preview: null,
        input_omitted: false,
        output_omitted: false,
      };
    };

    const iter69 = makeNode(68);
    const traceRoot: TraceRecord = {
      id: 'trace:710',
      position: '710',
      location: { attempt_id: 'att-71', step_id: 'step-1' },
      kind: 'workflow',
      state: 'completed',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: null, duration_ms: null },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: 'trace.records.710.workflow.nodes', revision: '1', total: '70', loaded: '0', next_offset: '0' }],
      native_id: null,
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    };

    const prefix64 = [
      ...Array.from({ length: 63 }, (_, i) => makeNode(i)),
      makeNode(63, 'childStep'),
    ];
    let releaseFirstPage!: () => void;
    let releaseExact69!: (node: WorkflowTraceNode | null) => void;
    const exactLookups: string[] = [];
    const valueCalls: string[] = [];

    const reader: TraceFieldReader = {
      page: vi.fn(async (_reqCut, _domain, offset) => {
        const items = prefix64.slice(offset, offset + 32).map((node, i) => ({
          offset: String(offset + i),
          identity: node.id,
          value: node as unknown as Record<string, unknown>,
          content: [],
          deferred: false,
          body_complete: true,
        }));
        const page: CheckpointPage = {
          domain: 'trace.records.710.workflow.nodes',
          cursor: '1',
          read_cut: cut,
          total: '70',
          collections: [],
          next_offset: offset === 0 ? '32' : '64',
          items,
        };
        if (offset === 0) {
          return new Promise<CheckpointPage>(resolve => { releaseFirstPage = () => resolve(page); });
        }
        return page;
      }),
      workflowNode: vi.fn(async (_reqCut, instance) => {
        exactLookups.push(visitIdentityKey(instance));
        return new Promise<WorkflowTraceNode | null>(resolve => { releaseExact69 = resolve; });
      }),
      value: vi.fn(async (reqCut, field) => {
        valueCalls.push(field.domain);
        const bytes = new TextEncoder().encode(JSON.stringify(field.domain));
        let bin = '';
        for (const b of bytes) bin += String.fromCharCode(b);
        return {
          representation: 'canonical_json_v1' as const,
          cut: reqCut,
          ...field,
          offset: 0,
          bytes_base64: btoa(bin),
          next_offset: null,
          readable_end: bytes.length,
        };
      }),
      content: vi.fn(),
    };

    render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-same-node-click',
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          selectedVisit: iter69.instance,
          traceRoot,
          traceCut: cut,
          traceReader: reader,
          transport,
        }}
        onClose={vi.fn()}
        organizeFn={organizeDocument}
      />,
    );

    const workerId = viewId([{ node: 'outer' }], 'node', 'worker');
    const childStepId = viewId([{ node: 'outer' }], 'node', 'childStep');
    const workerCanvasNode = await waitFor(() => {
      const el = document.querySelector(`.react-flow__node[data-id="${workerId}"]`);
      expect(el).not.toBeNull();
      return el!;
    });

    // Click THE SAME qualified canvas node while initial batch is STILL HELD (no overlay visits yet!).
    await waitFor(() => expect(releaseFirstPage).toBeTypeOf('function'));
    fireEvent.click(workerCanvasNode);

    // Release initial 64 pages; exact lookup MUST STILL target iter69 (not cleared to default sibling).
    await act(async () => { releaseFirstPage(); });
    await waitFor(() => expect(releaseExact69).toBeTypeOf('function'));
    expect(exactLookups).toEqual([visitIdentityKey(iter69.instance)]);

    // Click THE SAME qualified canvas node again while iter69 is still missing from overlay -> MUST remain pinned to iter69!
    fireEvent.click(workerCanvasNode);
    await act(async () => { releaseExact69(iter69); });

    fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
    await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('out-worker-68'));
    expect(valueCalls).toEqual(['out-worker-68']);

    // Explicitly switch to Visit #1 (iteration 1 -> iter 1 in initial prefix), then click same worker node -> stays Visit #1 (never reverts to target.selectedVisit iter69!).
    const iter1Key = visitIdentityKey(prefix64[1]!.instance);
    const iter1Option = document.querySelector(`[data-run-visit-key="${CSS.escape(iter1Key)}"]`) as HTMLElement;
    expect(iter1Option).not.toBeNull();
    fireEvent.click(iter1Option);
    await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('out-worker-1'));
    fireEvent.click(workerCanvasNode);
    fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
    await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('out-worker-1'));

    // Clicking a DIFFERENT qualified canvas node (childStep) switches away to childStep's default visit cleanly.
    const childStepCanvasNode = document.querySelector(`.react-flow__node[data-id="${childStepId}"]`)!;
    fireEvent.click(childStepCanvasNode);
    fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
    await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('out-childStep-63'));
  });

  it('retains coherent committed and preloaded trace content with visible error and working Retry across real Host frontier refresh failures', async () => {
    const server = new Server();
    await server.attached('A');
    const attTarget = server.client.getSnapshot().views.A!.target!;
    const convId = attTarget.conversation_id;
    const doc = makeNestedDoc();
    const run = baseRunId('801', convId);
    const cut10: CheckpointReadCut = { conversation_id: convId, revision: '10', through: '10', domain_revisions: {}, collection_cuts: {}, activation_id: null };
    const cut11: CheckpointReadCut = { conversation_id: convId, revision: '11', through: '11', domain_revisions: {}, collection_cuts: {}, activation_id: null };

    const traceDomain10 = 'trace.records.801.workflow.nodes';
    const traceDomain11 = 'trace.records.801b.workflow.nodes';
    const root10: TraceRecord = {
      id: 'trace:801',
      position: '801',
      location: { attempt_id: 'a-10', step_id: 's1' },
      kind: 'workflow',
      state: 'running',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: null, duration_ms: null },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: traceDomain10, revision: '10', total: '1', loaded: '0', next_offset: '0' }],
      native_id: JSON.stringify(run),
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    };
    const root11: TraceRecord = {
      ...root10,
      state: 'completed',
      collections: [{ domain: traceDomain11, revision: '11', total: '1', loaded: '0', next_offset: '0' }],
    };
    const node10: WorkflowTraceNode = {
      id: 'wn-10',
      instance: { block: { run, definition: { workflow_id: 'nested_flow', blocks: ['outer', 'body'] }, invocations: [0, 1] }, node: 'worker', visit: 0 },
      kind: 'template',
      state: { type: 'running' },
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: null, duration_ms: null },
      input: null,
      output: null,
      input_preview: null,
      output_preview: null,
      input_omitted: false,
      output_omitted: false,
    };
    const out11Bytes = new TextEncoder().encode(JSON.stringify('cut11-output-payload'));
    const node11: WorkflowTraceNode = {
      ...node10,
      state: { type: 'settled', outcome: 'completed' },
      output: { type: 'value', field: 'result', domain: 'out-cut11', item_offset: 0, path: [], encoded_bytes: String(out11Bytes.length) },
    };

    const snapTransport = makeTransport(doc, run, { hasSourcePresentation: false });
    const snapHeader = await snapTransport.readHeader(() => true);
    const snapChunk = await snapTransport.readChunk(0, 65536, () => true);
    server.handlers.set('conversation/workflowRunSnapshotHeader', () => ({
      type: 'workflow_run_snapshot_header' as const,
      header: snapHeader,
    }));
    server.handlers.set('conversation/workflowRunSnapshotChunk', () => ({
      type: 'workflow_run_snapshot_chunk' as const,
      chunk: snapChunk,
    }));

    let locateCalls = 0;
    server.held.add('session/traceLocateWorkflow');
    const locateOutcomes = new Map<string, () => MethodResult>();
    server.handlers.set('session/traceLocateWorkflow', request => {
      if (request.method !== 'session/traceLocateWorkflow') throw new Error('unexpected method');
      const outcome = locateOutcomes.get(String(request.id));
      if (!outcome) throw new Error('missing locate outcome');
      return outcome();
    });
    server.handlers.set('conversation/checkpointPage', request => {
      if (request.method !== 'conversation/checkpointPage') throw new Error('unexpected method');
      const isCut11 = request.params.cut.revision === '11';
      const node = isCut11 ? node11 : node10;
      return {
        type: 'checkpoint_page' as const,
        page: {
          domain: request.params.domain,
          cursor: request.params.cut.revision,
          read_cut: request.params.cut,
          total: '1',
          collections: [],
          next_offset: null,
          items: [{ offset: '0', identity: node.id, value: node as unknown as Record<string, unknown>, content: [], deferred: false, body_complete: true }],
        },
      };
    });
    server.handlers.set('conversation/checkpointValueRead', request => {
      if (request.method !== 'conversation/checkpointValueRead') throw new Error('unexpected method');
      return {
        type: 'checkpoint_value_chunk' as const,
        chunk: {
          representation: 'canonical_json_v1' as const,
          cut: request.params.cut,
          domain: request.params.domain,
          item_offset: request.params.item_offset,
          path: request.params.path,
          offset: 0,
          bytes_base64: Buffer.from(out11Bytes).toString('base64'),
          next_offset: null,
          readable_end: out11Bytes.length,
        },
      };
    });

    const liveRun: WorkflowRunView = {
      id: run,
      workflow_id: 'nested_flow',
      program_digest: 'sha256:host-refresh',
      resource_revision: '1',
      state: { type: 'running' },
      instances: [],
      omitted_instances: 0,
      steps_consumed: 1,
      steps_max: 4096,
      agents_consumed: 0,
      model_calls_consumed: 0,
    };

    try {
      const ui = render(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="A"
          conversationId={convId}
          attachmentTarget={attTarget}
          ownerCurrent
          liveCheckpointCut={cut10}
          liveRuns={[liveRun]}
        >
          <WorkflowRunPanel run={liveRun} />
        </ConversationRunCanvasHost>,
      );

      fireEvent.click(ui.getByRole('button', { name: 'View run graph' }));
      const req1 = await server.waitFor('session/traceLocateWorkflow', ++locateCalls);
      locateOutcomes.set(String(req1.id), () => ({
        type: 'trace_workflow_location',
        location: { record_id: root10.id, page: { read_cut: cut10, records: [root10], updates: [], next_cursor: null } },
      }));
      await act(async () => { server.reply(req1); });

      const workerId = viewId([{ node: 'outer' }], 'node', 'worker');
      const workerEl = await waitFor(() => {
        const el = document.querySelector(`.react-flow__node[data-id="${workerId}"]`);
        expect(el).not.toBeNull();
        return el!;
      });
      const flowBefore = screen.getByRole('dialog').querySelector('.react-flow');
      fireEvent.click(workerEl);
      fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
      await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('Output not recorded'));

      // Advance frontier to cut11 -> reject locate -> coherent cut10 MUST remain visible AND error + Retry trace MUST appear!
      ui.rerender(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="A"
          conversationId={convId}
          attachmentTarget={attTarget}
          ownerCurrent
          liveCheckpointCut={cut11}
          liveRuns={[{ ...liveRun, state: { type: 'settled', outcome: 'completed' } }]}
        >
          <WorkflowRunPanel run={liveRun} />
        </ConversationRunCanvasHost>,
      );
      const req2 = await server.waitFor('session/traceLocateWorkflow', ++locateCalls);
      locateOutcomes.set(String(req2.id), () => {
        throw new RpcFailure({ code: -32000, message: 'Frontier cut 11 locate failed' });
      });
      await act(async () => { server.reply(req2); });

      const retryBtn = await screen.findByRole('button', { name: 'Retry trace' });
      expect(screen.getByRole('alert').textContent).toContain('Frontier cut 11 locate failed');
      expect(screen.getByRole('dialog').querySelector('.react-flow')).toBe(flowBefore);
      expect(screen.getByRole('tab', { name: 'Output' }).getAttribute('aria-selected')).toBe('true');
      expect(screen.getByRole('tabpanel').textContent).toContain('Output not recorded');

      // Click Retry without advancing frontier -> locate succeeds cut11 -> error clears & cut11 Output loads cleanly.
      retryBtn.focus();
      fireEvent.click(retryBtn);
      const req3 = await server.waitFor('session/traceLocateWorkflow', ++locateCalls);
      locateOutcomes.set(String(req3.id), () => ({
        type: 'trace_workflow_location',
        location: { record_id: root11.id, page: { read_cut: cut11, records: [root11], updates: [], next_cursor: null } },
      }));
      await act(async () => { server.reply(req3); });

      await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('cut11-output-payload'));
      expect(screen.queryByRole('button', { name: 'Retry trace' })).toBeNull();
      expect(screen.getByRole('dialog').querySelector('.react-flow')).toBe(flowBefore);
      expect(screen.getByRole('dialog').contains(document.activeElement)).toBe(true);
      expect(server.requests.filter(r => r.request.method === 'conversation/workflowRunSnapshotHeader')).toHaveLength(1);
      expect(server.requests.filter(r => r.request.method === 'conversation/workflowRunSnapshotChunk')).toHaveLength(1);

      // Part 2: Trajectory complete preloaded trace opener (targetHasCompleteTrace = true) + frontier refresh failure & Retry.
      fireEvent.click(screen.getByRole('button', { name: 'Close run graph' }));
      let openComplete!: (target: WorkflowRunCanvasTarget) => void;
      function Bridge({ onReady }: { onReady: (fn: (target: WorkflowRunCanvasTarget) => void) => void }) {
        const ctx = useContext(WorkflowRunCanvasContext);
        onReady(target => ctx?.openRunCanvas(target));
        return null;
      }
      const reader10: TraceFieldReader = {
        page: vi.fn(),
        value: vi.fn(),
        content: vi.fn(),
      };
      ui.rerender(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="A"
          conversationId={convId}
          attachmentTarget={attTarget}
          ownerCurrent
          liveCheckpointCut={cut10}
          liveRuns={[liveRun]}
        >
          <Bridge onReady={fn => { openComplete = fn; }} />
        </ConversationRunCanvasHost>,
      );
      act(() => {
        openComplete({
          conversationId: convId,
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'history',
          traceRoot: root10,
          traceCut: cut10,
          traceReader: reader10,
          traceNodes: [node10],
          selectedVisit: node10.instance,
          selectedTraceNode: node10,
        });
      });
      await waitFor(() => expect(screen.getByRole('dialog')).toBeTruthy());
      const completeCallsBefore = locateCalls;

      // Advance frontier to cut11 -> targetHasCompleteTrace=true still surfaces locate failure + Retry trace!
      ui.rerender(
        <ConversationRunCanvasHost
          client={server.client}
          sessionId="A"
          conversationId={convId}
          attachmentTarget={attTarget}
          ownerCurrent
          liveCheckpointCut={cut11}
          liveRuns={[liveRun]}
        >
          <Bridge onReady={fn => { openComplete = fn; }} />
        </ConversationRunCanvasHost>,
      );
      const req4 = await server.waitFor('session/traceLocateWorkflow', completeCallsBefore + 1);
      locateOutcomes.set(String(req4.id), () => {
        throw new RpcFailure({ code: -32000, message: 'Preloaded trace refresh failed' });
      });
      await act(async () => { server.reply(req4); });

      const retryCompleteBtn = await screen.findByRole('button', { name: 'Retry trace' });
      expect(screen.getByRole('alert').textContent).toContain('Preloaded trace refresh failed');

      fireEvent.click(retryCompleteBtn);
      const req5 = await server.waitFor('session/traceLocateWorkflow', completeCallsBefore + 2);
      locateOutcomes.set(String(req5.id), () => ({
        type: 'trace_workflow_location',
        location: { record_id: root11.id, page: { read_cut: cut11, records: [root11], updates: [], next_cursor: null } },
      }));
      await act(async () => { server.reply(req5); });
      fireEvent.click(screen.getByRole('tab', { name: 'Output' }));
      await waitFor(() => expect(screen.getByRole('tabpanel').textContent).toContain('cut11-output-payload'));
      expect(screen.queryByRole('button', { name: 'Retry trace' })).toBeNull();
    } finally {
      server.client.disconnect();
    }
  });

  it('does not let an earlier loader batch error hide or misdirect retry of a later Host frontier refresh failure', async () => {
    const doc = makeNestedDoc();
    const run = baseRunId('802', 'conv-batch-vs-host');
    const cut10: CheckpointReadCut = { conversation_id: 'conv-batch-vs-host', revision: '10', through: '10', domain_revisions: {}, collection_cuts: {}, activation_id: null };
    const cut11: CheckpointReadCut = { conversation_id: 'conv-batch-vs-host', revision: '11', through: '11', domain_revisions: {}, collection_cuts: {}, activation_id: null };
    const transport = makeTransport(doc, run, { hasSourcePresentation: false });

    const traceRoot10: TraceRecord = {
      id: 'trace:802',
      position: '802',
      location: { attempt_id: 'a-802', step_id: 's1' },
      kind: 'workflow',
      state: 'running',
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: null, duration_ms: null },
      calls: [],
      attachments: [],
      fields: [],
      collections: [{ domain: 'trace.records.802.workflow.nodes', revision: '10', total: '2', loaded: '0', next_offset: '0' }],
      native_id: JSON.stringify(run),
      originating_tool_call_id: null,
      has_detail: false,
      truncated: false,
    };
    const reader10: TraceFieldReader = {
      page: vi.fn(async () => { throw new Error('Old cut10 batch failed'); }),
      value: vi.fn(),
      content: vi.fn(),
    };
    const onRetryHost = vi.fn();

    const ui = render(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-batch-vs-host',
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'live',
          transport,
        }}
        resolvedTrace={{ traceRoot: traceRoot10, traceCut: cut10, traceReader: reader10 }}
        traceStatus="ready"
        onRetryTrace={onRetryHost}
        onClose={vi.fn()}
        organizeFn={organizeDocument}
      />,
    );

    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('Old cut10 batch failed'));

    // Later Host frontier refresh to cut11 fails -> alert MUST surface Host error & clicking Retry MUST invoke onRetryHost!
    ui.rerender(
      <WorkflowRunCanvas
        target={{
          conversationId: 'conv-batch-vs-host',
          runId: run,
          workflowId: 'nested_flow',
          sourceMode: 'live',
          transport,
        }}
        resolvedTrace={{ traceRoot: traceRoot10, traceCut: cut10, traceReader: reader10 }}
        traceStatus="error"
        traceError="Latest host refresh cut11 failed"
        onRetryTrace={onRetryHost}
        onClose={vi.fn()}
        organizeFn={organizeDocument}
      />,
    );

    expect(screen.getByRole('alert').textContent).toContain('Latest host refresh cut11 failed');
    fireEvent.click(screen.getByRole('button', { name: 'Retry trace' }));
    expect(onRetryHost).toHaveBeenCalledTimes(1);
    expect(reader10.page).toHaveBeenCalledTimes(1);
    void cut11;
  });
});
