import { useTranslation } from '../../../locale/react';
import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useForm, type ReactFormExtendedApi } from '@tanstack/react-form';
import { useSelector } from '@xstate/react';
import type { SourceMutation } from '../../../../../protocol/app-server/v80';
import { Button } from '../../../presentation/primitives/Button';
import { SettingsPageFooter } from '../../../presentation/settings/SettingsPageShell';
import { SettingsNotice } from '../SettingsFeedback';
import { useNavigationGuard } from './guard';
import { SourceContext } from '../source-context';
import { useSettingsActor, useUnitTransaction } from '../machines/react';
import { admitsSourceMutation, committedUnit, unitOutcome, type MutationOutcome } from '../machines/settings-target';
import { awaitingCommitObservation, committed as unitCommitted, discardable, editingCompletion, requiresReview, unsavedIntent } from '../machines/unit-transaction';
import {
  revisionSelector, selectedRevision, shadowedDefinition, unitFacts, unitOwnership,
  type RevisionSelector, type ShadowedDefinition,
} from '../projection';
import { ConfirmAction } from '../primitives/aria';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The typed form instance of one semantic unit's editor.
 *
 * TanStack Form is the field-mechanics layer and nothing more: nested-object
 * and array editing, typed field state, precise subscriptions and the local
 * touched/dirty metadata a form needs to present itself. It is never the
 * configuration transaction owner — see `useUnitEditing`. */
export type TypedUnitForm<T> = ReactFormExtendedApi<
  T, undefined, undefined, undefined, undefined, undefined, undefined, undefined, undefined, undefined, undefined, never>;

/** The owner-driven reset generation of the enclosing semantic unit: it
 * changes exactly when the unit's transaction owner discards its intent,
 * accepts a reviewed revision, or retires after a discard or a settled commit.
 * An editor holding a local buffer that never becomes the draft — unparsed
 * JSON text — resynchronizes on it even when the value it presents is
 * unchanged. A new transaction, a field edit and a source observation never
 * change it. Outside a unit it is constant. */
export const UnitReset = createContext(0);

/** The page-level editing transaction.
 *
 * A Settings or resource detail page owns ONE visible commit: every
 * `useUnitEditing` on the page registers here, every field edit stays a draft,
 * and the fixed page footer is the only place the batch is saved or
 * abandoned. Reset/restore gestures are staged exactly like drafts — they
 * preview the fallback value and send nothing until Save.
 *
 * Saves run strictly sequentially: the Settings authority admits one source
 * mutation at a time (`admitsSourceMutation`), and `adoptProjection` rebases
 * sibling `rustx.toml` units after each commit, so submitting unit n+1 only
 * after unit n settles keeps every CAS fence exact. A conflict or rejection
 * pauses the queue and surfaces in the footer with adopt-latest / retry. */
export interface PageTransaction {
  readonly dirty: boolean;
  readonly saving: boolean;
  /** Bumps when Cancel restores every unit — buffered field editors
   * (unparsed JSON text) resynchronize on it like `UnitReset`. */
  readonly generation: number;
  readonly savePage: () => void;
  readonly cancelPage: () => void;
  /** Stages or clears a batch of module-level reset mutations (for the
   * Workspace "Follow global" switch) inside the enclosing page transaction. */
  readonly setModuleReset: (mutations: readonly SourceMutation[] | undefined, onCancel?: () => void) => void;
  /** The module-level reset mutations still staged for this page's Save.
   * Read-only page intent: it survives an unmount/remount of the switch that
   * staged it exactly like a detached unit draft. */
  readonly moduleReset: readonly SourceMutation[] | undefined;
  /** Discards all unit drafts and page-local drafts without invoking
   * `setModuleReset`'s cancellation callback. */
  readonly discardDrafts: () => void;
  /** A paused save: this unit's submission was refused. `adopt` accepts the
   * authoritative revision (discarding the intent); `retry` rebases the kept
   * intent on the reviewed revision and resubmits it. */
  readonly conflict?: { identity: string; adopt(): void; retry(): void };
  /** The last page-Save batch left module-reset mutations refused (rejected or
   * unknown outcome). The staged mutations stay pending; `retryModuleReset`
   * rebases them on the observed revision and saves again, `abandonModuleReset`
   * releases the intent and reports abandonment to the staging control. */
  readonly moduleResetError?: { kind: 'rejected' | 'uncertain'; detail?: string };
  readonly retryModuleReset: () => void;
  readonly abandonModuleReset: () => void;
  /** Internal: a `useUnitEditing` result registers its live handle. */
  readonly register: (identity: string, handle: PageUnitHandle) => () => void;
  /** Internal: a unit reports its unsaved-intent flag so the page stays dirty
   * even when the intent predates this mount (restored intent). */
  readonly reportDraft: (identity: string, hasDraft: boolean) => void;
  /** A page-local draft that has no unit of its own (client preferences,
   * an unsaved import). `save`/`cancel` run inside the page batch.
   * `beforeSave` runs synchronously at save start — a local buffer that
   * becomes a unit draft (a JSON editor applying its text) commits through
   * the same queue; returning `false` vetoes the save.
   * Returns the entry's unregistration: an editor's cleanup MUST release its
   * slot, or an unmounted editor's veto keeps blocking the page's Save with no
   * visible error. Only the latest registrant owns the slot — a superseded
   * entry's unregistration is a no-op. */
  /** A save callback may return the write's promise: the page then keeps
   * `saving` set until the write settles, so editing state during submit and
   * settle, double-submit protection and leave-guarding all read the same
   * transaction state. */
  readonly setLocal: (dirty: boolean, save?: () => void | Promise<void>, cancel?: () => void, beforeSave?: () => boolean | void) => () => void;
  /** Danger actions an editor publishes into the page footer's left slot.
   * The footer is owned by whoever mounts it — a page shell or a test
   * harness — so the editor never renders a second one. */
  readonly footerLeading?: ReactNode;
  readonly setFooterLeading: (leading: ReactNode | undefined) => void;
}

/** The page only ever submits, discards and reviews a unit — the handle
 * type is narrowed accordingly because `UnitEditing` is invariant in its
 * value type through `edit`/`apply`. */
export interface PageUnitHandle {
  current: { readonly draft: boolean; submit(remove?: boolean): void; discard(): void; review(): void };
}

const PageTransactionContext = createContext<PageTransaction | null>(null);

export function usePageTransaction(): PageTransaction | null {
  return useContext(PageTransactionContext);
}

/** Mount once per page that stages drafts. Registers the page with the
 * navigation guard so leaving a dirty page asks first, and owns the Cmd/Ctrl+S
 * shortcut. */
