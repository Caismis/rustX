import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/scrollbar.css';
import '../../src/presentation/theme/corner-shape.css';
import '../../src/presentation/theme/shiki.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
import { useState } from 'react';
import { createActor } from 'xstate';
import WorkflowEditor from '../../src/app/workflows/ui/editor/WorkflowEditor';
import { workflowEditorMachine } from '../../src/app/workflows/machines/editor';
import { workflowSurfaceMachine } from '../../src/app/workflows/machines/surface';
import { USER_SCOPE, type WorkflowScopePort } from '../../src/app/workflows/port';
import { createRoot } from 'react-dom/client';
import type {
  AttachmentTarget,
  CheckpointPage,
  CheckpointReadCut,
  CheckpointValueChunk,
  ContentChunk,
  ContentRef,
  RuntimeClientInteractionHeader,
  TraceRecord,
  WorkflowFileDocument,
  WorkflowInstanceView,
  WorkflowRunId,
  WorkflowRunView,
  WorkflowTraceNode,
} from '../../../protocol/app-server/v78';
import type { TraceCache } from '../../src/client/trace';
import { applyTheme } from '../../src/app/appearance';
import { WorkflowRunPanel } from '../../src/app/agent/WorkflowRunPanel';
import { ConversationRunCanvasHost } from '../../src/app/workflows/run-canvas/ConversationRunCanvasHost';
import { RpcFailure } from '../../src/client/errors';
import { Server, snapshot } from '../fixture';
import { statusSource } from '../workflow-fixtures';
import { localeController, type LocaleId } from '../../src/locale/controller';

async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  const digest = await crypto.subtle.digest('SHA-256', copy);
  return Array.from(new Uint8Array(digest), b => b.toString(16).padStart(2, '0')).join('');
}

function makeNestedDoc(): WorkflowFileDocument {
  const base = structuredClone(statusSource().document!);
  base.description = 'Immutable run snapshot fixture';
  const nodes: WorkflowFileDocument['block']['nodes'] = {
    workflow_start: { type: 'start' },
    outer: {
      type: 'loop',
      loop_count: 3,
      break_condition: null,
      body: {
        nodes: {
          loop_start: { type: 'start' },
          worker: { type: 'template', variables: {}, template: 'run' },
          revise: {
            type: 'review',
            content: [{ type: 'literal', value: 'Review result' }],
            actions: [{ id: 'approve', title: 'Approve' }],
            timeout_ms: '60000',
          },
        },
        edges: [
          { from: 'loop_start', port: 'next', to: 'worker' },
          { from: 'worker', port: 'next', to: 'revise' },
        ],
      },
    },
    done: { type: 'return', outputs: [] },
  };
  base.block.nodes = nodes;
  base.block.edges = [
    { from: 'workflow_start', port: 'next', to: 'outer' },
    { from: 'outer', port: 'next', to: 'done' },
  ];
  return base;
}

const params = new URLSearchParams(location.search);
const pagedParams = params.has('paged');
const doc = makeNestedDoc();
if (doc.block.nodes.outer.type === 'loop' && pagedParams) doc.block.nodes.outer.loop_count = 69;
const runId: WorkflowRunId = {
  conversation_id: 'conversation-A',
  origin: { type: 'tool', attempt_id: 'attempt-e2e' },
  invocation: '1',
};

const cut: CheckpointReadCut = {
  conversation_id: 'conversation-A',
  revision: '1',
  through: '1',
  domain_revisions: {},
  collection_cuts: {},
};

function payloadBytes(domain: string) {
  return new TextEncoder().encode(JSON.stringify(JSON.stringify({ domain, result: 'lazy-output-e2e' }))).length;
}

function makeVisit(
  id: string,
  iteration: number,
  state: WorkflowTraceNode['state'],
  node = 'worker',
  blocks: string[] = ['outer', 'body'],
): WorkflowTraceNode {
  const inDomain = `in-${id}`;
  const outDomain = `out-${id}`;
  return {
    id,
    instance: {
      block: { run: runId, definition: { workflow_id: 'nested_e2e', blocks }, invocations: blocks.length ? [0, iteration] : [0] },
      node,
      visit: 0,
    },
    kind: node === 'workflow_start' || node === 'loop_start' ? null : 'template',
    state,
    timing: { started_at: '2026-11-14T00:00:00Z', ended_at: state.type === 'settled' ? '2026-11-14T00:00:01Z' : null, duration_ms: '32' },
    input: { type: 'value', field: 'arguments', domain: inDomain, item_offset: 0, path: [], encoded_bytes: String(payloadBytes(inDomain)) },
    output: { type: 'value', field: 'result', domain: outDomain, item_offset: 0, path: [], encoded_bytes: String(payloadBytes(outDomain)) },
    input_preview: JSON.stringify({ iteration }),
    output_preview: null,
    input_omitted: false,
    output_omitted: false,
  };
}

