import { useEffect, useRef, useState } from 'react';
import type { RuntimeClientCheckpoint, RuntimeClientSnapshot, WorkflowRunView, WorkflowSnapshot } from '../../../../protocol/app-server/v51';
import { parseObservedValue } from '../../../../protocol/app-server/domain-observation';
import type { AppServerClient } from '../../client/app-server';
import type { SessionView } from '../../client/contracts';
import type { TraceFieldReader } from '../../client/trace';
import { readTraceCollection, readTraceValue } from '../../client/trace-content';

/** Restore finite Workflow display headers from their same immutable native cut. */
type WorkflowView = Pick<SessionView, 'id' | 'target' | 'attachmentObservation'> & { snapshot?: Pick<RuntimeClientSnapshot, 'workflows'>; checkpoint?: Pick<RuntimeClientCheckpoint, 'read_cut' | 'collections'> };
const workflowFrontiers = (view: WorkflowView | undefined) => view?.checkpoint?.collections.filter(item => item.domain === 'workflows.runs' || /^workflows\.runs\.@\d+\.instances$/.test(item.domain));

/** Unrelated conversation publications must not rerender header chrome. */
export function sameWorkflowView(a: WorkflowView | undefined, b: WorkflowView | undefined): boolean {
  return a === b || !!a && !!b && a.id === b.id && a.target === b.target && a.attachmentObservation === b.attachmentObservation
    && a.snapshot?.workflows?.revision === b.snapshot?.workflows?.revision
    && a.checkpoint?.read_cut.activation_id === b.checkpoint?.read_cut.activation_id
    && JSON.stringify(workflowFrontiers(a)) === JSON.stringify(workflowFrontiers(b));
}

export function useWorkflowContent(client: AppServerClient | undefined, view: WorkflowView | undefined, active: boolean, agentId?: string) {
  const snapshot = view?.snapshot?.workflows;
  const frontiers = workflowFrontiers(view);
  const incomplete = frontiers?.some(item => item.next_offset != null);
  const identity = JSON.stringify([view?.target, view?.attachmentObservation, snapshot?.revision, frontiers, active, agentId, view?.checkpoint?.read_cut.activation_id]);
  const latest = useRef(identity); latest.current = identity;
  const [value, setValue] = useState<{ identity: string; snapshot?: WorkflowSnapshot; error?: string }>();
  useEffect(() => {
    const target = view?.target, cut = view?.checkpoint?.read_cut, admission = view?.attachmentObservation;
    const frontier = frontiers?.find(item => item.domain === 'workflows.runs');
    if (!client || !active || !view || !target || !cut || !frontier || !incomplete || !snapshot) return;
    let mounted = true;
    const current = () => mounted && latest.current === identity && client.isAttachmentObservationCurrent(view.id, admission);
    const reader: TraceFieldReader = {
      page: async (cut, domain, offset, current) => (await client.request({ method: 'conversation/checkpointPage', params: { target, agent_id: agentId ?? null, cut, domain, offset, limit: 32 } }, 'checkpoint_page', undefined, current)).page,
      value: async (cut, field, offset, current) => (await client.request({ method: 'conversation/checkpointValueRead', params: { target, agent_id: agentId ?? null, cut, ...field, offset, max_bytes: 65536 } }, 'checkpoint_value_chunk', undefined, current)).chunk,
      content: async (_cut, ref, offset, current) => client.reads.readContent(target, agentId, ref, offset, current),
    };
    void (async () => {
      const items = await readTraceCollection(reader, cut, frontier, current), runs: WorkflowRunView[] = [];
      for (const item of items) {
        const run = item.deferred || item.value == null ? parseObservedValue((await readTraceValue(reader, cut, { domain: frontier.domain, item_offset: Number(item.offset), path: [] }, current)).text) : item.value;
        runs.push(run as WorkflowRunView);
      }
      if (current()) setValue({ identity, snapshot: { ...snapshot, runs } });
    })().catch(error => { if (current()) setValue({ identity, error: String(error).slice(0, 512) }); });
    return () => { mounted = false; };
  }, [client, identity]);
  return { snapshot: value?.identity === identity ? value.snapshot ?? snapshot : snapshot, error: value?.identity === identity ? value.error : undefined,
    loading: !!active && !!incomplete && value?.identity !== identity };
}