export function PageTransactionProvider({ children, onAllSaved }: { children: ReactNode; onAllSaved?: () => void }) {
  const actor = useSettingsActor();
  const guard = useNavigationGuard();
  // Latest UnitEditing handle per registered identity — never a stale
  // snapshot, because `useUnitEditing` refreshes `handle.current` each render.
  const handles = useRef(new Map<string, Set<PageUnitHandle>>());
  // A unit unregistered while its draft was still live: the editor is gone
  // but the transaction actor still owns intent this page's Save, Cancel and
  // navigation guard must reach. The detached handle keeps submitting and
  // discarding it; a subscription releases it once the draft resolves.
  const detached = useRef(new Map<string, PageUnitHandle>());
  const detachedSubs = useRef(new Map<string, { unsubscribe(): void }>());
  const [moduleReset, setModuleResetState] = useState<readonly SourceMutation[]>();
  // The staged reset mirrored into a ref: savePage reads the ref, so a
  // gesture that stages and saves in the same tick never races React state.
  const moduleResetRef = useRef<readonly SourceMutation[] | undefined>(undefined);
  const stageModuleReset = useCallback((mutations: readonly SourceMutation[] | undefined) => {
    const next = mutations && mutations.length > 0 ? mutations : undefined;
    moduleResetRef.current = next;
    setModuleResetState(next);
  }, []);
  const moduleResetCancelRef = useRef<(() => void) | undefined>(undefined);
  // Module-reset mutations refused during the current save run. They are
  // re-staged when the queue drains: a refused removal is still the user's
  // intent, so the switch stays pending and Save can retry it.
  const resetFailuresRef = useRef<SourceMutation[]>([]);
  const [moduleResetError, setModuleResetError] = useState<PageTransaction['moduleResetError']>();
  const [queue, setQueue] = useState<
    ({ kind: 'unit'; id: string } | { kind: 'mutation'; id: string; mutation: SourceMutation; selector: RevisionSelector })[]
  >([]);
  const dispatchedRef = useRef(new Set<string>());
  const [conflict, setConflict] = useState<PageTransaction['conflict']>();
  const [generation, setGeneration] = useState(0);
  // Page-local drafts (client preferences, unsaved imports) carry only their
  // dirty flag in state; the save/cancel gestures live in refs so reporting
  // the same dirty state twice never triggers a render.
  const [localDirty, setLocalDirty] = useState(false);
  const localEntryRef = useRef<{ save?: () => void; cancel?: () => void; beforeSave?: () => boolean | void } | undefined>(undefined);
  const localSaveRef = useRef<(() => void | Promise<void>) | undefined>(undefined);
  const localCancelRef = useRef<(() => void) | undefined>(undefined);
  const [localSaving, setLocalSaving] = useState(false);
  const localSavingRef = useRef(false);
  localSavingRef.current = localSaving;
  const saveActiveRef = useRef(false);
  const saveFailedRef = useRef(false);
  const pendingConflictsRef = useRef<NonNullable<PageTransaction['conflict']>[]>([]);
  const localBeforeSaveRef = useRef<(() => boolean | void) | undefined>(undefined);
  const queueRef = useRef(queue);
  queueRef.current = queue;
  const onAllSavedRef = useRef(onAllSaved);
  onAllSavedRef.current = onAllSaved;
  const liveHandle = useCallback((identity: string) =>
    handles.current.get(identity)?.values().next().value ?? detached.current.get(identity), []);
  const dismissConflict = useCallback(() => {
    setConflict(pendingConflictsRef.current.shift());
  }, []);
  const offerConflict = useCallback((entry: NonNullable<PageTransaction['conflict']>) => {
    setConflict(current => {
      if (!current) return entry;
      pendingConflictsRef.current.push(entry);
      return current;
    });
  }, []);

  // Each unit reports its own unsaved-intent flag: registered refs alone
  // leave the guard blind to a draft that was restored while its editor was
  // unmounted, because registration produces no actor event to re-run a
  // selector on. The flag is `unsavedIntent`, not a bare draft flag: a draft
  // already submitted (in-flight or awaiting its settling observation) is
  // owned by the app-level actor and survives page unmount, so leaving must
  // not ask about it — that was the post-Save guard race.
  const [draftFlags, setDraftFlags] = useState<ReadonlyMap<string, boolean>>(new Map());
  const reportDraft = useCallback((identity: string, hasDraft: boolean) => {
    setDraftFlags(current => current.get(identity) === hasDraft ? current : new Map(current).set(identity, hasDraft));
  }, []);
  const anyDraft = [...draftFlags.values()].some(Boolean);
  const dirty = anyDraft || !!moduleReset?.length || localDirty;
  const saving = queue.length > 0 || localSaving;

  const submitItem = useCallback((item: (typeof queue)[number]) => {
    const ctx = actor.getSnapshot().context;
    if (!admitsSourceMutation(ctx)) return;
    dispatchedRef.current.add(item.id);
    if (item.kind === 'unit') {
      liveHandle(item.id)?.current.submit(false);
      return;
    }
    // A module reset is removal intent on the semantic unit itself, keyed by
    // the mutation shape. A leftover transaction of an earlier gesture (a
    // refused reset never discarded) still pins its stale CAS base — release it
    // first so this submit starts from the current revision.
    actor.send({ type: 'UNIT.DISCARD', identity: item.id });
    const obs = ctx.observation ?? ctx.staleObservation;
    const rev = obs ? selectedRevision(obs, item.selector) : '';
    actor.send({ type: 'UNIT.SUBMIT', identity: item.id, selector: item.selector, revision: rev, mutation: item.mutation });
  }, [actor, liveHandle]);

  const finishSave = useCallback(() => {
    const failed = saveFailedRef.current;
    saveFailedRef.current = false;
    saveActiveRef.current = false;
    const refused = resetFailuresRef.current;
    resetFailuresRef.current = [];
    if (refused.length) stageModuleReset(refused);
    else moduleResetCancelRef.current = undefined;
    dispatchedRef.current.clear();
    if (!failed) {
      const pending = localSaveRef.current?.();
      if (pending && typeof (pending as Promise<void>).then === 'function') {
        setLocalSaving(true);
        void Promise.resolve(pending).catch(() => undefined).finally(() => setLocalSaving(false));
      }
      onAllSavedRef.current?.();
    }
  }, [stageModuleReset]);

  // The save queue advances one unit at a time: submit, wait for the unit's
  // own settle (or its transaction retiring after a definitive commit), then
  // the next. A refusal pauses the whole batch and reports in the footer.
  const head = queue[0];
  const headStatus = useSelector(actor, target => {
    if (!head) return 'idle' as const;
    if (!dispatchedRef.current.has(head.id)) return 'waiting' as const;
    const ref = target.context.units[head.id];
    // A queued submission refused by admission spawns no unit actor —
    // `ensureUnit` only runs on the accepted transition — so an absent ref
    // means 'settled' only while submission is also free to proceed now.
    // Otherwise the head waits for the barrier and retries, never silently
    // dropping its mutation.
    if (!ref) return admitsSourceMutation(target.context) ? 'settled' as const : 'waiting' as const;
    const snapshot = ref.getSnapshot();
    // `acknowledged` is a definitive commit still owed an observation — that
    // debt is the unit's, not the queue's: the write already answered.
    if (snapshot.matches({ mutation: 'settled' }) || snapshot.matches({ mutation: 'acknowledged' })) return 'settled' as const;
    const outcome = unitOutcome(target, head.id);
    if (outcome.kind === 'conflict' || outcome.kind === 'rejected' || outcome.kind === 'uncertain') return outcome.kind;
    // 'waiting' is only a unit whose submission was refused while another held
    // the barrier — it never entered a mutation. A unit still mutating stays
    // 'pending' even in the transient where `context.submission` has already
    // cleared; retrying there would resubmit it forever.
    if (snapshot.matches({ mutation: 'idle' }) && target.context.submission?.identity !== head.id) return 'waiting' as const;
    return 'pending' as const;
  });
  // The submission barrier is held while `submission` or `unobservedSettlement`
  // is set (see `admitsSourceMutation`); when both clear a refused head retries.
  const barrierFree = useSelector(actor, target =>
    target.context.submission === undefined && target.context.unobservedSettlement === undefined);
  useEffect(() => {
    const [item, ...rest] = queueRef.current;
    if (!item) return;

    if (headStatus === 'settled') {
      dispatchedRef.current.delete(item.id);
      setQueue(rest);
      if (rest.length) submitItem(rest[0]); else finishSave();
    } else if (headStatus === 'waiting' && barrierFree) {
      submitItem(item);
    } else if (headStatus === 'conflict' || headStatus === 'rejected' || headStatus === 'uncertain') {
      dispatchedRef.current.delete(item.id);
      // A refused module reset has no unit draft to keep: it is re-staged
      // when the queue drains (see finishSave) so the gesture is not lost, and
      // the refusal is surfaced to the control that staged it.
      saveFailedRef.current = true;
      if (item.kind === 'mutation' && headStatus !== 'conflict') {
        resetFailuresRef.current.push(item.mutation);
        const outcome = unitOutcome(actor.getSnapshot(), item.id);
        setModuleResetError({ kind: headStatus, detail: outcome.kind === 'rejected' ? outcome.detail : undefined });
      }
      // One unit's failure is its own: the remaining independent units still
      // submit, and the failed one keeps its draft for the footer gesture.
      if (headStatus === 'conflict') {
        if (item.kind === 'unit') {
          const handle = liveHandle(item.id);
          offerConflict(handle ? {
            identity: item.id,
            // Adopt the authoritative revision: abandon this browser's intent
            // entirely. `retry` keeps the intent but rebases it on the
            // reviewed revision first, so its resubmission carries a real CAS.
            adopt: () => { handle.current.discard(); dismissConflict(); },
            retry: () => { handle.current.review(); handle.current.submit(false); dismissConflict(); },
          } : { identity: item.id, adopt: dismissConflict, retry: dismissConflict });
        } else {
          offerConflict({
            identity: item.id,
            adopt: () => {
              actor.send({ type: 'UNIT.DISCARD', identity: item.id });
              moduleResetCancelRef.current?.();
              moduleResetCancelRef.current = undefined;
              stageModuleReset(undefined);
              setConflict(undefined);
            },
            // submitItem discards the leftover transaction and resubmits on
            // the current observed revision — the kept intent rebases.
            retry: () => {
              dismissConflict();
              setQueue(current => [item, ...current]);
              submitItem(item);
            },
          });
        }
      }
      setQueue(rest);
      if (rest.length) submitItem(rest[0]); else finishSave();
    }
  }, [actor, headStatus, barrierFree, submitItem, finishSave, liveHandle, offerConflict, dismissConflict]);

  const savePage = useCallback(() => {
    if (saveActiveRef.current || localSavingRef.current) return;
    // A local `beforeSave` may veto (invalid buffers stay on the page) or
    // seed a unit draft synchronously — read the actors, not the React
    // snapshots captured before it ran.
    if (localBeforeSaveRef.current?.() === false) return;
    resetFailuresRef.current = [];
    saveFailedRef.current = false;
    saveActiveRef.current = true;
    setModuleResetError(undefined);
    const units = actor.getSnapshot().context.units;
    const items: (typeof queue)[number][] = [];
    // Save commits exactly the intent of the editors this page currently
    // presents. A draft detached behind an unmounted editor stays pending —
    // its flag still arms the navigation guard and it re-registers on remount
    // — but it is not this editing task's to commit: submitting it here
    // would write a hidden draft the user can no longer see.
    for (const identity of handles.current.keys()) {
      const ref = units[identity];
      if (ref && ref.getSnapshot().context.draft !== undefined) items.push({ kind: 'unit', id: identity });
    }
    const stagedReset = moduleResetRef.current;
    if (stagedReset?.length) {
      stagedReset.forEach(mutation => {
        // The reset is the semantic unit's removal intent, so its transaction
        // identity is the mutation shape — the same identity its field editor
        // uses — never a positional index that would leak a stale CAS pin
        // across pages and gestures.
        items.push({ kind: 'mutation', id: JSON.stringify(mutation), mutation, selector: revisionSelector(mutation) });
      });
      stageModuleReset(undefined);
    }
    dispatchedRef.current.clear();
    pendingConflictsRef.current = [];
    setConflict(undefined);

    if (!items.length) { finishSave(); return; }
    setQueue(items);
    submitItem(items[0]);
  }, [actor, submitItem, finishSave, stageModuleReset]);

  const discardDrafts = useCallback(() => {
    for (const handle of [...[...handles.current.values()].flatMap(set => [...set]), ...detached.current.values()]) handle.current.discard();
    localCancelRef.current?.();
    setGeneration(generation => generation + 1);
  }, []);

  const cancelPage = useCallback(() => {
    discardDrafts();
    dispatchedRef.current.clear();
    // Release the CAS pins staged/failed reset mutations own on their semantic
    // units, so a later gesture on another page starts from current authority.
    for (const mutation of [...(moduleResetRef.current ?? []), ...resetFailuresRef.current]) {
      actor.send({ type: 'UNIT.DISCARD', identity: JSON.stringify(mutation) });
    }
    resetFailuresRef.current = [];
    setQueue([]);
    saveActiveRef.current = false;
    pendingConflictsRef.current = [];
    setConflict(undefined);
    setModuleResetError(undefined);
    const onCancel = moduleResetCancelRef.current;
    moduleResetCancelRef.current = undefined;
    stageModuleReset(undefined);
    onCancel?.();
  }, [actor, discardDrafts, stageModuleReset]);

  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  const cancelRef = useRef(cancelPage);
  cancelRef.current = cancelPage;
  const saveRef = useRef(savePage);
  saveRef.current = savePage;

  // Leaving a dirty page — another settings page, another scope, a detail
  // back gesture, a primary-nav switch or closing the window — asks first.
  // A queued item counts until its own submission is sent: the head is safe
  // once it is pending/settled (on the wire); every item behind it, and a
  // refused head, is still unsubmitted intent the unmount would drop. Draft
  // units report their own flag; only staged-removal units (no draft) need
  // this queue accounting.
  const unsubmitted = queue.filter((_, index) => index > 0 || headStatus === 'waiting').length;
  const token = useRef<object | undefined>(undefined);
  if (!token.current) token.current = {};
  useEffect(() => { guard.reportSurface(token.current!, dirty || unsubmitted > 0, cancelRef.current); }, [guard, dirty, unsubmitted]);
  useEffect(() => () => guard.reportSurface(token.current!, false), [guard]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
        event.preventDefault();
        if (dirtyRef.current) saveRef.current();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  useEffect(() => () => {
    for (const sub of detachedSubs.current.values()) sub.unsubscribe();
    detachedSubs.current.clear();
  }, []);

  const register = useCallback((identity: string, handle: PageUnitHandle) => {
    // A remounting editor takes back a detached registration for its unit.
    detachedSubs.current.get(identity)?.unsubscribe();
    detachedSubs.current.delete(identity);
    detached.current.delete(identity);
    let registrations = handles.current.get(identity);
    if (!registrations) {
      registrations = new Set();
      handles.current.set(identity, registrations);
    }
    registrations.add(handle);
    return () => {
      const currentRegistrations = handles.current.get(identity);
      currentRegistrations?.delete(handle);
      if (currentRegistrations?.size === 0) handles.current.delete(identity);
      const stillRegistered = handles.current.has(identity);
      // Unmounting the last editor cannot end intent the actor still owns:
      // while a draft is live the unit detaches instead of unregistering.
      const ref = actor.getSnapshot().context.units[identity];
      if (!stillRegistered && ref && ref.getSnapshot().context.draft !== undefined) {
        detached.current.set(identity, handle);
        const sub = ref.subscribe(snapshot => {
          if (snapshot.context.draft !== undefined) return;
          sub.unsubscribe();
          detachedSubs.current.delete(identity);
          if (detached.current.get(identity) === handle) detached.current.delete(identity);
          setDraftFlags(current => {
            if (!current.get(identity)) return current;
            const next = new Map(current); next.delete(identity); return next;
          });
        });
        detachedSubs.current.set(identity, sub);
        return;
      }
      if (!stillRegistered) setDraftFlags(current => {
        if (!current.has(identity)) return current;
        const next = new Map(current); next.delete(identity); return next;
      });
    };
  }, [actor]);

  const setModuleReset = useCallback((mutations: readonly SourceMutation[] | undefined, onCancel?: () => void) => {
    moduleResetCancelRef.current = onCancel;
    stageModuleReset(mutations);
  }, [stageModuleReset]);
  const setLocalEntry = useCallback((nextDirty: boolean, save?: () => void | Promise<void>, cancel?: () => void, beforeSave?: () => boolean | void) => {
    const entry = { save, cancel, beforeSave };
    localEntryRef.current = entry;
    localSaveRef.current = save;
    localCancelRef.current = cancel;
    localBeforeSaveRef.current = beforeSave;
    setLocalDirty(current => current === nextDirty ? current : nextDirty);
    return () => {
      // A superseded entry's cleanup must not erase a newer registration.
      if (localEntryRef.current !== entry) return;
      localEntryRef.current = undefined;
      localSaveRef.current = undefined;
      localCancelRef.current = undefined;
      localBeforeSaveRef.current = undefined;
      setLocalDirty(false);
    };
  }, []);
  const [footerLeading, setFooterLeading] = useState<ReactNode>();

  /** Rebase every still-pending reset intent on the reviewed revision and
   * run the page save again — only failed items, never replayed commits. */
  const retryModuleReset = useCallback(() => {
    setModuleResetError(undefined);
    savePage();
  }, [savePage]);

  /** Abandon the pending reset: release its intent and CAS pins, then tell
   * the staging control the gesture ended so it shows the customized view. */
  const abandonModuleReset = useCallback(() => {
    for (const mutation of [...(moduleResetRef.current ?? []), ...resetFailuresRef.current]) {
      actor.send({ type: 'UNIT.DISCARD', identity: JSON.stringify(mutation) });
    }
    resetFailuresRef.current = [];
    setModuleResetError(undefined);
    const onCancel = moduleResetCancelRef.current;
    moduleResetCancelRef.current = undefined;
    stageModuleReset(undefined);
    onCancel?.();
  }, [actor, stageModuleReset]);

  const transaction: PageTransaction = useMemo(() => ({
    dirty, saving, generation, setModuleReset, moduleReset, discardDrafts,
    savePage, cancelPage, conflict, moduleResetError, retryModuleReset, abandonModuleReset,
    register, reportDraft, setLocal: setLocalEntry,
    footerLeading, setFooterLeading,
  }), [dirty, saving, generation, setModuleReset, moduleReset, discardDrafts, savePage, cancelPage,
    conflict, moduleResetError, retryModuleReset, abandonModuleReset, register, reportDraft, setLocalEntry, footerLeading]);
  return <PageTransactionContext.Provider value={transaction}>{children}</PageTransactionContext.Provider>;
}

/** The page footer's default content: status/conflict on the left,
 * [取消] [保存] on the right, both greying out at `opacity 0.4` while the
 * page is clean. */
export function PageTransactionFooter({ leading, status, cancelAlwaysEnabled = false }: {
  leading?: ReactNode; status?: ReactNode; cancelAlwaysEnabled?: boolean;
}) {
  const tx = useTranslation();
  const page = usePageTransaction();
  if (!page) return null;
  const conflict = page.conflict;
  const footerLeading = leading || page.footerLeading ? <>{leading}{page.footerLeading}</> : undefined;
  return <SettingsPageFooter dirty={page.dirty} cancelEnabled={cancelAlwaysEnabled} saving={page.saving} leading={footerLeading}
    status={conflict ? <span role="alert">
      {tx('settings:unsaved.save-conflict')}
      <Button variant="ghost" size="sm" onClick={conflict.adopt}>{tx('settings:unsaved.adopt-latest')}</Button>
      <Button variant="ghost" size="sm" onClick={conflict.retry}>{tx('settings:bridge.use-reviewed-revision')}</Button>
    </span> : status}
    onCancel={page.cancelPage} onSave={page.savePage} />;
}

/** Structural equality of two authored values.
 *
 * The bridge needs to know whether the value the transaction actor currently
 * owns is still the one this form last reflected into it. Native configuration
 * documents are plain JSON values, so a structural comparison answers exactly
 * that, without depending on key order or object identity. */
function sameValue(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (typeof left !== 'object' || typeof right !== 'object' || left === null || right === null) return false;
  if (Array.isArray(left) !== Array.isArray(right)) return false;
  if (Array.isArray(left) && Array.isArray(right)) {
    return left.length === right.length && left.every((item, index) => sameValue(item, right[index]));
  }
  const a = left as Record<string, unknown>, b = right as Record<string, unknown>;
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const key of keys) if (!sameValue(a[key], b[key])) return false;
  return true;
}