function makeInstance(iteration: number, state: WorkflowInstanceView['state']): WorkflowInstanceView {
  return {
    block: { run: runId, definition: { workflow_id: 'nested_e2e', blocks: ['outer', 'body'] }, invocations: [0, iteration] },
    node: 'worker',
    visit: 0,
    kind: 'template',
    state,
    iteration,
    iterations_max: pagedParams ? 69 : 3,
    child: null,
    child_agent_id: null,
    checks_passed: state.type === 'settled' ? true : null,
    loop_exit: null,
  };
}

const sourceJson = JSON.stringify(doc);
const sourceBytes = new TextEncoder().encode(sourceJson);
const sourceBase64 = btoa(String.fromCharCode(...sourceBytes));
const sourceSha256 = await sha256Hex(sourceBytes);

const initialTraceNodes: WorkflowTraceNode[] = [
  makeVisit('wn-start', 0, { type: 'settled', outcome: 'completed' }, 'workflow_start', []),
  makeVisit('wn-iter-1', 1, { type: 'settled', outcome: 'completed' }),
  makeVisit('wn-iter-2', 2, { type: 'running' }),
];

const initialRun: WorkflowRunView = {
  id: runId,
  workflow_id: 'nested_e2e',
  program_digest: 'sha256:e2e',
  resource_revision: '1',
  state: { type: 'running' },
  instances: [
    makeInstance(1, { type: 'settled', outcome: 'completed' }),
    makeInstance(pagedParams ? 69 : 2, { type: 'running' }),
  ],
  omitted_instances: 0,
  steps_consumed: 2,
  steps_max: 4096,
  agents_consumed: 0,
  model_calls_consumed: 0,
};

const unavailable = params.has('unavailable');
const traceFailOnce = params.has('traceFailOnce');
let snapshotHeaderReads = 0;
let snapshotChunkReads = 0;
let tracePageAttempt = 0;

const server = new Server();
const s = snapshot('A');
s.workflows = { revision: '1', runs: [initialRun], omitted_runs: 0 };
server.snapshots.set('A', s);
await server.attached('A');

const attachedTarget: AttachmentTarget = server.client.getSnapshot().views['A']!.target!;

server.handlers.set('conversation/workflowRunSnapshotHeader', () => {
  snapshotHeaderReads += 1;
  return {
    type: 'workflow_run_snapshot_header' as const,
    header: unavailable
      ? null
      : {
          run_id: runId,
          workflow_id: 'nested_e2e',
          program_digest: 'sha256:e2e',
          source_revision: 'rev-e2e',
          snapshot_digest: 'sha256:snap-e2e',
          has_source_presentation: true,
          byte_length: sourceBytes.byteLength,
          sha256: sourceSha256,
        },
  };
});

server.handlers.set('conversation/workflowRunSnapshotChunk', () => {
  snapshotChunkReads += 1;
  return {
    type: 'workflow_run_snapshot_chunk' as const,
    chunk: unavailable ? null : { offset: 0, data: sourceBase64, next_offset: null },
  };
});

const traceDomain = 'trace.records.1.workflow.nodes';
const valueReads: string[] = [];
let currentTraceCut: CheckpointReadCut = cut;

const traceNodesTotal = pagedParams ? 70 : 3;
let liveTraceNodes: WorkflowTraceNode[] = pagedParams
  ? [
    makeVisit('wn-start', 0, { type: 'settled', outcome: 'completed' }, 'workflow_start', []),
    ...Array.from({ length: 69 }, (_, idx) =>
      makeVisit(`wn-iter-${idx + 1}`, idx + 1, idx === 68 ? { type: 'running' } : { type: 'settled', outcome: 'completed' })),
  ]
  : initialTraceNodes;

function currentTraceRoot(): TraceRecord {
  return {
    id: 'trace:1',
    position: '1',
    location: { attempt_id: 'attempt-e2e', step_id: 'step-1' },
    kind: 'workflow',
    state: 'running',
    timing: { started_at: '2026-11-14T00:00:00Z', ended_at: null, duration_ms: null },
    calls: [],
    attachments: [],
    fields: [],
    collections: [{
      domain: traceDomain,
      revision: currentTraceCut.revision,
      total: String(liveTraceNodes.length),
      loaded: '0',
      next_offset: '0',
    }],
    native_id: null,
    originating_tool_call_id: null,
    has_detail: false,
    truncated: false,
  };
}

server.handlers.set('session/traceLocateWorkflow', request => {
  if (request.method !== 'session/traceLocateWorkflow') throw Error('Unexpected locator method');
  if (JSON.stringify(request.params.run_id) !== JSON.stringify(runId)) throw Error('Foreign run lookup');
  return { type: 'trace_workflow_location' as const, location: {
    record_id: 'trace:1',
    page: { read_cut: currentTraceCut, records: [currentTraceRoot()], next_cursor: null, updates: [] },
  } };
});

