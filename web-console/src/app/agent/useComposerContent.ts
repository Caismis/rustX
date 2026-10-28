import { useEffect, useRef, useState } from 'react';
import type { TodoTask, RuntimeClientSnapshot } from '../../../../protocol/app-server/v62';
import { parseObservedValue } from '../../../../protocol/app-server/domain-observation';
import type { AppServerClient } from '../../client/app-server';
import type { SessionView } from '../../client/contracts';
import type { TraceFieldReader } from '../../client/trace';
import { readTraceCollection, readTraceValue } from '../../client/trace-content';

function readerFor(client: AppServerClient, target: NonNullable<SessionView['target']>): TraceFieldReader {
  return {
    page: async (cut, domain, offset, current) => (await client.request({method:'conversation/checkpointPage',params:{target,agent_id:null,cut,domain,offset,limit:32}},'checkpoint_page',undefined,current)).page,
    value: async (cut, field, offset, current) => (await client.request({method:'conversation/checkpointValueRead',params:{target,agent_id:null,cut,...field,offset,max_bytes:65536}},'checkpoint_value_chunk',undefined,current)).chunk,
    content: async (_cut, ref, offset, current) => client.reads.readContent(target, undefined, ref, offset, current),
  };
}

/** Todo and Goal hydrate independently inside one native observation owner. */
export function useComposerContent(client: AppServerClient, view: SessionView | undefined) {
  const frontier = view?.checkpoint?.collections.find(frontier => frontier.domain === 'todos.tasks');
  const goal = view?.snapshot?.goal?.current, todo = frontier && BigInt(frontier.total) > BigInt(frontier.loaded);
  const scope = JSON.stringify([view?.target, view?.attachmentObservation]);
  const todoIdentity = JSON.stringify([scope, frontier?.revision, frontier?.total]);
  const goalIdentity = JSON.stringify([scope, goal?.reference, goal?.objective_deferred, goal?.blocked_reason_deferred]);
  const latest = useRef({ todoIdentity, goalIdentity }); latest.current = { todoIdentity, goalIdentity };
  const [todos, setTodos] = useState<{ owner: AppServerClient; identity: string; scope: string; value?: TodoTask[]; error?: string }>();
  const [loadedGoal, setGoal] = useState<{ owner: AppServerClient; identity: string; value?: NonNullable<RuntimeClientSnapshot['goal']>; error?: string }>();
  useEffect(() => {
    const target = view?.target, cut = view?.checkpoint?.read_cut, admission = view?.attachmentObservation;
    if (!view || !target || !cut || !frontier || !view.snapshot?.todos) return;
    if (!todo) {
      setTodos({ owner: client, identity: todoIdentity, scope, value: view.snapshot.todos.tasks ?? [] });
      return;
    }
    if (todos?.owner === client && todos.identity === todoIdentity && todos.value !== undefined && !todos.error) return;
    let mounted = true;
    const current = () => mounted && latest.current.todoIdentity === todoIdentity && client.isAttachmentObservationCurrent(view.id, admission);
    const reader = readerFor(client, target);
    void (async () => {
      const items = await readTraceCollection(reader, cut, frontier, current), tasks: TodoTask[] = [];
      for (const item of items) {
        const value = item.deferred || item.value == null ? parseObservedValue((await readTraceValue(reader, cut, { domain: frontier.domain, item_offset: Number(item.offset), path: [] }, current)).text) : item.value;
        tasks.push(value as TodoTask);
      }
      if (current()) setTodos({ owner: client, identity: todoIdentity, scope, value: tasks });
    })().catch(error => { if (current()) setTodos(previous => ({ owner: client, identity: todoIdentity, scope, value: previous?.owner === client && previous.scope === scope ? previous.value : undefined, error: String(error).slice(0, 512) })); });
    return () => { mounted = false; };
  }, [client, todoIdentity, todo]);
  useEffect(() => {
    const target = view?.target, cut = view?.checkpoint?.read_cut, admission = view?.attachmentObservation;
    if (!view || !target || !cut || !goal || !goal.objective_deferred && !goal.blocked_reason_deferred) return;
    let mounted = true;
    const current = () => mounted && latest.current.goalIdentity === goalIdentity && client.isAttachmentObservationCurrent(view.id, admission);
    const reader = readerFor(client, target);
    void (async () => {
      const original = { ...goal };
      if (goal.objective_deferred) { original.objective = await client.goalObjective(view.id, goal.reference); original.objective_deferred = false; }
      if (goal.blocked_reason_deferred) {
        const value = await readTraceValue(reader, cut, { domain: 'goal', item_offset: 0, path: ['current', 'blocked_reason'] }, current);
        const reason: unknown = value.representation === 'utf8_text' ? value.text : parseObservedValue(value.text);
        if (typeof reason !== 'string' && reason !== null) throw Error('Original Goal reason is unavailable.');
        original.blocked_reason = reason; original.blocked_reason_deferred = false;
      }
      if (current()) setGoal({ owner: client, identity: goalIdentity, value: { current: original } });
    })().catch(error => { if (current()) setGoal({ owner: client, identity: goalIdentity, error: String(error).slice(0, 512) }); });
    return () => { mounted = false; };
  }, [client, goalIdentity]);
  // A same-owner Todo revision keeps its last complete list while refreshing;
  // a replaced observation or an inline/absent Todo always uses native contents.
  const retainedTodos = todo && todos?.owner === client && todos.scope === scope ? todos.value : undefined;
  const currentTodos = todos?.owner === client && todos.identity === todoIdentity ? todos : undefined;
  const currentGoal = loadedGoal?.owner === client && loadedGoal.identity === goalIdentity ? loadedGoal : undefined;
  return {
    snapshot: view?.snapshot && (retainedTodos || currentGoal?.value) ? { ...view.snapshot,
      ...(retainedTodos && view.snapshot.todos ? { todos: { ...view.snapshot.todos, tasks: retainedTodos } } : {}),
      ...(currentGoal?.value ? { goal: currentGoal.value } : {}),
    } : view?.snapshot,
    todoLoaded: currentTodos?.value !== undefined && !currentTodos.error,
    error: currentTodos?.error ?? currentGoal?.error,
  };
}