/** Where one whole-identity resource definition's authoring stands in this
 * scope. Each state has its own wording and its own actions; none of them is
 * reached by rendering alone.
 *
 * - `new` — this scope authors nothing and inherits nothing: creating it was
 *   the explicit gesture that opened this editor;
 * - `inherited` — a lower scope's definition is in effect and is only
 *   inspected. No draft exists and the fields are not writable;
 * - `overriding` — an explicit Override began this scope's own definition from
 *   the inherited one. It is a draft; nothing is written until Save;
 * - `authored` — this scope authors the definition. */
export type DefinitionAuthoring = 'new' | 'inherited' | 'overriding' | 'authored';

/** Everything one semantic unit's editing surface needs, owned where it
 * belongs.
 *
 * These facts stay distinct and are never collapsed into one form value:
 *
 * - the native **effective** value, projected from `SourceSettings.resolved`;
 * - this scope's native **authored presence** (`authoredPresent`) — whether its
 *   source holds the unit at all;
 * - this scope's native **authored value** (`authored`) — what native parsed
 *   that unit into. A whole-identity resource lives in its own file, so a file
 *   that exists but does not parse is present with no value: it is still this
 *   scope's definition, replaced or removed on its own revision;
 * - this browser's **override intent**, which exists only after an explicit
 *   Override action or, for a value-inherited semantic unit, an actual edit —
 *   an inherited whole resource definition admits no edit before its explicit
 *   override (`DefinitionAuthoring`);
 * - the **dirty draft** carrying that intent's value;
 * - the exact **CAS base revision** the next write is fenced on.
 *
 * None of those live in a React component or in a TanStack Form instance.
 * They live in the unit's transaction actor, which is owned by the Settings
 * authority actor and outlives this form, the detail view it sits in, the page
 * it belongs to and the whole Settings surface.
 *
 * Rendering an inherited unit shows the native effective value while authoring
 * nothing: no draft is created, Save stays unavailable, and a no-op Save can
 * therefore never turn "no Workspace override" into an explicit empty one.
 * `blank` is only an editing seed for a unit this scope has yet to author; it
 * is never presented as an effective value and never written on its own.
 *
 * Remove is the exact inverse of authoring, not a generic mutation: it exists
 * only while this scope really authors the unit. Authored presence is the
 * native projection fact the call site passes, never a truthiness test —
 * `false`, `[]`, `{}` and `""` are authored values like any other — and never
 * inferred from a parse: an unparsed definition is present. */