server.handlers.set('conversation/checkpointPage', request => {
  const { domain, offset = 0, limit = 32 } = request.params as { domain: string; offset?: number; limit?: number };
  if (domain !== traceDomain) {
    throw new Error(`Unexpected checkpointPage domain: ${domain}`);
  }
  if (traceFailOnce && offset === 0 && tracePageAttempt === 0) {
    tracePageAttempt += 1;
    throw new RpcFailure({ code: -32000, message: 'Injected initial trace batch failure' });
  }
  if (traceFailOnce && offset === 0) {
    tracePageAttempt += 1;
  }
  const pageSize = Math.min(32, Math.max(1, limit));
  const slice = liveTraceNodes.slice(offset, offset + pageSize);
  const next = offset + slice.length;
  const page: CheckpointPage = {
    domain,
    cursor: currentTraceCut.revision,
    read_cut: currentTraceCut,
    total: String(traceNodesTotal),
    collections: [],
    next_offset: next < liveTraceNodes.length ? String(next) : null,
    items: slice.map((node, index) => ({
      offset: String(offset + index),
      identity: node.id,
      value: node as unknown as Record<string, unknown>,
      content: [],
      deferred: false,
      body_complete: true,
    })),
  };
  return { type: 'checkpoint_page' as const, page };
});

server.handlers.set('conversation/workflowTraceNode', request => {
  if (request.method !== 'conversation/workflowTraceNode') throw Error('Unexpected node lookup method');
  const { cut: reqCut, instance } = request.params;
  if (JSON.stringify(reqCut) !== JSON.stringify(currentTraceCut)) {
    throw new Error('Stale cut in workflowTraceNode fixture lookup');
  }
  const key = JSON.stringify([
    instance.block.run.conversation_id,
    instance.block.run.origin,
    instance.block.run.invocation,
    instance.block.definition.workflow_id,
    instance.block.definition.blocks,
    instance.block.invocations,
    instance.node,
    instance.visit,
  ]);
  const node = liveTraceNodes.find(item => JSON.stringify([
    item.instance.block.run.conversation_id,
    item.instance.block.run.origin,
    item.instance.block.run.invocation,
    item.instance.block.definition.workflow_id,
    item.instance.block.definition.blocks,
    item.instance.block.invocations,
    item.instance.node,
    item.instance.visit,
  ]) === key) ?? null;
  return { type: 'workflow_trace_node' as const, node };
});

server.handlers.set('conversation/checkpointValueRead', request => {
  if (request.method !== 'conversation/checkpointValueRead') throw Error('Unexpected value method');
  const { domain, item_offset, path, cut: readCut } = request.params;
  valueReads.push(domain);
  const json = JSON.stringify({ domain, result: 'lazy-output-e2e' });
  const bytes = new TextEncoder().encode(JSON.stringify(json));
  const bytes_base64 = btoa(String.fromCharCode(...bytes));
  const chunk: CheckpointValueChunk = {
    representation: 'canonical_json_v1',
    cut: readCut,
    domain,
    item_offset,
    path: [...path],
    offset: 0,
    bytes_base64,
    next_offset: null,
    readable_end: bytes.length,
  };
  return { type: 'checkpoint_value_chunk' as const, chunk };
});

server.handlers.set('conversation/contentRead', request => {
  const { content_ref } = request.params as { content_ref: ContentRef };
  const chunk: ContentChunk = {
    content_ref,
    offset: 0,
    data_base64: '',
    end_offset: 0,
    readable_end: 0,
    next_offset: null,
    complete: true,
  };
  return { type: 'content_chunk' as const, chunk };
});

function makeTraceCache(currentCut: CheckpointReadCut, epoch: number): TraceCache {
  const root = currentTraceRoot();
  return {
    epoch,
    page: {
      read_cut: currentCut,
      records: [root],
      updates: [],
      next_cursor: null,
    },
    readCuts: { [root.id]: currentCut },
    details: {},
  };
}

// A real authoring actor and rendered editor coexist with the readonly run view.
// Tests edit its real inspector; no Run Canvas callback receives this actor.
const authorSource = statusSource();
let authorWrites = 0;
const authorPort: WorkflowScopePort = {
  endpoint: 'ws://127.0.0.1:1/', scope: USER_SCOPE,
  enable: async () => { throw Error('Unexpected authoring enable'); },
  analyze: async () => { throw Error('Unexpected authoring analysis'); },
  toolCatalog: async () => ({ sources: [] }),
  modelChoices: async () => ({ type: 'models', revision: '1', models: [], next_offset: null }),
  list: async () => ({ target: { kind: 'user' }, directory: '/fixture', absent_revision: 'missing', diagnostic: null, files: [] }),
  read: async () => structuredClone(authorSource),
  check: async () => ({ revision: authorSource.revision, structure: { status: 'valid', outputs: {} }, dependencies: { status: 'not_checked' }, explanation: null }),
  write: async () => { authorWrites++; throw Error('Unexpected authoring write'); },
};
const authorSurface = createActor(workflowSurfaceMachine, { input: { ports: () => authorPort } }).start();
const authorEditor = createActor(workflowEditorMachine, { input: { port: authorPort, target: { endpoint: authorPort.endpoint, scope: USER_SCOPE, id: authorSource.id } } }).start();

