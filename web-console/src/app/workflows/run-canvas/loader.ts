import { validateCanonicalInspectionPage } from '../../../../../protocol/app-server/trace-inspection';
import type {
  CheckpointReadCut,
  TraceRecord,
  WorkflowFileDocument,
  WorkflowRunId,
  WorkflowTraceNode,
} from '../../../../../protocol/app-server/v78';
import { workflowKey } from '../../../bindings/workflow-children';
import type { TraceFieldReader } from '../../../client/trace';
import type { WorkflowRunSnapshotHeader, WorkflowRunSnapshotLoadResult, WorkflowRunSnapshotTransport } from './contract';
import { sameWorkflowRunId, visitIdentityKey } from './identity';
import { readWorkflowRunSnapshot, RunPresentationCache } from './snapshot-reader';

export const DEFAULT_MAX_TRACE_NODES = 128;
export const DEFAULT_TRACE_BATCH_PAGES = 2;

export type RunSnapshotPhase =
  | { readonly phase: 'idle' }
  | { readonly phase: 'loading' }
  | { readonly phase: 'ready'; readonly header: WorkflowRunSnapshotHeader; readonly document: WorkflowFileDocument; readonly layoutComputed: boolean }
  | { readonly phase: 'unavailable'; readonly reason: 'missing_snapshot' | 'unsupported' | 'empty'; readonly message?: string }
  | { readonly phase: 'corrupt'; readonly message: string }
  | { readonly phase: 'expired'; readonly message: string }
  | { readonly phase: 'denied'; readonly message: string }
  | { readonly phase: 'error'; readonly message: string };

export interface BoundedTraceNodesResult {
  readonly nodes: readonly WorkflowTraceNode[];
  readonly incomplete: boolean;
  readonly nextOffset: number | null;
  readonly total: string;
}

function isValidTraceNode(value: unknown): value is WorkflowTraceNode {
  if (!value || typeof value !== 'object') return false;
  const node = value as WorkflowTraceNode;
  if (typeof node.id !== 'string' || !node.instance || typeof node.instance !== 'object') return false;
  const inst = node.instance;
  if (!inst.block || !inst.block.run || !inst.block.definition) return false;
  if (typeof inst.block.run.conversation_id !== 'string' || typeof inst.block.run.invocation !== 'string' || !inst.block.run.origin) return false;
  if (typeof inst.block.definition.workflow_id !== 'string' || !Array.isArray(inst.block.definition.blocks)) return false;
  if (!Array.isArray(inst.block.invocations)) return false;
  if (typeof inst.node !== 'string' || !Number.isSafeInteger(inst.visit) || inst.visit < 0) return false;
  return !!node.state && typeof node.state.type === 'string' && !!node.timing && typeof node.timing === 'object';
}

/** Page trace workflow nodes in bounded coherent batches using canonical inspection
 * validation, without eagerly reading any visit input/output value streams or
 * pulling unbounded history merely to display a graph. */