export interface UnitEditing<T> {
  readonly identity: string;
  /** The value this editor must present: the browser draft when one exists,
   * otherwise this scope's authored value, otherwise the native inherited one,
   * otherwise the neutral authoring seed. */
  readonly displayed: T;
  /** Reflect a new authored value into the transaction actor. */
  readonly edit: (value: T) => void;
  /** Restore a superseded local edit: if the whole value is back at the
   * authoritative/inherited one the draft withdraws; otherwise the restored
   * value replaces the draft while preserving unrelated edits. */
  readonly restore: (value: (clean: T) => T) => void;
  /** One explicit quick-setting gesture, refused while another draft needs review. */
  readonly apply: (value: T) => void;
  /** Immediate-write commit for auto-save surfaces: identical to `apply` but
   * tolerates a staged draft — the committed value supersedes it. */
  readonly commit: (value: T) => void;
  readonly submit: (remove?: boolean) => void;
  readonly discard: () => void;
  readonly review: () => void;
  readonly draft: boolean;
  /** This scope's source holds the unit, parsed or not. */
  readonly authoredPresent: boolean;
  /** This scope's source holds the unit but native parsed no value from it. */
  readonly unparsed: boolean;
  readonly overriding: boolean;
  readonly inheritance: boolean;
  readonly committed: boolean;
  /** This unit's commit was observed at its exact revision. A definitive
   * commit whose first read diverged is still unresolved and is never saved
   * completion, even though the target has finished its post-write read. */
  readonly settled: boolean;
  /** The exact submission accepted for this editing flow. Never inferred from
   * another editor's outcome or a unit's historical completion. */
  readonly submission?: { identity: string; token: number };
  /** The current draft generation already crossed the native submission
   * boundary once. After a refused outcome it keeps its intent but does not
   * re-earn write authorization: autosave must not replay it — only a further
   * edit or the explicit review gesture clears this. */
  readonly attempted: boolean;
  readonly busy: boolean;
  readonly admitted: boolean;
  readonly reviewNeeded: boolean;
  /** An embedded editor of a shared unit presents no outcome or review
   * chrome of its own (see `UnitOptions.embedded`). */
  readonly embedded: boolean;
  readonly intent: boolean;
  /** This unit's own native outcome. Never another unit's. */
  readonly outcome: MutationOutcome;
  /** See `UnitReset`. */
  readonly resets: number;
  readonly base: string;
  readonly observed: string;
  readonly scope: 'user' | 'workspace';
  readonly facts: ReturnType<typeof unitFacts>;
  readonly configUnit: boolean;
  /** The lifecycle of a whole-identity resource definition; `undefined` for a
   * value-inherited semantic unit. */
  readonly definition?: DefinitionAuthoring;
  /** The lower-scope definition this scope's definition shadows or would
   * shadow, if any. */
  readonly shadowed?: ShadowedDefinition;
  /** This unit's definitive commit still awaits the authoritative observation
   * that settles it. The presented source predates the commit, so it is no
   * base for a new draft: no field, Author or Override gesture edits the unit
   * until the observation arrives. */
  readonly awaitingObservation: boolean;
  /** Whether the fields may change this unit's draft now. */
  readonly writable: boolean;
  /** The one explicit transition from inspecting an inherited definition to
   * authoring this scope's own. Present only while `definition` is
   * `inherited`. */
  readonly override?: () => void;
}

