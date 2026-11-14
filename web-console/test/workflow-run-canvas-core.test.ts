import { createHash } from 'node:crypto';
import { describe, expect, it, vi } from 'vitest';
import type {
  CheckpointPage,
  CheckpointReadCut,
  TraceRecord,
  WorkflowFileDocument,
  WorkflowInstanceView,
  WorkflowRunId,
  WorkflowRunView,
  WorkflowTraceNode,
} from '../../protocol/app-server/v78';
import { boundedDocument } from '../src/app/workflows/document/commands';
import { organizeDocument } from '../src/app/workflows/ui/editor/layout';
import { RpcFailure } from '../src/client/errors';
import type { TraceFieldReader } from '../src/client/trace';
import { projectScene } from '../src/app/workflows/ui/editor/scene';
import { viewId } from '../src/app/workflows/ui/editor/selection';
import {
  blockPathFromDefinition,
  qualifiedNodeViewId,
  resolveSceneNodeId,
  sameWorkflowRunId,
  visitIdentityKey,
} from '../src/app/workflows/run-canvas/identity';
import {
  aggregateRunOverlay,
  statusOfTraceState,
} from '../src/app/workflows/run-canvas/aggregation';
import type {
  WorkflowRunSnapshotChunk,
  WorkflowRunSnapshotHeader,
  WorkflowRunSnapshotTransport,
} from '../src/app/workflows/run-canvas/contract';
import { readWorkflowRunSnapshot, RunPresentationCache } from '../src/app/workflows/run-canvas/snapshot-reader';
import {
  readBoundedTraceWorkflowNodes,
  WorkflowRunSnapshotSession,
  type RunSnapshotPhase,
} from '../src/app/workflows/run-canvas/loader';
import { exampleSource } from './workflow-fixtures';
import { runBoundsDocument, longRunVisits } from './workflow-run-bounds';
import { workflowRun } from './workflow-data';

function encodeSnapshotTransport(
  doc: WorkflowFileDocument,
  runId: WorkflowRunId,
  options: {
    chunkSize?: number;
    hasSourcePresentation?: boolean;
    corruptHash?: boolean;
    headerNull?: boolean;
  } = {},
): { header: WorkflowRunSnapshotHeader; transport: WorkflowRunSnapshotTransport; chunkReads: number[] } {
  const bytes = new TextEncoder().encode(JSON.stringify(doc));
  const sha256 = options.corruptHash
    ? '0'.repeat(64)
    : createHash('sha256').update(bytes).digest('hex');
  const header: WorkflowRunSnapshotHeader = {
    run_id: runId,
    workflow_id: 'test_flow',
    program_digest: 'prog-digest-1',
    source_revision: 'rev-42',
    snapshot_digest: 'snap-digest-42',
    has_source_presentation: options.hasSourcePresentation ?? true,
    byte_length: bytes.length,
    sha256,
  };
  const chunkSize = options.chunkSize ?? 65_536;
  const chunkReads: number[] = [];
  const transport: WorkflowRunSnapshotTransport = {
    async readHeader(current) {
      if (!current()) throw new Error('Cancelled');
      return options.headerNull ? null : header;
    },
    async readChunk(offset, limit, current) {
      if (!current()) throw new Error('Cancelled');
      chunkReads.push(offset);
      const size = Math.min(chunkSize, limit, bytes.length - offset);
      if (size <= 0) return null;
      const slice = bytes.subarray(offset, offset + size);
      let binary = '';
      for (const byte of slice) binary += String.fromCharCode(byte);
      const end = offset + slice.length;
      const chunk: WorkflowRunSnapshotChunk = {
        offset,
        data: btoa(binary),
        next_offset: end < bytes.length ? end : null,
      };
      return chunk;
    },
  };
  return { header, transport, chunkReads };
}

function baseRunId(invocation = '1', conversationId = 'conv-1'): WorkflowRunId {
  return { conversation_id: conversationId, origin: { type: 'tool', attempt_id: 'attempt-1' }, invocation };
}