export async function readBoundedTraceWorkflowNodes(
  reader: TraceFieldReader,
  cut: CheckpointReadCut,
  root: TraceRecord,
  current: () => boolean,
  maxNodes = DEFAULT_MAX_TRACE_NODES,
  options?: {
    readonly startOffset?: number;
    readonly maxPages?: number;
    readonly expectedRunId?: WorkflowRunId;
    readonly seenIds?: Set<string>;
    readonly seenVisits?: Set<string>;
  },
): Promise<BoundedTraceNodesResult | null> {
  const collection = root.collections.find(item => item.domain.endsWith('.workflow.nodes'));
  if (!collection) return { nodes: [], incomplete: false, nextOffset: null, total: '0' };
  if (collection.total === '0') return { nodes: [], incomplete: false, nextOffset: null, total: '0' };

  const collected: WorkflowTraceNode[] = [];
  const seenIds = options?.seenIds ?? new Set<string>();
  const seenVisits = options?.seenVisits ?? new Set<string>();
  const startOffset = options?.startOffset ?? 0;
  const maxPages = Math.max(1, options?.maxPages ?? Math.ceil(maxNodes / 32));
  let offset: number | null = startOffset;
  let pagesRead = 0;

  while (offset !== null && current() && pagesRead < maxPages && collected.length < maxNodes) {
    const page = await reader.page(cut, collection.domain, offset, current);
    if (!current()) return null;
    if (page.domain !== collection.domain || JSON.stringify(page.read_cut) !== JSON.stringify(cut)) {
      throw new Error('Original Workflow node page changed.');
    }
    validateCanonicalInspectionPage(page, offset, collection.total, true);
    pagesRead += 1;

    for (const item of page.items) {
      const value = item.value;
      if (!isValidTraceNode(value) || seenIds.has(value.id)) {
        throw new Error('Original Workflow node header is unavailable.');
      }
      if (options?.expectedRunId && !sameWorkflowRunId(value.instance.block.run, options.expectedRunId)) {
        throw new Error('Workflow node page entry belongs to a different run.');
      }
      const vKey = visitIdentityKey(value.instance);
      if (seenVisits.has(vKey)) {
        throw new Error('Duplicate Workflow node visit in canonical page.');
      }
      seenIds.add(value.id);
      seenVisits.add(vKey);
      collected.push(value);
    }

    offset = page.next_offset == null ? null : Number(page.next_offset);
  }

  return current()
    ? {
      nodes: collected,
      incomplete: offset !== null,
      nextOffset: offset,
      total: collection.total,
    }
    : null;
}

/** Fences asynchronous run snapshot loading and lazy one-time layout resolution
 * across conversation/run/close transitions while reusing immutable layouts. */
export class WorkflowRunSnapshotSession {
  private epoch = 0;
  private disposed = false;
  private loadedKey: string | null = null;
  private readonly presentationCache = new RunPresentationCache();

  constructor(
    private readonly organizeFn?: (doc: WorkflowFileDocument) => Promise<WorkflowFileDocument>,
  ) {}

  currentKey(): string | null {
    return this.loadedKey;
  }

  invalidate() {
    this.epoch += 1;
    this.loadedKey = null;
  }

  dispose() {
    this.disposed = true;
    this.epoch += 1;
    this.loadedKey = null;
    this.presentationCache.clear();
  }

  cacheSize() {
    return this.presentationCache.size();
  }

  async load(options: {
    readonly scopeKey: string;
    readonly runId: WorkflowRunId;
    readonly transport: WorkflowRunSnapshotTransport;
    readonly owned: () => boolean;
    readonly onPhase: (phase: RunSnapshotPhase) => void;
  }): Promise<RunSnapshotPhase> {
    const requestKey = `${options.scopeKey}:${workflowKey(options.runId)}`;
    const token = ++this.epoch;
    this.loadedKey = requestKey;
    const isCurrent = () => !this.disposed && this.epoch === token && options.owned();

    if (!isCurrent()) return { phase: 'idle' };
    options.onPhase({ phase: 'loading' });

    const loaded: WorkflowRunSnapshotLoadResult = await readWorkflowRunSnapshot(
      options.transport,
      options.runId,
      isCurrent,
    );
    if (!isCurrent()) return { phase: 'idle' };

    if (loaded.status === 'cancelled') return { phase: 'idle' };
    if (loaded.status !== 'ready') {
      const terminal: RunSnapshotPhase = loaded.status === 'unavailable'
        ? { phase: 'unavailable', reason: loaded.reason, message: loaded.message }
        : { phase: loaded.status, message: loaded.message };
      options.onPhase(terminal);
      return terminal;
    }

    try {
      const resolved = await this.presentationCache.resolve(
        loaded.header,
        loaded.document,
        isCurrent,
        this.organizeFn,
      );
      if (!isCurrent() || !resolved) return { phase: 'idle' };
      const ready: RunSnapshotPhase = {
        phase: 'ready',
        header: loaded.header,
        document: resolved.document,
        layoutComputed: resolved.layoutComputed,
      };
      options.onPhase(ready);
      return ready;
    } catch (error) {
      if (!isCurrent()) return { phase: 'idle' };
      const failed: RunSnapshotPhase = {
        phase: 'error',
        message: error instanceof Error ? error.message : String(error),
      };
      options.onPhase(failed);
      return failed;
    }
  }
}