export interface UnitOptions<T> {
  /** Called once only when this flow's accepted identity/token completes.
   * The unit actor independently verifies generation and remaining intent. */
  onSaved?: () => void;
  /** An actor-owned creation draft has a stable identity before its native
   * resource name is chosen. Only its submitted mutation names the resource. */
  draftKey?: string;
  /** The exact value this scope authors for this unit, or `undefined` when it
   * authors none or native parsed none. Call sites pass the native projection
   * without a fallback. */
  authored?: T;
  /** Whether this scope's source holds the unit, independent of whether native
   * parsed it into `authored`. It defaults to `authored !== undefined`, which is
   * exact wherever presence and value come from one parsed document — every
   * `rustx.toml` unit and every MCP identity of a parsed `mcp.toml`. A named
   * Agent is its own file, so its call site passes the file's presence. */
  authoredPresent?: boolean;
  /** The neutral seed for authoring a unit this scope does not have yet. */
  blank: T;
  revision: string;
  mutation: (value: T | null) => SourceMutation;
  /** Adapt one native effective value into this control's authored shape.
   * Returning `undefined` declares the unit non-inheritable in the editor —
   * a Provider credential is never read back from a shadowed definition. */
  inherited?: (effective: unknown) => T | undefined;
  /** The native projection of this unit carries its presence but never its
   * value, because the value is a secret-bearing literal. The form then never
   * displays or copies an existing value: authoring replaces it outright. */
  redacted?: boolean;
  /** A second editor embedded beside the unit's primary editor shares the
   * same transaction (the MCP permissions section inside the connection
   * editor): it edits the shared draft but the outcome and review chrome
   * belong to the primary editor — rendered once, never twice. */
  embedded?: boolean;
}

