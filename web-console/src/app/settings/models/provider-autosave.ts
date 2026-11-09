import type { ProviderWrite } from '../../../../../protocol/app-server/v77';
import type { SettingsTargetActor } from '../machines/system';
import { admitsSourceMutation } from '../machines/settings-target';
import { providerMutation } from './ProviderTemplates';

const IDLE_MS = 1200;
const pendingSaves = new WeakMap<SettingsTargetActor, Map<string, { flush(): void; hold(): void; cancel(): void }>>();

/** Schedule one draft generation, even if its Provider detail unmounts. The
 * existing transaction owns the value and the target still owns CAS and I/O. */
export function scheduleProviderSave(actor: SettingsTargetActor, id: string, identity: string, trigger: 'idle' | 'blur' | 'now' = 'idle') {
  const unit = actor.getSnapshot().context.units[identity];
  if (!unit) return;
  let pending = pendingSaves.get(actor);
  if (!pending) pendingSaves.set(actor, pending = new Map());
  const saves = pending;
  saves.get(identity)?.cancel();
  const { generation, selector } = unit.getSnapshot().context;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let ready = false;
  const save = { flush, hold, cancel };
  saves.set(identity, save);
  const targetUpdates = actor.subscribe({ next: () => { if (ready) queueMicrotask(trySave); }, complete: cancel });
  const draftUpdates = unit.subscribe({ next: () => { if (ready) queueMicrotask(trySave); }, complete: cancel });

  function cancel() {
    if (saves.get(identity) !== save) return;
    saves.delete(identity);
    clearTimeout(timer);
    targetUpdates.unsubscribe();
    draftUpdates.unsubscribe();
  }

  function trySave() {
    if (!ready || saves.get(identity) !== save) return;
    const { status, context } = actor.getSnapshot();
    const draft = unit.getSnapshot().context;
    if (status !== 'active' || context.units[identity] !== unit || draft.generation !== generation || !draft.draft) {
      cancel();
      return;
    }
    if (!admitsSourceMutation(context)) return;
    // A different local commit may have rebased the draft. A changed source
    // without that proof still requires the user's explicit review.
    const source = context.observation!;
    if (draft.base !== draft.observed || draft.base !== source[source.target.kind]?.revision) return;
    cancel();
    actor.send({ type: 'UNIT.SUBMIT', identity, selector, revision: draft.base, mutation: providerMutation(id, draft.draft.value as ProviderWrite) });
  }

  function flush() {
    clearTimeout(timer);
    ready = true;
    trySave();
  }

  function hold() {
    clearTimeout(timer);
    ready = false;
  }

  if (trigger === 'now') flush();
  if (trigger === 'idle') timer = setTimeout(flush, IDLE_MS);
}

export function flushProviderSave(actor: SettingsTargetActor, identity: string) {
  pendingSaves.get(actor)?.get(identity)?.flush();
}

export function holdProviderSave(actor: SettingsTargetActor, identity: string) {
  pendingSaves.get(actor)?.get(identity)?.hold();
}

export function cancelProviderSave(actor: SettingsTargetActor, identity: string) {
  pendingSaves.get(actor)?.get(identity)?.cancel();
}
