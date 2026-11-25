import { useCallback, useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import type { SourceMutation, SourceScope } from '../../../../protocol/app-server/v80';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { Switch } from '../../presentation/primitives/Switch';
import { SettingsGroup, SettingsRow } from '../../presentation/settings/SettingsPageShell';
import { usePageTransaction } from './forms/bridge';
import { useSettingsActor } from './machines/react';
import type { RevisionSelector } from './projection';

type UnitDraftMap = Record<string, {
  getSnapshot(): { context: { draft: unknown; selector: RevisionSelector } };
  subscribe(listener: () => void): { unsubscribe(): void };
}>;

function hasDraftForUnits(unitsMap: UnitDraftMap, units?: readonly string[]): boolean {
  if (!units?.length) return false;
  return Object.values(unitsMap).some(ref => {
    const snap = ref.getSnapshot().context;
    return snap.draft !== undefined
      && snap.selector.kind === 'config'
      && snap.selector.unit !== undefined
      && units.includes(snap.selector.unit);
  });
}

/** Top-of-module switch on Workspace Settings pages controlling whether this
 * section inherits User (global) settings cleanly (`rustx.toml` overrides
 * omitted) or reveals section fields for workspace-specific customization. */
export function WorkspaceFollowGlobal({
  scope,
  hasOverrides,
  authoredRemovals,
  units,
  children,
}: {
  scope: SourceScope;
  hasOverrides: boolean;
  authoredRemovals: readonly SourceMutation[];
  units?: readonly string[];
  children: ReactNode;
}) {
  const tx = useTranslation();
  const actor = useSettingsActor();
  const page = usePageTransaction();
  const setModuleReset = page?.setModuleReset;
  const unitsRef = useRef(units);
  unitsRef.current = units;

  const subscribe = useCallback((notify: () => void) => {
    if (scope !== 'workspace' || !unitsRef.current?.length) return () => {};
    const childSubs = new Map<string, { unsubscribe(): void }>();
    const syncChildren = () => {
      const current = actor.getSnapshot().context.units as UnitDraftMap;
      for (const [id, sub] of childSubs) {
        if (!(id in current)) { sub.unsubscribe(); childSubs.delete(id); }
      }
      for (const [id, ref] of Object.entries(current)) {
        if (!childSubs.has(id)) childSubs.set(id, ref.subscribe(notify));
      }
    };
    syncChildren();
    const rootSub = actor.subscribe(() => {
      syncChildren();
      notify();
    });
    return () => {
      rootSub.unsubscribe();
      for (const sub of childSubs.values()) sub.unsubscribe();
      childSubs.clear();
    };
  }, [actor, scope]);

  const hasUnitDraft = useSyncExternalStore(
    subscribe,
    () => scope === 'workspace' && hasDraftForUnits(actor.getSnapshot().context.units as UnitDraftMap, unitsRef.current),
    () => false,
  );

  // A staged reset is page-level intent: it survives this row's unmount and
  // remount (e.g. an Extensions filter switch) exactly like a detached draft,
  // so a remount with one pending starts back on the pending-following view.
  const stagedReset = scope === 'workspace' && (page?.moduleReset?.length ?? 0) > 0;
  const [customized, setCustomized] = useState((hasOverrides || hasUnitDraft) && !stagedReset);
  const [confirmingReset, setConfirmingReset] = useState(false);
  const prevContext = useRef({ actor, hasOverrides });

  useEffect(() => {
    if (prevContext.current.actor !== actor || prevContext.current.hasOverrides !== hasOverrides) {
      prevContext.current = { actor, hasOverrides };
      setCustomized(hasOverrides || hasUnitDraft);
      return;
    }
    if (hasUnitDraft) setCustomized(true);
  }, [actor, hasOverrides, hasUnitDraft]);

  // On remount the pending reset's Cancel callback still points at the dead
  // instance's setter — re-arm it with the staged mutations, not a fresh set.
  useEffect(() => {
    const pending = page?.moduleReset;
    if (pending?.length) setModuleReset?.(pending, () => setCustomized(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // A conflicting save leaves the workspace overrides in place: the switch
  // falls back to the honest customized view, and the row surfaces the same
  // adopt/retry recovery the page footer would show.
  const conflict = page?.conflict;
  useEffect(() => {
    if (conflict) setCustomized(true);
  }, [conflict]);

  // A refused (rejected or unknown-outcome) reset is neither "still
  // customized" nor "following": the overrides still exist natively while
  // the removal intent stays staged. The failure belongs to this switch — it
  // alone owns the visible diagnosis and the retry/abandon recovery, since
  // the field editors it would inherit from are already hidden.
  const resetError = page?.moduleResetError;

  if (scope !== 'workspace') return <>{children}</>;

  const following = !customized;
  const description = following
    ? hasOverrides
      ? tx('settings:workspace.follow-global.pending-reset')
      : tx('settings:workspace.follow-global.desc-following')
    : tx('settings:workspace.follow-global.desc-custom');

  return (
    <>
      <SettingsGroup>
        <SettingsRow
          label={tx('settings:workspace.follow-global.title')}
          description={description}
          control={
            <Switch
              label={tx('settings:workspace.follow-global.title')}
              checked={following}
              onChange={nextFollow => {
                if (nextFollow) {
                  if ((hasOverrides && authoredRemovals.length > 0) || hasUnitDraft) { setConfirmingReset(true); return; }
                  page?.discardDrafts();
                  setCustomized(false);
                } else {
                  setConfirmingReset(false);
                  setModuleReset?.(undefined);
                  setCustomized(true);
                }
              }}
            />
          }
        />
        {confirmingReset && (
          <SettingsRow
            label={tx('settings:workspace.follow-global.confirm-reset')}
            control={
              <>
                <Button variant="ghost" size="sm" onClick={() => setConfirmingReset(false)}>{tx('settings:aria.cancel')}</Button>
                <Button variant="danger" size="sm" onClick={() => {
                  setConfirmingReset(false);
                  page?.discardDrafts();
                  setCustomized(false);
                  if (hasOverrides && authoredRemovals.length > 0) {
                    setModuleReset?.(authoredRemovals, () => setCustomized(true));
                    page?.savePage();
                  }
                }}>{tx('settings:workspace.follow-global.confirm-clear')}</Button>
              </>
            }
          />
        )}
        {conflict && (
          <SettingsRow
            label={tx('settings:unsaved.save-conflict')}
            control={
              <>
                <Button variant="ghost" size="sm" onClick={conflict.adopt}>{tx('settings:unsaved.adopt-latest')}</Button>
                <Button variant="ghost" size="sm" onClick={conflict.retry}>{tx('settings:bridge.use-reviewed-revision')}</Button>
              </>
            }
          />
        )}
        {resetError && (
          <SettingsRow
            label={`${tx('settings:workspace.follow-global.title')} ${tx('settings:bridge.was-not-saved')}${resetError.detail ? ` ${resetError.detail}` : ''}`}
            control={
              <>
                <Button variant="ghost" size="sm" onClick={page?.abandonModuleReset}>{tx('settings:workspace.follow-global.keep-custom')}</Button>
                <Button variant="ghost" size="sm" onClick={page?.retryModuleReset}>{tx('settings:aria.retry')}</Button>
              </>
            }
          />
        )}
      </SettingsGroup>
      {customized ? children : null}
    </>
  );
}