export function useUnitEditing<T>({ authored, authoredPresent = authored !== undefined, blank, revision, mutation, inherited = value => value as T, redacted = false, draftKey, onSaved, embedded = false }: UnitOptions<T>): UnitEditing<T> {
  const actor = useSettingsActor();
  const source = useContext(SourceContext);
  const scope = source?.target.kind ?? 'user';
  const unitMutation = mutation(null);
  const facts = unitFacts(source, scope, unitMutation);
  const workspace = scope === 'workspace';
  const ownership = unitOwnership(unitMutation);
  const inheritance = workspace && ownership === 'value';
  // The lower-scope definition a whole-identity resource would shadow. It is
  // native authored fact of another document, projected without secrets.
  const shadowed = ownership === 'identity' ? shadowedDefinition(source, scope, unitMutation) : undefined;
  const identity = draftKey ?? JSON.stringify(unitMutation);
  const selector = revisionSelector(unitMutation);
  // The unit's live transaction, owned by the Settings authority actor.
  // `undefined` is exactly "this browser authored nothing for the unit and
  // fences on native authority".
  const snapshot = useUnitTransaction(actor, identity);
  const transaction = snapshot?.context;
  // A live transaction answers for its own commit. Once a settled commit's
  // transaction has retired, the target's mutation outcome still names it.
  const lastCommit = useSelector(actor, target => committedUnit(target));
  const committed = snapshot ? unitCommitted(snapshot) : lastCommit === identity;
  // This unit's own mutation is in flight: its transaction owns the intent
  // until the outcome, so its controls are closed.
  const busy = !!snapshot?.matches({ mutation: 'submitting' });
  // This unit's own commit is definitive but not yet observed. The transaction
  // refuses every edit until it is; the controls say so rather than offering
  // the pre-commit projection as an editing base.
  const awaitingObservation = !!snapshot && awaitingCommitObservation(snapshot);
  // Whether the target admits any new source mutation now. It is one
  // target-wide fact owned by the Settings authority actor: while another
  // unit's mutation is submitting, or a settled mutation still awaits its
  // authoritative observation, this unit stays editable but cannot submit.
  const admitted = useSelector(actor, target => admitsSourceMutation(target.context));
  // The target reports one outcome at a time; this reads only the one that is
  // about this unit, so two independent sections never borrow each other's.
  const outcome = useSelector(actor, target => unitOutcome(target, identity), (left, right) => left.kind === right.kind && JSON.stringify(left) === JSON.stringify(right));
  // A settled transaction may already have retired. An unresolved commit
  // cannot retire, so only the absence of a live transaction allows the
  // target's observed outcome to stand for completed settlement.
  const settled = snapshot ? snapshot.matches({ mutation: 'settled' }) : outcome.kind === 'saved' && outcome.observed;
  const retiredCompletion = useSelector(actor, target => target.context.completions[identity]);
  const completion = snapshot ? editingCompletion(snapshot) : retiredCompletion;
  const [pending, setPending] = useState<{ actor: typeof actor; receipt: { identity: string; token: number } }>();
  const submission = pending?.actor === actor && pending.receipt.identity === identity ? pending.receipt : undefined;
  // A different identity/authority is a different editing flow. Returning to a
  // previously visited identity must not revive its abandoned waiting relation.
  useEffect(() => { setPending(undefined); }, [actor, identity]);
  useEffect(() => {
    if (submission && completion === submission.token) {
      setPending(undefined);
      onSaved?.();
    }
  }, [submission, completion, onSaved]);
  const resets = useSelector(actor, target => target.context.resets[identity] ?? 0);
  const draft = transaction?.draft as { value: T } | undefined;
  const unparsed = authoredPresent && authored === undefined;
  const base = transaction?.base ?? revision;
  const observed = transaction?.observed ?? revision;
  // The native effective value, adapted to this control's authored shape. It is
  // displayed, never copied into authoring state. A redacted unit has no value
  // to adapt, so nothing can be copied out of it by construction.
  const inheritedValue = inheritance && facts.effective.state === 'available' ? inherited(facts.effective.value) : undefined;
  const definition: DefinitionAuthoring | undefined = ownership !== 'identity' ? undefined
    : authoredPresent ? 'authored'
      : !shadowed ? 'new'
        : draft ? 'overriding' : 'inherited';
  // The seed an explicit override begins from. It is displayed while the
  // inherited definition is inspected and is never authoring state until the
  // override transition copies it into a draft.
  const overrideSeed = (shadowed?.seed ?? blank) as T;
  // An unparsed definition of this scope is `authored`: it is displayed from
  // the neutral seed, never from the definition it shadows, and replacing it is
  // an ordinary edit fenced on its own revision.
  // A staged removal previews the post-removal effective value — the
  // inherited one, or the neutral seed — and sends nothing until the page
  // saves. Editing again silently unstages it.
  const page = useContext(PageTransactionContext);
  const cleanValue: T = redacted ? blank
    : authored !== undefined ? authored
      : inheritedValue !== undefined ? inheritedValue
        : definition === 'inherited' ? overrideSeed : blank;
  const displayed: T = draft ? draft.value : cleanValue;
  const writable = definition !== 'inherited' && !awaitingObservation;
  const begin = (value: T) => actor.send({ type: 'UNIT.EDIT', identity, selector, revision, value });
  const submit = (value: T | null) => {
    const previous = actor.getSnapshot().context.submission;
    actor.send({ type: 'UNIT.SUBMIT', identity, selector, revision, mutation: mutation(value) });
    const accepted = actor.getSnapshot().context.submission;
    // send() processes admission synchronously; only a newly accepted native
    // submission can arm completion. Refused gestures create no waiting flow.
    setPending(accepted && accepted !== previous && accepted.identity === identity
      ? { actor, receipt: { identity, token: accepted.token } } : undefined);
  };
  const result: UnitEditing<T> = {
    identity, displayed, outcome, resets, draft: draft !== undefined, authoredPresent, unparsed,
    attempted: transaction?.attempted !== undefined && transaction.attempted === transaction.generation,
    embedded,
    overriding: draft !== undefined || authoredPresent, inheritance, committed, settled, submission, busy, admitted,
    base, observed, scope, facts, configUnit: unitMutation.kind === 'config',
    definition, shadowed, awaitingObservation, writable,
    edit: (value: T) => { if (writable) begin(value); },
    restore: (update: (clean: T) => T) => {
      if (!writable) return;
      const value = update(cleanValue);
      if (sameValue(value, cleanValue)) { setPending(undefined); actor.send({ type: 'UNIT.DISCARD', identity }); }
      else begin(value);
    },
    apply: (value: T) => {
      if (!writable || !admitted || busy || draft || (snapshot && requiresReview(snapshot))) return;
      begin(value);
      submit(value);
    },
    commit: (value: T) => {
      if (!writable || !admitted || busy || (snapshot && requiresReview(snapshot))) return;
      begin(value);
      submit(value);
    },
    override: definition === 'inherited' ? () => { if (!awaitingObservation) begin(overrideSeed); } : undefined,
    submit: (remove = false) => {
      if (!remove && !draft) return;
      // Only the mutation itself carries the authored payload; the transaction
      // is handed the token and the non-sensitive selector that settle it.
      submit(remove ? null : draft!.value);
    },
    discard: () => { setPending(undefined); actor.send({ type: 'UNIT.DISCARD', identity }); },
    review: () => actor.send({ type: 'UNIT.REVIEW', identity }),
    // Whether the current source diverges from the CAS base is the
    // transaction's own fact: it alone knows whether a post-commit observation
    // has completed.
    reviewNeeded: !!snapshot && requiresReview(snapshot),
    // Discard abandons browser authoring intent only. A definitive commit, and
    // the observation and review it still owes, are never offered as a draft.
    intent: !!snapshot && discardable(snapshot),
  };
  // Register with the enclosing page transaction, which batches this unit's
  // draft and staged removal into the page-level Save. The handle carries the
  // latest snapshot of this result so the page never submits a stale one.
  const unit = result;
  const handle = useRef<PageUnitHandle>({ current: unit });
  handle.current.current = unit;
  // `page` is rebuilt every provider render, so the effects key on its stable
  // callbacks: anything broader would unregister/re-register on every render.
  const register = page?.register;
  const reportDraft = page?.reportDraft;
  const unsaved = !!snapshot && unsavedIntent(snapshot);
  useLayoutEffect(() => register?.(identity, handle.current), [register, identity]);
  useEffect(() => { reportDraft?.(identity, unsaved); }, [reportDraft, identity, unsaved]);
  return unit;
}

/** The presentation frame every semantic-unit editor shares: native authored
 * and effective facts, the redaction notice, the CAS disclosure, the conflict
 * review and the Save / Override / Remove / Discard actions.
 *
 * The frame renders the actions; it never owns them. Every one of them is a
 * message to the unit's transaction actor. */
