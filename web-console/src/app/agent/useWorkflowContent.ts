import { useEffect, useRef, useState } from 'react';
import type { RuntimeClientCheckpoint, RuntimeClientSnapshot, WorkflowRunId, WorkflowRunView, WorkflowSnapshot } from '../../../../protocol/app-server/v78';
import { checkpointReadCutKey } from '../../../../protocol/app-server/content-identity';
import { parseObservedValue } from '../../../../protocol/app-server/domain-observation';
import { validateCanonicalInspectionPage } from '../../../../protocol/app-server/trace-inspection';
import type { AppServerClient } from '../../client/app-server';
import type { SessionView } from '../../client/contracts';
import type { TraceFieldReader } from '../../client/trace';
import { readTraceCollection, readTraceValue } from '../../client/trace-content';
import { sameWorkflowRunId } from '../workflows/run-canvas/identity';

/** Restore finite Workflow display headers from their same immutable native cut. */
export type WorkflowView = Pick<SessionView, 'id' | 'target' | 'attachmentObservation'> & {
  snapshot?: Pick<RuntimeClientSnapshot, 'workflows'>;
  checkpoint?: Pick<RuntimeClientCheckpoint, 'read_cut' | 'collections'>;
};
const MAX_BOUNDED_INSTANCE_PAGES = 4;
const workflowFrontiers = (view: WorkflowView | undefined) => view?.checkpoint?.collections.filter(item => item.domain === 'workflows.runs' || /^workflows\.runs\.@\d+\.instances$/.test(item.domain));
const instanceFrontierIndex = (domain: string): number | undefined => {
  const match = /^workflows\.runs\.@(\d+)\.instances$/.exec(domain);
  return match ? Number(match[1]) : undefined;
};

/** Unrelated conversation publications must not rerender header chrome. */
export function sameWorkflowView(a: WorkflowView | undefined, b: WorkflowView | undefined): boolean {
  return a === b || !!a && !!b && a.id === b.id && a.target === b.target && a.attachmentObservation === b.attachmentObservation
    && a.snapshot?.workflows?.revision === b.snapshot?.workflows?.revision
    && a.checkpoint?.read_cut.activation_id === b.checkpoint?.read_cut.activation_id
    && JSON.stringify(workflowFrontiers(a)) === JSON.stringify(workflowFrontiers(b));
}

export interface WorkflowContentOptions {
  /** Hydrate omitted `workflows.runs.@N.instances` pages ONLY when explicitly scoped (e.g. open Run Canvas). */
  readonly hydrateRunId?: WorkflowRunId;
  /** Tight upper bound on `workflows.runs.@N.instances` pages; defaults to 0 so Chat headers never eagerly pull omitted history. */
  readonly maxInstancePages?: number;
}