describe('Workflow run canvas identity', () => {
  it('distinguishes identically named local nodes across root and nested loop scopes', () => {
    const doc = exampleSource('implement_and_review').document!;
    doc.block.nodes.work = { type: 'code', code_language: 'python3', variables: {}, code: 'def main(): return {}', outputs: {} };
    doc.block.nodes.outer = {
      type: 'loop',
      loop_count: 3,
      break_condition: null,
      body: {
        nodes: {
          start: { type: 'start' },
          work: { type: 'code', code_language: 'python3', variables: {}, code: 'def main(): return {}', outputs: {} },
          inner: {
            type: 'iteration',
            iterator: { type: 'reference', path: ['start', 'items'] },
            output: { type: 'reference', path: ['work', 'text'] },
            body: {
              nodes: {
                start: { type: 'start' },
                work: { type: 'template', variables: {}, template: 'ok' },
              },
              edges: [{ from: 'start', port: 'next', to: 'work' }],
            },
          },
        },
        edges: [{ from: 'start', port: 'next', to: 'work' }, { from: 'work', port: 'next', to: 'inner' }],
      },
      loop_variables: [],
    };
    const scene = projectScene(doc);
    const rootWork = qualifiedNodeViewId([], 'work');
    const outerWork = qualifiedNodeViewId(['outer', 'body'], 'work');
    const innerWork = qualifiedNodeViewId(['outer', 'body', 'inner', 'body'], 'work');

    expect(new Set([rootWork, outerWork, innerWork]).size).toBe(3);
    expect(scene.nodes.some(n => n.id === rootWork)).toBe(true);
    expect(scene.nodes.some(n => n.id === outerWork)).toBe(true);
    expect(scene.nodes.some(n => n.id === innerWork)).toBe(true);
    expect(blockPathFromDefinition({ blocks: ['outer', 'body', 'inner', 'body'] })).toEqual([
      { node: 'outer' },
      { node: 'inner' },
    ]);
  });

  it('never maps synthetic editor notes or unexecuted starts to node visits', () => {
    const doc = exampleSource('parallel_review').document!;
    doc.editor = {
      ...doc.editor,
      notes: [{ id: 'note-1', block: [], text: 'author note', position: { x: 10, y: 10 }, dimensions: { width: 200, height: 100 }, theme: 'blue' }],
    };
    const scene = projectScene(doc);
    const run = baseRunId();
    expect(resolveSceneNodeId(scene.nodes, {
      block: { run, definition: { workflow_id: 'parallel_review', blocks: [] }, invocations: [0] },
      node: 'note-1',
    })).toBeUndefined();
    expect(viewId([], 'note', 'note-1')).not.toBe(qualifiedNodeViewId([], 'note-1'));
  });
});

describe('Workflow run canvas aggregation', () => {
  it('ensures completed earlier visits never hide later active visits on the same qualified node', () => {
    const run = workflowRun();
    const runId = run.id;
    const base = run.instances[1];
    const completedIter1: WorkflowInstanceView = {
      ...base,
      node: 'work',
      visit: 0,
      block: { run: runId, definition: { workflow_id: run.workflow_id, blocks: ['loop', 'body'] }, invocations: [0, 1] },
      state: { type: 'settled', outcome: 'completed' },
    };
    const activeIter2: WorkflowInstanceView = {
      ...base,
      node: 'work',
      visit: 0,
      block: { run: runId, definition: { workflow_id: run.workflow_id, blocks: ['loop', 'body'] }, invocations: [0, 2] },
      state: { type: 'waiting', reason: 'review' },
    };
    const failedOther: WorkflowInstanceView = {
      ...base,
      node: 'check',
      visit: 0,
      block: { run: runId, definition: { workflow_id: run.workflow_id, blocks: [] }, invocations: [0] },
      state: { type: 'settled', outcome: 'completed' },
    };
    const failedOtherVisit2: WorkflowInstanceView = {
      ...failedOther,
      visit: 1,
      state: { type: 'settled', outcome: 'failed' },
    };
    run.instances = [completedIter1, activeIter2, failedOther, failedOtherVisit2];

    const summary = aggregateRunOverlay({ run });
    const loopWork = summary.byViewId.get(qualifiedNodeViewId(['loop', 'body'], 'work'))!;
    expect(loopWork.status).toBe('waiting');
    expect(loopWork.activeCount).toBe(1);
    expect(loopWork.waitingCount).toBe(1);
    expect(loopWork.completedCount).toBe(1);
    expect(loopWork.visitCount).toBe(2);
    expect(loopWork.waitingReasons).toEqual(['review']);

    const checkNode = summary.byViewId.get(qualifiedNodeViewId([], 'check'))!;
    expect(checkNode.status).toBe('failed');
    expect(checkNode.failedCount).toBe(1);
    expect(checkNode.completedCount).toBe(1);
    expect(checkNode.visitCount).toBe(2);
  });

  it('bounds retained per-node visits while preserving counts, active visits, and selected visit', () => {
    const run = workflowRun();
    const runId = run.id;
    const selectedVisit = {
      block: { run: runId, definition: { workflow_id: run.workflow_id, blocks: ['loop', 'body'] }, invocations: [0, 5] },
      node: 'step',
      visit: 0,
    };
    const instances: WorkflowInstanceView[] = [];
    for (let index = 1; index <= 2500; index++) {
      instances.push({
        ...run.instances[1],
        node: 'step',
        visit: 0,
        block: { run: runId, definition: { workflow_id: run.workflow_id, blocks: ['loop', 'body'] }, invocations: [0, index] },
        state: index === 2499 ? { type: 'running' } : { type: 'settled', outcome: 'completed' },
      });
    }
    run.instances = instances;
    const start = performance.now();
    const summary = aggregateRunOverlay({ run, selectedVisit, maxVisitsPerNode: 50 });
    const elapsedMs = performance.now() - start;

    const overlay = summary.byViewId.get(qualifiedNodeViewId(['loop', 'body'], 'step'))!;
    expect(overlay.visitCount).toBe(2500);
    expect(overlay.activeCount).toBe(1);
    expect(overlay.completedCount).toBe(2499);
    expect(overlay.status).toBe('running');
    expect(overlay.visits.length).toBe(50);
    expect(overlay.omittedVisits).toBe(2450);
    expect(overlay.visits.some(v => visitIdentityKey(v.instance) === visitIdentityKey(selectedVisit))).toBe(true);
    expect(overlay.visits.some(v => statusOfTraceState(v.state) === 'running')).toBe(true);
    console.info('[measure] bounded aggregate', { visits: 2500, retained: overlay.visits.length, elapsedMs });
  });
});