function UnitShell<T>({ title, unit, redacted = false, removable, removalNotice, saveLabel, children, presentation, legendHidden = false, onCancel, onSubmit, explicitSave }: {
  title: string; unit: UnitEditing<T>; redacted?: boolean; removable: boolean;
  removalNotice?: ReactNode; saveLabel?: string | undefined; children: ReactNode;
  presentation?: 'resource' | 'dialog'; legendHidden?: boolean; onCancel?: () => void;
  onSubmit?: () => void;
  /** A write-only field's one explicit save gesture (no autosave, no page
   * footer): commits the staged replacement draft. */
  explicitSave?: () => void;
}) {
  const tx = useTranslation();
  // Portaled editors do not inherit the page fieldset's disabled state.
  // Use the same authoritative readiness facts for their fields and actions.
  const actor = useSettingsActor();
  const availability = useSelector(actor, snapshot => snapshot.context.connection !== 'connected' ? 'offline'
    : snapshot.context.readError ? 'failed' : !snapshot.context.observation ? 'loading' : undefined);
  const unavailable = availability !== undefined;
  const saving = unit.busy || unit.awaitingObservation;
  const workspace = unit.scope === 'workspace';
  /** The unit's own card: the region that owns its in-flight mutation and
   * presents the outcome. A confirmed removal submits, which closes every
   * control in the card — its trigger included — until native answers, so
   * focus settles on the card itself, which stays mounted and enabled through
   * the whole write. */
  const card = useRef<HTMLFormElement>(null);
  // A Workspace removal restores what it shadows: a semantic unit always has an
  // inherited value, and a resource definition has one exactly when a User
  // definition of the same identity exists. Removing a Workspace definition
  // that shadows nothing is a real deletion and is named as one.
  const restoresInherited = workspace && (unit.definition === undefined || unit.shadowed !== undefined);
  const owner = workspace ? tx('settings:extension-detail.workspace') : tx('settings:extension-detail.user');
  const removal = removable && unit.authoredPresent && (restoresInherited
          // A Workspace removal is inheritance, not destruction: it removes the
          // unit this Workspace authors, through exact CAS, and the native
          // inherited value becomes effective again. The effective resource
          // survives, so this is never presented as deleting it.
          ? <span data-removal="override-removal"><ConfirmAction tone="restore" label={tx('settings:bridge.use-global-default-value', { p0: title })} triggerText={tx('settings:models.restore')} disabled={!unit.admitted}
            title={tx('settings:bridge.use-the-global-default-for-value', { p0: title })} confirm={tx('settings:bridge.use-global-default-value', { p0: title })}
            description={tx('settings:forms.restore-help')}
            settle={card} onConfirm={() => unit.submit(true)} /></span>
          // A config unit on a settings page has no authored-removal
          // affordance — there is no "reset to default"; the field simply
          // edits the authored value. Resource identities keep their delete.
          : unit.configUnit && !presentation ? null
          : <span data-removal="authored-removal"><ConfirmAction tone="destructive" label={tx('settings:bridge.remove-value', { p0: title })} triggerText={tx('settings:copy.delete')} disabled={!unit.admitted}
            title={tx('settings:bridge.remove-value-from-value-configuration', { p0: title, p1: owner })} confirm={tx('settings:bridge.remove-value', { p0: title })}
            description={removalNotice ?? tx('settings:forms.delete-help')}
            settle={card} onConfirm={() => unit.submit(true)} /></span>);
  return <UnitReset.Provider value={unit.resets}><form ref={card} tabIndex={-1} aria-label={title} className={`${css.unit}${presentation ? ` ${presentation === 'dialog' ? css.unitDialog : css.unitResource}` : ''}`} data-definition={unit.definition} data-draft={unit.draft || undefined}
    data-authored={unit.facts.authored.state} data-effective={unit.facts.effective.state}
    data-authored-value={unit.definition === 'authored' ? unit.unparsed ? 'unparsed' : 'parsed' : undefined}
    // Only a dialog keeps its own submit affordance; on a page the footer
    // is the single commit and Enter inside a field must not bypass staging.
    onSubmit={event => { event.preventDefault(); if (presentation === 'dialog') { if (onSubmit) onSubmit(); else unit.submit(); } }}>
    <fieldset disabled={unit.busy || unavailable}><legend className={legendHidden ? css.visuallyHidden : undefined}>{title}</legend>
      <div className={presentation === 'dialog' ? css.unitBody : undefined}>
      {presentation === 'dialog' && availability && <SettingsNotice text={tx(`settings:models.authority-${availability}`)}/>}
      {unit.configUnit && unit.facts.authored.state !== 'unavailable' && <>
        <SettingsNotice text={unit.facts.authored.state === 'invalid' ? `${tx('settings:bridge.authored-source-is-invalid')} ${unit.facts.authored.diagnostic}`
          : unit.facts.effective.state === 'invalid' ? `${tx('settings:bridge.native-effective-resolution-failed')} ${unit.facts.effective.diagnostic}` : undefined}/>
      </>}
      {redacted && <p className={css.hint}>{tx('settings:forms.secret-replacement')}</p>}
      {unit.definition && (!presentation || unit.definition === 'inherited' || unit.definition === 'overriding' || unit.unparsed) && <DefinitionNotice unit={unit} />}
      {/* Inspecting an inherited definition shows its safe native facts in the
          same fields, and none of them is writable until the explicit
          override transition. */}
      <fieldset className={css.fields} disabled={!unit.writable}>{children}</fieldset>
      {!unit.embedded && <SettingsNotice text={unit.reviewNeeded && unit.outcome.kind !== 'conflict' && unit.outcome.kind !== 'uncertain' ? tx(unit.committed && !unit.draft ? 'settings:forms.committed-changed-elsewhere' : 'settings:forms.changed-elsewhere') : undefined}
        fallback={<div className={css.review}><p role="status">{tx(unit.committed && !unit.draft ? 'settings:forms.committed-changed-elsewhere' : 'settings:forms.changed-elsewhere')}</p></div>}/>}
      {!unit.embedded && <UnitOutcomeNotice title={title} outcome={unit.outcome} />}
      </div>
      {(presentation === 'dialog' || !!unit.override || (removable && (!presentation ? !workspace && unit.authoredPresent : false)) || (unit.reviewNeeded && !unit.embedded) || !!onCancel || !!explicitSave) && <div className={css.actions}>
        {explicitSave && <Button type="button" variant="primary" aria-label={`${tx('settings:bridge.save')} ${title}`} aria-busy={saving}
          disabled={!unit.draft || !unit.admitted || unit.busy || unit.awaitingObservation} onClick={explicitSave}>
          {saving ? tx('settings:catalog.saving') : tx('settings:bridge.save')}
        </Button>}
        {/* Only a dialog commits itself; a page commits through its footer. */}
        {presentation === 'dialog' && <Button variant="primary" type="submit" aria-label={saveLabel ?? `${tx('settings:bridge.save')} ${title}`} aria-busy={saving} disabled={!unit.draft || !unit.admitted}>
          {saving ? tx('settings:catalog.saving') : saveLabel ?? tx('settings:bridge.save')}
        </Button>}
        {unit.override && <Button type="button" variant="primary"
          aria-label={`${tx('settings:bridge.override')} ${title} ${tx('settings:bridge.in-this-workspace')}`}
          title={tx('settings:bridge.begin-a-workspace-definition-of-value-from-the-inherited-one-not', { p0: title })}
          disabled={unit.awaitingObservation} onClick={unit.override}>{tx('settings:forms.customize')}</Button>}
        {presentation === 'dialog' ? removal : !presentation && removable && !workspace ? removal : null}
        {presentation === 'dialog' && <Button type="button" disabled={!unit.intent} onClick={unit.discard}>{tx('settings:models.reset-form')}</Button>}
        {unit.reviewNeeded && !unit.embedded && <Button type="button" onClick={unit.review}>{tx('settings:bridge.use-reviewed-revision')}</Button>}
        {onCancel && <Button type="button" onClick={() => { unit.discard(); onCancel(); }}>{tx('settings:aria.cancel')}</Button>}
      </div>}
    </fieldset>
  </form></UnitReset.Provider>;
}

/** What one whole-identity resource definition is in this scope, worded for
 * exactly its lifecycle state. */
export function DefinitionNotice<T>({ unit }: { unit: UnitEditing<T> }) {
  const tx = useTranslation();
  const shadowed = unit.shadowed;
  return <>
    {unit.definition === 'inherited' && <p role="status" data-definition-state="inherited">{tx('settings:forms.using-global')}</p>}
    <SettingsNotice text={unit.definition === 'inherited' ? shadowed?.diagnostic : undefined}/>
    {unit.definition === 'overriding' && <p role="status" data-definition-state="overriding">{tx('settings:forms.workspace-customization')}</p>}
    {!!shadowed?.withheld.length && <p className={css.hint}>{tx('settings:forms.reenter-secrets', { names: shadowed.withheld.join(', ') })}</p>}
    {unit.unparsed && <p role="status" data-definition-state="authored">{tx('settings:forms.unreadable')}</p>}
  </>;
}