export function useWorkflowContent(
  client: AppServerClient | undefined,
  view: WorkflowView | undefined,
  active: boolean,
  agentId?: string,
  options?: WorkflowContentOptions,
) {
  const snapshot = view?.snapshot?.workflows;
  const frontiers = workflowFrontiers(view);
  const runsFrontier = frontiers?.find(item => item.domain === 'workflows.runs');
  const maxInstancePages = Math.min(Math.max(0, options?.maxInstancePages ?? 0), MAX_BOUNDED_INSTANCE_PAGES);
  const instanceHydrationEnabled = maxInstancePages > 0;
  const incomplete = Boolean(
    runsFrontier?.next_offset != null
    || frontiers?.some(item => item.next_offset != null),
  );
  const hydrateRunKey = options?.hydrateRunId ? JSON.stringify(options.hydrateRunId) : '';
  const scope = JSON.stringify([view?.target, view?.attachmentObservation, agentId, view?.checkpoint?.read_cut.activation_id]);
  const identity = JSON.stringify([scope, snapshot?.revision, frontiers, active, hydrateRunKey, maxInstancePages]);
  const latest = useRef(identity); latest.current = identity;
  const [value, setValue] = useState<{ owner: AppServerClient; identity: string; scope: string; snapshot?: WorkflowSnapshot; error?: string }>();
  useEffect(() => {
    const target = view?.target, cut = view?.checkpoint?.read_cut, admission = view?.attachmentObservation;
    const frontier = frontiers?.find(item => item.domain === 'workflows.runs');
    const instanceFrontiers = instanceHydrationEnabled
      ? (frontiers ?? []).filter(item => item.next_offset != null && instanceFrontierIndex(item.domain) !== undefined)
      : [];
    if (!client || !active || !snapshot) return;
    if (!incomplete) { setValue({ owner: client, identity, scope, snapshot }); return; }
    if (!view || !target || !cut || (!frontier && instanceFrontiers.length === 0)) return;
    let mounted = true;
    const current = () => mounted && latest.current === identity && client.isAttachmentObservationCurrent(view.id, admission);
    const reader: TraceFieldReader = {
      page: async (cut, domain, offset, current) => (await client.request({ method: 'conversation/checkpointPage', params: { target, agent_id: agentId ?? null, cut, domain, offset, limit: 32 } }, 'checkpoint_page', undefined, current)).page,
      value: async (cut, field, offset, current) => (await client.request({ method: 'conversation/checkpointValueRead', params: { target, agent_id: agentId ?? null, cut, ...field, offset, max_bytes: 65536 } }, 'checkpoint_value_chunk', undefined, current)).chunk,
      content: async (_cut, ref, offset, current) => client.reads.readContent(target, agentId, ref, offset, current),
    };
    void (async () => {
      const runs: WorkflowRunView[] = [];
      const needRunsCollection = Boolean(frontier && (frontier.next_offset != null || !instanceHydrationEnabled));
      if (frontier && needRunsCollection) {
        const items = await readTraceCollection(reader, cut, frontier, current);
        for (const item of items) {
          const run = item.deferred || item.value == null ? parseObservedValue((await readTraceValue(reader, cut, { domain: frontier.domain, item_offset: Number(item.offset), path: [] }, current)).text) : item.value;
          runs.push(run as WorkflowRunView);
        }
      } else {
        runs.push(...snapshot.runs);
      }
      for (const instFrontier of instanceFrontiers) {
        const runIndex = instanceFrontierIndex(instFrontier.domain);
        if (runIndex === undefined || !runs[runIndex]) continue;
        const targetRun = runs[runIndex];
        if (options?.hydrateRunId && !sameWorkflowRunId(targetRun.id, options.hydrateRunId)) continue;
        const expectedTotal = Number(instFrontier.total);
        if (targetRun.omitted_instances === 0 && targetRun.instances.length >= expectedTotal) continue;
        const nextOffsetNum = instFrontier.next_offset == null ? 0 : Number(instFrontier.next_offset);
        const startOffset = nextOffsetNum > 0 && nextOffsetNum === targetRun.instances.length ? nextOffsetNum : 0;
        const hydrated = startOffset > 0 ? [...targetRun.instances] : [];
        let offset: number | null = startOffset;
        let pages = 0;
        while (offset !== null && pages < maxInstancePages && current()) {
          const page = await reader.page(cut, instFrontier.domain, offset, current);
          if (!current()) return;
          if (page.domain !== instFrontier.domain || checkpointReadCutKey(page.read_cut) !== checkpointReadCutKey(cut) || page.total !== instFrontier.total) {
            throw Error('Original Workflow instance page changed.');
          }
          validateCanonicalInspectionPage(page, offset, instFrontier.total, offset !== 0 && BigInt(offset) < BigInt(instFrontier.total));
          for (const item of page.items) {
            const inst = item.deferred || item.value == null
              ? parseObservedValue((await readTraceValue(reader, cut, { domain: instFrontier.domain, item_offset: Number(item.offset), path: [] }, current)).text)
              : item.value;
            hydrated.push(inst as WorkflowRunView['instances'][number]);
          }
          pages += 1;
          offset = page.next_offset == null ? null : Number(page.next_offset);
        }
        if (!current()) return;
        runs[runIndex] = {
          ...targetRun,
          instances: hydrated,
          omitted_instances: Math.max(0, expectedTotal - hydrated.length),
        };
      }
      if (current()) setValue({ owner: client, identity, scope, snapshot: { ...snapshot, runs } });
    })().catch(error => { if (current()) setValue(previous => ({ owner: client, identity, scope,
      snapshot: previous?.owner === client && previous.scope === scope ? previous.snapshot : undefined, error: String(error).slice(0, 512) })); });
    return () => { mounted = false; };
  }, [client, identity]);
  const isCurrentCut = !active || !incomplete || Boolean(value && value.owner === client && value.identity === identity && !value.error);
  const currentSnapshot = !active || !incomplete
    ? snapshot
    : (value && value.owner === client && value.identity === identity && !value.error ? value.snapshot : undefined);
  const retained = active && incomplete && value && value.owner === client && value.scope === scope ? value.snapshot : undefined;
  return {
    snapshot: retained ?? snapshot,
    currentSnapshot,
    isCurrentCut,
    error: value && value.owner === client && value.identity === identity ? value.error : undefined,
    loading: !!active && !!incomplete && (!value || value.owner !== client || value.identity !== identity),
  };
}
