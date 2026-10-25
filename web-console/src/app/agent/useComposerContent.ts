import { useEffect, useRef, useState } from 'react';
import type { TodoTask, RuntimeClientSnapshot } from '../../../../protocol/app-server/v57';
import { parseObservedValue } from '../../../../protocol/app-server/domain-observation';
import type { AppServerClient } from '../../client/app-server';
import type { SessionView } from '../../client/contracts';
import type { TraceFieldReader } from '../../client/trace';
import { readTraceCollection, readTraceValue } from '../../client/trace-content';

/** Finite snapshot headers become the normal dock contents, without read controls. */
export function useComposerContent(client: AppServerClient, view: SessionView | undefined) {
  const frontier = view?.checkpoint?.collections.find(frontier => frontier.domain === 'todos.tasks');
  const goal = view?.snapshot?.goal?.current, todo = frontier && BigInt(frontier.total) > BigInt(frontier.loaded);
  const identity = JSON.stringify([view?.target, view?.attachmentObservation, frontier, goal?.reference, goal?.objective_deferred, goal?.blocked_reason_deferred]);
  const latest = useRef(identity); latest.current = identity;
  const [state, setState] = useState<{ identity: string; todos?: TodoTask[]; goal?: NonNullable<RuntimeClientSnapshot['goal']>; error?: string }>();
  const loaded = state?.identity === identity ? state : undefined;
  useEffect(() => {
    const target = view?.target, cut = view?.checkpoint?.read_cut, admission = view?.attachmentObservation;
    if (!view || !target || !cut || !todo && !goal?.objective_deferred && !goal?.blocked_reason_deferred) return;
    let mounted = true;
    const current = () => mounted && latest.current === identity && client.isAttachmentObservationCurrent(view.id, admission);
    const reader: TraceFieldReader = {
      page: async (cut, domain, offset, current) => (await client.request({method:'conversation/checkpointPage',params:{target,agent_id:null,cut,domain,offset,limit:32}},'checkpoint_page',undefined,current)).page,
      value: async (cut, field, offset, current) => (await client.request({method:'conversation/checkpointValueRead',params:{target,agent_id:null,cut,...field,offset,max_bytes:65536}},'checkpoint_value_chunk',undefined,current)).chunk,
      content: async (_cut, ref, offset, current) => client.reads.readContent(target, undefined, ref, offset, current),
    };
    void (async () => {
      const next: NonNullable<typeof state> = { identity };
      if (todo && frontier) {
        const items = await readTraceCollection(reader, cut, frontier, current), tasks: TodoTask[] = [];
        for (const item of items) {
          const value = item.deferred || item.value == null ? parseObservedValue((await readTraceValue(reader, cut, { domain: frontier.domain, item_offset: Number(item.offset), path: [] }, current)).text) : item.value;
          tasks.push(value as TodoTask);
        }
        next.todos = tasks;
      }
      if (goal?.objective_deferred || goal?.blocked_reason_deferred) {
        const original = { ...goal };
        if (goal.objective_deferred) { original.objective = await client.goalObjective(view.id, goal.reference); original.objective_deferred = false; }
        if (goal.blocked_reason_deferred) {
          const value = await readTraceValue(reader, cut, { domain: 'goal', item_offset: 0, path: ['current', 'blocked_reason'] }, current);
          const reason: unknown = value.representation === 'utf8_text' ? value.text : parseObservedValue(value.text);
          if (typeof reason !== 'string' && reason !== null) throw Error('Original Goal reason is unavailable.');
          original.blocked_reason = reason; original.blocked_reason_deferred = false;
        }
        next.goal = { current: original };
      }
      if (current()) setState(next);
    })().catch(error => { if (current()) setState({ identity, error: String(error).slice(0, 512) }); });
    return () => { mounted = false; };
  }, [client, identity]);
  return { snapshot: loaded && view?.snapshot ? { ...view.snapshot, ...(loaded.todos && view.snapshot.todos ? { todos: { ...view.snapshot.todos, tasks: loaded.todos } } : {}), ...(loaded.goal ? { goal: loaded.goal } : {}) } : view?.snapshot, todoLoaded: loaded?.todos !== undefined, error: loaded?.error };
}