/** Only outcomes that need attention occupy space in the editor. */
export function UnitOutcomeNotice({ title, outcome }: { title: string; outcome: MutationOutcome }) {
  const tx = useTranslation();
  const text = outcome.kind === 'conflict' ? `${title} ${tx('settings:bridge.was-not-saved-the-source-changed-your-draft-and-base-revision-ar')}`
    : outcome.kind === 'rejected' ? `${title} ${tx('settings:bridge.was-not-saved')} ${outcome.detail}`
    : outcome.kind === 'uncertain' ? `${title} · ${tx('settings:settings.save-outcome-uncertain-authority-is-reread-the-write-is-never-re')}` : undefined;
  return <SettingsNotice text={text} fallback={<p role="alert" className={css.error}>{text}</p>}/>;
}

/** Auto-save flush for `immediate` forms: a change stages the draft and a
 * short debounce commits it, matching how DSH/pi-web/ZCode write scalar
 * settings on every gesture. The debounce gives typing a single commit per
 * pause and gives a superseding gesture a chance to supersede the draft
 * before it is submitted. */
function useImmediateCommit<T>(unit: UnitEditing<T>, enabled: boolean) {
  const latest = useRef(unit);
  latest.current = unit;
  useEffect(() => {
    if (!enabled || !unit.draft || !unit.admitted || unit.busy || unit.awaitingObservation) return;
    // A draft pending review (external divergence or a failed commit) waits
    // for the explicit review gesture — auto-save never retries into it.
    if (unit.reviewNeeded) return;
    // This draft generation already crossed the submission boundary once.
    // Whether its outcome committed, conflicted, was rejected or is unknown,
    // a mount/unmount, a filter switch or a refresh must never re-authorize
    // the write: only a further edit (a newer generation) or `review` does.
    // The attempt record lives in the transaction actor, not this component,
    // so a remounted editor sees the same fact.
    if (unit.attempted) return;
    const timer = setTimeout(() => latest.current.commit(latest.current.displayed), 400);
    return () => clearTimeout(timer);
  }, [enabled, unit.draft, unit.admitted, unit.busy, unit.awaitingObservation, unit.reviewNeeded, unit.attempted, unit.displayed]);
}

/** One native semantic unit's editing surface, with a plain value contract.
 *
 * Used where the unit is a single scalar or a small closed choice, so a typed
 * form layer would add mechanics without removing any. */
export function UnitForm<T>({ title, children, removable = true, removalNotice, saveLabel, presentation, legendHidden, immediate = false, ...options }: UnitOptions<T> & {
  title: string; children: (value: T, change: (value: T) => void, restore: (update: (clean: T) => T) => void) => ReactNode;
  removable?: boolean; removalNotice?: ReactNode; saveLabel?: string;
  /** 'dialog' gives the unit its own Save inside the card; a structured
   * editor uses it where no page footer exists. */
  presentation?: 'resource' | 'dialog';
  /** Inside a `SettingsGroup` the group's own title is the visible heading;
   * the legend stays as the fieldset's accessible name only. */
  legendHidden?: boolean;
  /** Auto-save: every staged draft commits on a short debounce; the form
   * renders no Save affordance. */
  immediate?: boolean;
}) {
  const unit = useUnitEditing(options);
  // A write-only field (redacted secret replacement) is one complete edit:
  // it never commits on a typing pause — only the explicit save gesture.
  const autosave = immediate && !options.redacted;
  useImmediateCommit(unit, autosave);
  return <UnitShell title={title} unit={unit} redacted={options.redacted} removable={removable} removalNotice={removalNotice} saveLabel={saveLabel} presentation={presentation} legendHidden={legendHidden}
    explicitSave={immediate && !!options.redacted ? () => unit.commit(unit.displayed) : undefined}>
    {children(unit.displayed, unit.edit, unit.restore)}
  </UnitShell>;
}

/** One native semantic unit's editing surface, with a typed TanStack Form
 * field layer.
 *
 * This is the ownership bridge, and it is the reason TanStack Form can be
 * adopted without becoming a second configuration authority:
 *
 * 1. Every field change is reflected into the unit's XState transaction actor,
 *    so the durable editing intent never lives only in this component.
 * 2. The form is seeded from the actor-owned displayed value, so remounting
 *    the detail — a page change, a list/detail change, closing and reopening
 *    Settings — rehydrates from the draft the actor still owns.
 * 3. An authoritative native revision update changes `authored`, never the
 *    draft, so a dirty form is never reset by one. The form is resynchronized
 *    only when the actor's own value stops being the one this form last
 *    reflected into it — a discard, a reviewed-revision gesture, or a
 *    definitive commit dropping a confirmed draft.
 * 4. `isDirty` / `isTouched` are presentation metadata and are disposable.
 * 5. Save submits through the unit transaction path, with the exact CAS base
 *    the transaction owns; this component never calls a native operation.
 *
 * There is therefore exactly one value called the draft, and the actor owns it. */
export function TypedUnitForm<T>({ title, children, removable = true, removalNotice, presentation, legendHidden, onCancel, onSaved, immediate = false, ...options }: UnitOptions<T> & {
  title: string; children: (form: TypedUnitForm<T>) => ReactNode;
  removable?: boolean; removalNotice?: ReactNode;
  presentation?: 'resource' | 'dialog'; legendHidden?: boolean; onCancel?: () => void;
  /** Auto-save: every staged draft commits on a short debounce. */
  immediate?: boolean;
}) {
  const unit = useUnitEditing({ ...options, onSaved });
  const autosave = immediate && !options.redacted;
  useImmediateCommit(unit, autosave);
  // The value this form last pushed into the transaction actor. Comparing the
  // actor's current value against it — rather than against the form's own
  // state — is what tells an external change apart from this form's own edit.
  const reflected = useRef<T>(unit.displayed);
  // The latest unit facts, for the change listener the form instance holds.
  const current = useRef(unit);
  current.current = unit;
  const form = useForm({
    defaultValues: unit.displayed,
    onSubmit: () => {
      const owner = current.current;
      if (!owner.draft || !owner.admitted || owner.busy || owner.awaitingObservation) return;
      owner.submit();
    },
    listeners: {
      onChange: ({ formApi }) => {
        const { writable, displayed, edit } = current.current;
        // An inspected inherited definition, or a unit whose commit awaits its
        // observation, is not writable: the transaction owner refuses the
        // edit, so the field returns to the owner's value at once rather than
        // holding a local value no draft backs.
        if (!writable) { formApi.reset(displayed); return; }
        const values = formApi.state.values as T;
        reflected.current = values;
        edit(values);
      },
    },
  }) as TypedUnitForm<T>;
  useEffect(() => {
    if (sameValue(reflected.current, unit.displayed)) return;
    // The actor's value moved for a reason that is not this form's editing:
    // a discarded draft, a reviewed revision, or a confirmed commit dropping
    // the authored draft. The actor is the owner, so the form follows it.
    reflected.current = unit.displayed;
    form.reset(unit.displayed);
  }, [form, unit.displayed]);
  return <UnitShell title={title} unit={unit} redacted={options.redacted} removable={removable} removalNotice={removalNotice} presentation={presentation} legendHidden={legendHidden} onCancel={onCancel}
    onSubmit={() => { void form.handleSubmit(); }}
    explicitSave={immediate && !!options.redacted ? () => unit.commit(unit.displayed) : undefined}>
    {children(form)}
  </UnitShell>;
}