function App() {
  const [runs, setRuns] = useState<WorkflowRunView[]>([initialRun]);
  const [traceCut, setTraceCut] = useState<CheckpointReadCut>(cut);
  const [traceCacheState, setTraceCacheState] = useState<TraceCache>(() => makeTraceCache(cut, 1));
  const [ownerCurrent, setOwnerCurrent] = useState(true);
  const [theme, setTheme] = useState<'light' | 'dark'>(params.get('theme') === 'dark' ? 'dark' : 'light');

  const switchTheme = (next: 'light' | 'dark') => {
    setTheme(next);
    applyTheme(next);
  };

  if (typeof window !== 'undefined') {
    window.runCanvasFixture = {
      completeLiveRun() {
        const updatedRun: WorkflowRunView = {
          ...initialRun,
          state: { type: 'settled', outcome: 'completed' },
          instances: [
            makeInstance(1, { type: 'settled', outcome: 'completed' }),
            makeInstance(pagedParams ? 69 : 2, { type: 'settled', outcome: 'completed' }),
          ],
        };
        const nextCut: CheckpointReadCut = { ...cut, revision: '2', through: '2' };
        currentTraceCut = nextCut;
        liveTraceNodes = liveTraceNodes.map(node => ({ ...node, state: { type: 'settled', outcome: 'completed' }, timing: { ...node.timing, ended_at: '2026-11-14T00:00:01Z' } }));
        setRuns([updatedRun]);
        setTraceCut(nextCut);
        setTraceCacheState(prev => makeTraceCache(nextCut, prev.epoch + 1));
      },
      disconnectLiveOwner() {
        setRuns([]);
      },
      retireCanvasOwner() {
        setOwnerCurrent(false);
      },
      setLocale(locale: LocaleId) {
        localeController.setLocale(locale);
      },
      setTheme: switchTheme,
      valueReads: () => [...valueReads],
      snapshotReads: () => ({ headers: snapshotHeaderReads, chunks: snapshotChunkReads }),
      authorState: () => {
        const { document, drafts, past, future, notice, uncertain } = authorEditor.getSnapshot().context;
        return { document, drafts, past, future, notice, uncertain, writes: authorWrites };
      },
    };
  }

  const interactions: RuntimeClientInteractionHeader[] = [];

  return (
    <main style={{ padding: 16, display: 'flex', flexDirection: 'column', gap: 12 }}>
      <header style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center' }}>
        <button type="button" onClick={() => localeController.setLocale('en')}>EN</button>
        <button type="button" onClick={() => localeController.setLocale('zh')}>ZH</button>
        <button type="button" onClick={() => switchTheme(theme === 'light' ? 'dark' : 'light')}>Toggle theme</button>
      </header>
      <ConversationRunCanvasHost
        client={server.client}
        sessionId="A"
        conversationId={s.conversation_id}
        attachmentTarget={attachedTarget}
        liveCheckpointCut={traceCut}
        traceCache={traceCacheState}
        ownerCurrent={ownerCurrent}
        liveRuns={runs}
        pendingInteractions={interactions}
      >
        <WorkflowRunPanel run={runs[0] ?? { ...initialRun, instances: [makeInstance(2, { type: 'running' })] }} />
      </ConversationRunCanvasHost>
      {params.has('authoring') && <div id="author-editor" style={{ height: 700, minHeight: 700, display: 'flex', flexDirection: 'column' }}><WorkflowEditor surface={authorSurface} editor={authorEditor} scopeLabel="Global" /></div>}
    </main>
  );
}

applyTheme(params.get('theme') === 'dark' ? 'dark' : 'light');
const initialLocale = params.get('locale');
if (initialLocale === 'zh' || initialLocale === 'en') {
  localeController.setLocale(initialLocale);
}
createRoot(document.getElementById('root')!).render(<App />);

declare global {
  interface Window {
    runCanvasFixture: {
      completeLiveRun(): void;
      disconnectLiveOwner(): void;
      retireCanvasOwner(): void;
      setLocale(locale: LocaleId): void;
      setTheme(theme: 'light' | 'dark'): void;
      valueReads(): string[];
      snapshotReads(): { headers: number; chunks: number };
      authorState(): unknown;
    };
  }
}
