import type {
  AttachmentTarget,
  CheckpointReadCut,
  TraceRecord,
  WorkflowRunId,
} from '../../../../../protocol/app-server/v78';
import type { AppServerClient } from '../../../client/app-server';
import type { TraceFieldReader } from '../../../client/trace';

export interface ResolvedWorkflowTraceContext {
  readonly traceRoot: TraceRecord;
  readonly traceCut: CheckpointReadCut;
  readonly traceReader: TraceFieldReader;
}

export type TraceContextStatus = 'loading' | 'ready' | 'unavailable' | 'error';

export function createScopedTraceReader(options: {
  readonly client: AppServerClient;
  readonly target: AttachmentTarget;
  readonly agentId?: string;
  readonly expectedConversationId: string;
  readonly expectedActivationId: string | null;
  readonly owned: () => boolean;
}): TraceFieldReader {
  const { client, target, agentId, expectedConversationId, expectedActivationId, owned } = options;
  const validateCut = (cut: CheckpointReadCut, callerCurrent: () => boolean) => {
    if (!owned() || !callerCurrent()
      || cut.conversation_id !== expectedConversationId
      || (cut.activation_id ?? null) !== expectedActivationId) {
      throw new Error('Trace field scope was replaced.');
    }
  };
  return {
    value: async (cut, field, offset, callerCurrent) => {
      validateCut(cut, callerCurrent);
      const result = await client.request(
        {
          method: 'conversation/checkpointValueRead',
          params: {
            target,
            agent_id: agentId ?? null,
            cut,
            domain: field.domain,
            item_offset: field.item_offset,
            path: field.path,
            offset,
            max_bytes: 65536,
          },
        },
        'checkpoint_value_chunk',
        undefined,
        () => owned() && callerCurrent(),
      );
      validateCut(cut, callerCurrent);
      return result.chunk;
    },
    content: async (cut, reference, offset, callerCurrent) => {
      validateCut(cut, callerCurrent);
      if (reference.conversation_id !== cut.conversation_id) {
        throw new Error('Trace content source changed.');
      }
      return client.reads.readContent(target, agentId, reference, offset, () => owned() && callerCurrent());
    },
    page: async (cut, domain, offset, callerCurrent) => {
      validateCut(cut, callerCurrent);
      const result = await client.request(
        {
          method: 'conversation/checkpointPage',
          params: { target, agent_id: agentId ?? null, cut, domain, offset, limit: 32 },
        },
        'checkpoint_page',
        undefined,
        () => owned() && callerCurrent(),
      );
      validateCut(cut, callerCurrent);
      return result.page;
    },
    workflowNode: async (cut, instance, callerCurrent) => {
      validateCut(cut, callerCurrent);
      const result = await client.request(
        {
          method: 'conversation/workflowTraceNode',
          params: { target, agent_id: agentId ?? null, cut, instance },
        },
        'workflow_trace_node',
        undefined,
        () => owned() && callerCurrent(),
      );
      validateCut(cut, callerCurrent);
      return result.node ?? null;
    },
  };
}

/** Resolve an admitted run through the native index, including old runs and
 * IDs omitted from bounded inline headers. The returned root, cut and reader
 * form one inspection source; no unrelated history or values are scanned. */
export async function resolveWorkflowTraceForRun(options: {
  readonly client: AppServerClient;
  readonly attachmentTarget: AttachmentTarget;
  readonly agentId?: string;
  readonly expectedConversationId: string;
  readonly expectedActivationId: string | null;
  readonly runId: WorkflowRunId;
  readonly owned: () => boolean;
  readonly readOwned: () => boolean;
}): Promise<ResolvedWorkflowTraceContext | null> {
  const { client, attachmentTarget, agentId, expectedConversationId, expectedActivationId, runId, owned, readOwned } = options;
  if (!owned()) return null;
  if (runId.conversation_id !== expectedConversationId) throw Error('Workflow run conversation changed.');
  const { location } = await client.request(
    agentId
      ? { method: 'agent/traceLocateWorkflow', params: { target: attachmentTarget, agent_id: agentId, run_id: runId } }
      : { method: 'session/traceLocateWorkflow', params: { target: attachmentTarget, run_id: runId } },
    'trace_workflow_location',
    undefined,
    owned,
  );
  if (!owned() || !location) return null;
  const cut = location.page.read_cut;
  const root = location.page.records.find(record => record.id === location.record_id);
  if (!cut || cut.conversation_id !== expectedConversationId
    || (cut.activation_id ?? null) !== expectedActivationId
    || root?.kind !== 'workflow'
    || !root.collections.some(collection => collection.domain.endsWith('.workflow.nodes'))) {
    throw Error('Original Workflow trace inspection source changed.');
  }
  return {
    traceRoot: root,
    traceCut: cut,
    traceReader: createScopedTraceReader({ client, target: attachmentTarget, agentId, expectedConversationId, expectedActivationId, owned: readOwned }),
  };
}
