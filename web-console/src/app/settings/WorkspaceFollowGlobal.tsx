import { useCallback, useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from 'react';
import type { SourceMutation, SourceScope } from '../../../../protocol/app-server/v80';
import { useTranslation } from '../../locale/react';
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

  const [customized, setCustomized] = useState(hasOverrides || hasUnitDraft);
  const prevContext = useRef({ actor, hasOverrides });

  useEffect(() => {
    if (prevContext.current.actor !== actor || prevContext.current.hasOverrides !== hasOverrides) {
      prevContext.current = { actor, hasOverrides };
      setCustomized(hasOverrides || hasUnitDraft);
      return;
    }
    if (hasUnitDraft) setCustomized(true);
  }, [actor, hasOverrides, hasUnitDraft]);

  useEffect(() => () => setModuleReset?.(undefined), [setModuleReset]);

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
                  page?.discardDrafts();
                  setCustomized(false);
                  if (hasOverrides && authoredRemovals.length > 0) {
                    setModuleReset?.(authoredRemovals, () => setCustomized(true));
                  }
                } else {
                  setModuleReset?.(undefined);
                  setCustomized(true);
                }
              }}
            />
          }
        />
      </SettingsGroup>
      {customized ? children : null}
    </>
  );
}