describe('Workflow run snapshot reader and fencing', () => {
  it('reassembles multi-chunk snapshot JSON, verifies SHA-256, and skips layout when source presentation exists', async () => {
    const doc = exampleSource('parallel_review').document!;
    const runId = baseRunId();
    const { transport, chunkReads } = encodeSnapshotTransport(doc, runId, { chunkSize: 128, hasSourcePresentation: true });
    const result = await readWorkflowRunSnapshot(transport, runId, () => true);

    expect(result.status).toBe('ready');
    if (result.status !== 'ready') throw new Error('expected ready');
    expect(result.document).toEqual(doc);
    expect(chunkReads.length).toBeGreaterThan(1);

    const organizeSpy = vi.fn(async (candidate: WorkflowFileDocument) => candidate);
    const cache = new RunPresentationCache();
    const presentation = await cache.resolve(result.header, result.document, () => true, organizeSpy);
    expect(presentation?.layoutComputed).toBe(false);
    expect(organizeSpy).not.toHaveBeenCalled();
  });

  it('computes layout once lazily when has_source_presentation is false and reuses bounded cache', async () => {
    const doc = exampleSource('parallel_review').document!;
    const runId = baseRunId();
    const { header } = encodeSnapshotTransport(doc, runId, { hasSourcePresentation: false });
    const organizeSpy = vi.fn(async (candidate: WorkflowFileDocument) => ({
      ...candidate,
      editor: { ...candidate.editor, viewport: { x: 11, y: 22, zoom: 1 } },
    }));
    const cache = new RunPresentationCache();

    const first = await cache.resolve(header, doc, () => true, organizeSpy);
    const second = await cache.resolve(header, doc, () => true, organizeSpy);

    expect(first?.layoutComputed).toBe(true);
    expect(second?.layoutComputed).toBe(false);
    expect(organizeSpy).toHaveBeenCalledTimes(1);
    expect(cache.size()).toBe(1);
  });

  it('reports corrupt on SHA-256 mismatch, unavailable on null header, and classifies expired/denied RPC errors', async () => {
    const doc = exampleSource('parallel_review').document!;
    const runId = baseRunId();

    const corrupt = await readWorkflowRunSnapshot(
      encodeSnapshotTransport(doc, runId, { corruptHash: true }).transport,
      runId,
      () => true,
    );
    expect(corrupt.status).toBe('corrupt');

    const unavailable = await readWorkflowRunSnapshot(
      encodeSnapshotTransport(doc, runId, { headerNull: true }).transport,
      runId,
      () => true,
    );
    expect(unavailable.status).toBe('unavailable');

    const expired = await readWorkflowRunSnapshot({
      readHeader: async () => {
        throw new RpcFailure({ code: -32001, message: 'Checkpoint read cut expired' });
      },
      readChunk: async () => null,
    }, runId, () => true);
    expect(expired.status).toBe('expired');

    const denied = await readWorkflowRunSnapshot({
      readHeader: async () => {
        throw new RpcFailure({ code: -32003, message: 'Access denied across conversation boundary' });
      },
      readChunk: async () => null,
    }, runId, () => true);
    expect(denied.status).toBe('denied');

    expect(sameWorkflowRunId(runId, baseRunId('2'))).toBe(false);
  });

  it('fences stale in-flight snapshot loads across run switches and close', async () => {
    const doc1 = exampleSource('parallel_review').document!;
    const doc2 = exampleSource('implement_and_review').document!;
    const run1 = baseRunId('1');
    const run2 = baseRunId('2');
    let releaseFirstHeader!: (value: WorkflowRunSnapshotHeader | null) => void;
    const { header: header2, transport: transport2 } = encodeSnapshotTransport(doc2, run2, { hasSourcePresentation: false });

    const phases: RunSnapshotPhase[] = [];
    const organizeSpy = vi.fn(async (doc: WorkflowFileDocument) => doc);
    const session = new WorkflowRunSnapshotSession(organizeSpy);

    const firstTransport: WorkflowRunSnapshotTransport = {
      readHeader: () => new Promise(resolve => { releaseFirstHeader = resolve; }),
      readChunk: async () => null,
    };

    const p1 = session.load({ scopeKey: 'conv-1', runId: run1, transport: firstTransport, owned: () => true, onPhase: p => phases.push(p) });
    const p2 = session.load({ scopeKey: 'conv-1', runId: run2, transport: transport2, owned: () => true, onPhase: p => phases.push(p) });

    const { header: staleHeader } = encodeSnapshotTransport(doc1, run1);
    releaseFirstHeader(staleHeader);

    const [res1, res2] = await Promise.all([p1, p2]);
    expect(res1.phase).toBe('idle');
    expect(res2.phase).toBe('ready');
    if (res2.phase === 'ready') expect(res2.header.snapshot_digest).toBe(header2.snapshot_digest);
    expect(organizeSpy).toHaveBeenCalledTimes(1);

    session.dispose();
    expect(session.cacheSize()).toBe(0);
  });

  it('pages trace workflow nodes cleanly up to cap without eagerly calling value or content reads', async () => {
    const runId = baseRunId();
    const cut: CheckpointReadCut = { conversation_id: 'conv-1', revision: '10', through: '10', domain_revisions: {}, collection_cuts: {} };
    const root: TraceRecord = {
      id: 'trace:30',
      kind: 'workflow',
      state: 'completed',
      location: { attempt_id: 'attempt-1', step_id: '1' },
      request: null,
      preview: { text: 'test_flow', truncated: false },
      collections: [{ domain: 'trace.records.30.workflow.nodes', revision: '10', total: '50', loaded: '0', next_offset: '0' }],
      position: 'trace:30', timing: { started_at: '2026-11-14T00:00:00Z', ended_at: null, duration_ms: null },
      calls: [], attachments: [], fields: [], has_detail: false, truncated: false,
    } satisfies TraceRecord;

    const valueSpy = vi.fn(async () => { throw new Error('Should not eagerly read visit values'); });
    const contentSpy = vi.fn(async () => { throw new Error('Should not eagerly read content'); });
    const pageSpy = vi.fn(async (_cut: CheckpointReadCut, domain: string, offset: number): Promise<CheckpointPage> => {
      const items = Array.from({ length: 32 }, (_, idx): CheckpointPage['items'][number] | null => {
        const ordinal = offset + idx;
        if (ordinal >= 50) return null;
        const node: WorkflowTraceNode = {
          id: `wn-${ordinal}`,
          instance: { block: { run: runId, definition: { workflow_id: 'test_flow', blocks: [] }, invocations: [0] }, node: 'worker', visit: ordinal },
          kind: 'tool',
          state: { type: 'settled', outcome: 'completed' },
          timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '100' },
          input: { type: 'value', field: 'arguments', domain: `in-${ordinal}`, item_offset: 0, path: [], encoded_bytes: '10' },
          output: { type: 'value', field: 'result', domain: `out-${ordinal}`, item_offset: 0, path: [], encoded_bytes: '10' },
          input_preview: null,
          output_preview: null,
          input_omitted: false,
          output_omitted: false,
        };
        return { offset: String(ordinal), identity: node.id, value: node, content: [], deferred: false, body_complete: true };
      }).filter((x): x is CheckpointPage['items'][number] => x !== null);
      return {
        cursor: '10',
        read_cut: cut,
        domain,
        total: '50',
        collections: [],
        items,
        next_offset: offset + 32 < 50 ? String(offset + 32) : null,
      };
    });

    const reader: TraceFieldReader = { page: pageSpy, value: valueSpy, content: contentSpy };
    const bounded = await readBoundedTraceWorkflowNodes(reader, cut, root, () => true, 32, { maxPages: 1 });

    expect(bounded).not.toBeNull();
    expect(bounded!.nodes).toHaveLength(32);
    expect(bounded!.incomplete).toBe(true);
    expect(bounded!.nextOffset).toBe(32);
    expect(pageSpy).toHaveBeenCalledTimes(1);
    expect(valueSpy).not.toHaveBeenCalled();
    expect(contentSpy).not.toHaveBeenCalled();

    const batch2 = await readBoundedTraceWorkflowNodes(reader, cut, root, () => true, 64, {
      startOffset: bounded!.nextOffset!,
      maxPages: 1,
    });
    expect(batch2).not.toBeNull();
    expect(batch2!.nodes).toHaveLength(18);
    expect(batch2!.incomplete).toBe(false);
    expect(batch2!.nextOffset).toBeNull();
  });

  it('rejects malformed, inconsistent, duplicate, or foreign-run canonical trace pages', async () => {
    const runA = baseRunId('72');
    const runB = baseRunId('71');
    const cut: CheckpointReadCut = { conversation_id: 'conv-1', revision: '10', through: '10', domain_revisions: {}, collection_cuts: {} };
    const root = {
      id: 'trace:31',
      kind: 'workflow',
      state: 'completed',
      location: { attempt_id: 'attempt-1', step_id: '1' },
      collections: [{ domain: 'trace.records.31.workflow.nodes', revision: '10', total: '2', loaded: '0', next_offset: '0' }],
      position: 'trace:31', timing: { started_at: '2026-11-14T00:00:00Z', ended_at: null, duration_ms: null },
      calls: [], attachments: [], fields: [], has_detail: false, truncated: false,
    } satisfies TraceRecord;
    const mkNode = (id: string, run: WorkflowRunId, visit: number): WorkflowTraceNode => ({
      id,
      instance: { block: { run, definition: { workflow_id: 'f', blocks: [] }, invocations: [0] }, node: 'worker', visit },
      kind: 'tool',
      state: { type: 'settled', outcome: 'completed' },
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '10' },
      input: null,
      output: null,
      input_preview: null,
      output_preview: null,
      input_omitted: false,
      output_omitted: false,
    });

    const badCutReader: TraceFieldReader = {
      value: vi.fn(),
      content: vi.fn(),
      page: vi.fn(async () => ({
        cursor: '9',
        read_cut: { ...cut, revision: '9' },
        domain: 'trace.records.31.workflow.nodes',
        total: '2',
        collections: [],
        next_offset: null,
        items: [
          { offset: '0', identity: 'wn-0', value: mkNode('wn-0', runA, 0), content: [], deferred: false, body_complete: true },
          { offset: '1', identity: 'wn-1', value: mkNode('wn-1', runA, 1), content: [], deferred: false, body_complete: true },
        ],
      })),
    };
    await expect(readBoundedTraceWorkflowNodes(badCutReader, cut, root, () => true)).rejects.toThrow();

    const duplicateVisitReader: TraceFieldReader = {
      value: vi.fn(),
      content: vi.fn(),
      page: vi.fn(async () => ({
        cursor: '10',
        read_cut: cut,
        domain: 'trace.records.31.workflow.nodes',
        total: '2',
        collections: [],
        next_offset: null,
        items: [
          { offset: '0', identity: 'wn-0', value: mkNode('wn-0', runA, 0), content: [], deferred: false, body_complete: true },
          { offset: '1', identity: 'wn-1', value: mkNode('wn-1', runA, 0), content: [], deferred: false, body_complete: true },
        ],
      })),
    };
    await expect(readBoundedTraceWorkflowNodes(duplicateVisitReader, cut, root, () => true)).rejects.toThrow(/Duplicate/);

    const foreignRunReader: TraceFieldReader = {
      value: vi.fn(),
      content: vi.fn(),
      page: vi.fn(async () => ({
        cursor: '10',
        read_cut: cut,
        domain: 'trace.records.31.workflow.nodes',
        total: '2',
        collections: [],
        next_offset: null,
        items: [
          { offset: '0', identity: 'wn-0', value: mkNode('wn-0', runA, 0), content: [], deferred: false, body_complete: true },
          { offset: '1', identity: 'wn-1', value: mkNode('wn-1', runB, 1), content: [], deferred: false, body_complete: true },
        ],
      })),
    };
    await expect(
      readBoundedTraceWorkflowNodes(foreignRunReader, cut, root, () => true, 32, { expectedRunId: runA }),
    ).rejects.toThrow(/different run/);
  });

  it('resolves arbitrary Start node IDs, fences foreign-run visits, and bounds large history aggregation latency', () => {
    const doc = exampleSource('parallel_review').document!;
    doc.block.nodes = {
      workflow_start: { type: 'start' },
      outer: {
        type: 'loop',
        loop_count: 2,
        break_condition: null,
        body: {
          nodes: {
            loop_start: { type: 'start' },
            worker: { type: 'template', variables: {}, template: 'ok' },
          },
          edges: [{ from: 'loop_start', port: 'next', to: 'worker' }],
        },
        loop_variables: [],
      },
    };
    doc.block.edges = [{ from: 'workflow_start', port: 'next', to: 'outer' }];
    const scene = projectScene(doc);
    const run = baseRunId('10');
    const otherRun = baseRunId('99');

    const rootStartId = resolveSceneNodeId(scene.nodes, {
      block: { run, definition: { workflow_id: 'f', blocks: [] }, invocations: [0] },
      node: 'workflow_start',
    });
    const loopStartId = resolveSceneNodeId(scene.nodes, {
      block: { run, definition: { workflow_id: 'f', blocks: ['outer', 'body'] }, invocations: [0, 1] },
      node: 'loop_start',
    });
    expect(rootStartId).toBe(viewId([], 'start'));
    expect(loopStartId).toBe(viewId([{ node: 'outer' }], 'start'));

    const traceNodes: WorkflowTraceNode[] = [
      {
        id: 'ws-0',
        instance: { block: { run, definition: { workflow_id: 'f', blocks: [] }, invocations: [0] }, node: 'workflow_start', visit: 0 },
        kind: null,
        state: { type: 'settled', outcome: 'completed' },
        timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '5' },
        input: null,
        output: null,
        input_preview: null,
        output_preview: null,
        input_omitted: false,
        output_omitted: false,
      },
      {
        id: 'ls-0',
        instance: { block: { run, definition: { workflow_id: 'f', blocks: ['outer', 'body'] }, invocations: [0, 1] }, node: 'loop_start', visit: 0 },
        kind: null,
        state: { type: 'running' },
        timing: { started_at: '2026-10-20T00:00:01Z', ended_at: null, duration_ms: null },
        input: null,
        output: null,
        input_preview: null,
        output_preview: null,
        input_omitted: false,
        output_omitted: false,
      },
      // Foreign-run instance targeting the same node must be filtered out completely.
      {
        id: 'foreign-0',
        instance: { block: { run: otherRun, definition: { workflow_id: 'f', blocks: ['outer', 'body'] }, invocations: [0, 1] }, node: 'loop_start', visit: 0 },
        kind: null,
        state: { type: 'settled', outcome: 'failed' },
        timing: { started_at: '2026-10-20T00:00:01Z', ended_at: '2026-10-20T00:00:02Z', duration_ms: '10' },
        input: null,
        output: null,
        input_preview: null,
        output_preview: null,
        input_omitted: false,
        output_omitted: false,
      },
    ];

    const summary = aggregateRunOverlay({
      runId: run,
      traceNodes,
      sceneNodes: scene.nodes,
    });
    expect(summary.byViewId.get(rootStartId!)?.status).toBe('completed');
    expect(summary.byViewId.get(loopStartId!)?.status).toBe('incomplete');
    expect(summary.byViewId.get(loopStartId!)?.visitCount).toBe(1);

    // By default (without matching live owner instance), unsettled states downgrade cleanly to incomplete.
    const downgraded = aggregateRunOverlay({
      runId: run,
      traceNodes,
      sceneNodes: scene.nodes,
    });
    expect(downgraded.byViewId.get(loopStartId!)?.status).toBe('incomplete');

    // Matching live owner keeps active visit running; settled live run or omitted frozen visit downgrades/settles.
    const activeLiveRun: WorkflowRunView = {
      id: run,
      workflow_id: 'f',
      program_digest: 'digest-f',
      resource_revision: '1',
      tool_call_id: 'tc-1',
      state: { type: 'running' },
      omitted_instances: 0,
      steps_consumed: 2,
      steps_max: 64,
      agents_consumed: 0,
      model_calls_consumed: 0,
      instances: [{
        block: traceNodes[1].instance.block,
        node: 'loop_start',
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
    const liveConfirmed = aggregateRunOverlay({
      runId: run,
      traceNodes,
      sceneNodes: scene.nodes,
      run: activeLiveRun,
    });
    expect(liveConfirmed.byViewId.get(loopStartId!)?.status).toBe('running');

    const settledLiveRun: WorkflowRunView = {
      ...activeLiveRun,
      state: { type: 'settled', outcome: 'completed' },
      instances: [{ ...activeLiveRun.instances[0], state: { type: 'settled', outcome: 'completed' } }],
    };
    expect(aggregateRunOverlay({
      runId: run,
      traceNodes,
      sceneNodes: scene.nodes,
      run: settledLiveRun,
    }).byViewId.get(loopStartId!)?.status).toBe('completed');

    expect(aggregateRunOverlay({
      runId: run,
      traceNodes,
      sceneNodes: scene.nodes,
      run: { ...activeLiveRun, instances: [] },
    }).byViewId.get(loopStartId!)?.status).toBe('incomplete');

    // Bounded history scale test: 500 worker visits remain strictly bounded at 200 per node while keeping selected visit 499.
    const manyVisits: WorkflowTraceNode[] = Array.from({ length: 500 }, (_, i) => ({
      id: `mv-${i}`,
      instance: { block: { run, definition: { workflow_id: 'f', blocks: ['outer', 'body'] }, invocations: [0, i + 1] }, node: 'worker', visit: 0 },
      kind: 'tool',
      state: { type: 'settled', outcome: 'completed' },
      timing: { started_at: '2026-10-20T00:00:00Z', ended_at: '2026-10-20T00:00:01Z', duration_ms: '10' },
      input: null,
      output: null,
      input_preview: null,
      output_preview: null,
      input_omitted: false,
      output_omitted: false,
    }));
    const selectedVisit = { block: { run, definition: { workflow_id: 'f', blocks: ['outer', 'body'] }, invocations: [0, 5] }, node: 'worker', visit: 0 };
    const startMs = performance.now();
    const scaled = aggregateRunOverlay({
      runId: run,
      traceNodes: manyVisits.slice(0, 64),
      selectedTraceNode: manyVisits[4],
      selectedVisit,
      sceneNodes: scene.nodes,
    });
    const elapsedMs = performance.now() - startMs;
    const workerView = qualifiedNodeViewId(['outer', 'body'], 'worker');
    expect(scaled.byViewId.get(workerView)?.visits.some(v => visitIdentityKey(v.instance) === visitIdentityKey(selectedVisit))).toBe(true);
    console.info('[measure] partial aggregate', { visits: 64, elapsedMs });
  });

  it('pins the exact selected visit under an active overflow and never invents missing visit identity', () => {
    const run = workflowRun();
    run.instances = Array.from({ length: 250 }, (_, visit) => ({ ...run.instances[1], node: 'worker', visit, state: { type: 'running' } }));
    const selected = { block: run.instances[249].block, node: 'worker', visit: 249 };
    const overlay = aggregateRunOverlay({ run, runId: run.id, selectedVisit: selected, maxVisitsPerNode: 10 });
    const worker = overlay.byViewId.get(qualifiedNodeViewId(selected.block.definition.blocks, 'worker'))!;
    expect(worker.visitCount).toBe(250);
    expect(worker.visits).toHaveLength(10);
    expect(worker.visits.some(visit => visit.key === visitIdentityKey(selected))).toBe(true);
    const unknown = aggregateRunOverlay({ run: { ...run, instances: [{ ...run.instances[0], visit: null }] }, runId: run.id });
    expect(unknown.byViewId.size).toBe(0);
    expect(unknown.traceIncomplete).toBe(true);
    const settled = aggregateRunOverlay({ run: { ...run, state: { type: 'settled', outcome: 'cancelled' } }, runId: run.id });
    expect([...settled.byViewId.values()].every(node => node.activeCount === 0 && node.status === 'incomplete')).toBe(true);
  });

  it('measures admitted maximum nested graph and long repeated history with deterministic work bounds', async () => {
    const run = baseRunId();
    const doc = runBoundsDocument('maximum');
    expect(boundedDocument(doc)).toBe(doc);
    const encoded = encodeSnapshotTransport(doc, run, { chunkSize: 16384, hasSourcePresentation: false });
    let layoutCalls = 0, layoutMs = 0;
    const session = new WorkflowRunSnapshotSession(async document => {
      layoutCalls++;
      const started = performance.now();
      const result = await organizeDocument(document);
      layoutMs += performance.now() - started;
      return result;
    });
    const started = performance.now();
    const options = { scopeKey: 'maximum', runId: run, transport: encoded.transport, owned: () => true, onPhase: vi.fn() };
    const loaded = await session.load(options);
    const loadMs = performance.now() - started;
    expect(loaded.phase).toBe('ready');
    if (loaded.phase !== 'ready') throw Error('maximum snapshot was unavailable');
    const chunksPerLoad = encoded.chunkReads.length;
    expect(chunksPerLoad).toBe(Math.ceil(encoded.header.byte_length / 16384));
    expect(chunksPerLoad).toBeGreaterThan(1);
    const sceneStarted = performance.now();
    const scene = projectScene(loaded.document);
    const sceneMs = performance.now() - sceneStarted;
    expect(scene.nodes).toHaveLength(256);
    const reopened = await session.load(options);
    expect(reopened.phase === 'ready' && reopened.layoutComputed).toBe(false);
    expect(layoutCalls).toBe(1);
    expect(encoded.chunkReads.length).toBe(chunksPerLoad * 2);
    expect(session.cacheSize()).toBe(1);
    session.dispose();
    expect(session.cacheSize()).toBe(0);

    // This separate native-admitted 42-node graph runs 100 rounds within the
    // 4,096-step budget, retaining 3,903 visits with exact invocation paths.
    const historyDoc = runBoundsDocument('long_history');
    const historyScene = projectScene(historyDoc);
    const visits = longRunVisits(run, 'repeated');
    expect(visits).toHaveLength(3903);
    const cut: CheckpointReadCut = { conversation_id: run.conversation_id, revision: '1', through: '16000', domain_revisions: {}, collection_cuts: {} };
    const domain = 'trace.records.1.workflow.nodes';
    const root: TraceRecord = {
      id: 'trace:1', position: 'trace:1', kind: 'workflow', state: 'completed',
      location: {}, timing: { started_at: '2026-11-14T00:00:00Z', ended_at: null, duration_ms: null },
      fields: [], calls: [], attachments: [], truncated: false, has_detail: false,
      collections: [{ domain, revision: '1', total: String(visits.length), loaded: '0', next_offset: '0' }],
    };
    const reader: TraceFieldReader = {
      value: vi.fn(), content: vi.fn(),
      page: vi.fn(async (_cut, selectedDomain, offset) => {
        const nodes = visits.slice(offset, offset + 32), next = offset + nodes.length;
        return { domain: selectedDomain, cursor: '1', read_cut: cut, total: String(visits.length), collections: [], next_offset: next < visits.length ? String(next) : null,
          items: nodes.map((node, index) => ({ offset: String(offset + index), identity: node.id, value: node as unknown as Record<string, unknown>, content: [], deferred: false, body_complete: true })),
        } satisfies CheckpointPage;
      }),
    };
    const batch = await readBoundedTraceWorkflowNodes(reader, cut, root, () => true, 128, { maxPages: 2, expectedRunId: run });
    expect(batch?.nodes).toHaveLength(64);
    expect(batch?.incomplete).toBe(true);
    expect(reader.page).toHaveBeenCalledTimes(2);
    const statusStarted = performance.now();
    const partial = aggregateRunOverlay({ runId: run, traceNodes: batch!.nodes, sceneNodes: historyScene.nodes, traceIncomplete: true });
    expect(partial.traceIncomplete).toBe(true);
    const full = aggregateRunOverlay({ runId: run, traceNodes: visits, sceneNodes: historyScene.nodes, selectedVisit: visits.at(-2)!.instance });
    const updateMs = performance.now() - statusStarted;
    expect(full.totalVisitsSeen).toBe(3903);
    expect(full.byViewId.get(qualifiedNodeViewId(['repeat', 'body'], 'worker_37'))?.visitCount).toBe(100);
    expect([...full.byViewId.values()].every(node => node.visits.length <= 200)).toBe(true);
    expect(reader.value).not.toHaveBeenCalled();
    expect(reader.content).not.toHaveBeenCalled();
    console.info('[measure] run canvas bounds', { snapshotBytes: encoded.header.byte_length, chunksPerLoad, nodes: scene.nodes.length, visits: visits.length, loadMs, layoutMs, sceneMs, updateMs, layoutCalls, initialTracePages: 2, eagerValues: 0 });
  });
});
