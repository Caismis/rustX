import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useSyncExternalStore } from 'react';
import { useSelector } from '@xstate/react';
import type { AppServerClient } from '../../../client/app-server';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { settingsTargetKey, type SettingsTarget } from '../projection';
import { configurationSystem, type SessionConfigurationActor, type SettingsTargetActor } from './system';
import type { UnitTransactionRef } from './settings-target';
import type { UnitTransactionSnapshot } from './unit-transaction';

/** The Settings authority actor the editors of one Settings instance submit
 * intent to. Provided by `Settings`; an editor never reaches for a client. */
export const SettingsActorContext = createContext<SettingsTargetActor | undefined>(undefined);
export function useSettingsActor(): SettingsTargetActor {
  const actor = useContext(SettingsActorContext);
  if (!actor) throw new Error('A Settings editor must be rendered inside a Settings authority actor.');
  return actor;
}

/** Bind one Settings presentation to the authority actor of its exact target.
 *
 * The actor is addressed by (endpoint, authority revision, target) and outlives
 * this component: closing Settings detaches the presentation; it never cancels
 * a native mutation and never discards an editing transaction. The presentation
 * attachment is the only thing this binding tells the actor: the transport is
 * delivered to it by its `ConfigurationSystem`, so no read, reconnect or
 * convergence ever depends on this component rendering. */
export function useSettingsTarget(client: AppServerClient, target: SettingsTarget, host: ProductHostWorkspaces | undefined) {
  const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
  // The Product Host object identity is a presentation detail that may change
  // on any render; the port and its actor are bound to the authority instead.
  const hostRef = useRef(host);
  hostRef.current = host;
  const targetRef = useRef(target);
  targetRef.current = target;
  const lifetime = `${transport.endpoint ?? ''}|${transport.authorityRevision ?? 0}|${settingsTargetKey(target)}`;
  const actor = useMemo(
    () => configurationSystem(client).settingsTarget(targetRef.current, () => hostRef.current),
    [client, lifetime],
  );
  useEffect(() => {
    actor.send({ type: 'ATTACH' });
    // An authority replacement may already have stopped this lifetime, which
    // leaves no presentation attachment to end.
    return () => { if (actor.getSnapshot().status === 'active') actor.send({ type: 'DETACH' }); };
  }, [actor]);
  return { actor, transport };
}

/** Bind one Session configuration presentation to its Session's actor.
 *
 * A presentation holds the actor and nothing more. Observation — including the
 * read a reconnected generation owes — is driven by the transport the
 * `ConfigurationSystem` delivers, never by this component's effects. */
export function useSessionConfiguration(client: AppServerClient, sessionId: string) {
  const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const lifetime = `${transport.endpoint ?? ''}|${transport.authorityRevision ?? 0}|${sessionId}`;
  const actor = useMemo(() => configurationSystem(client).sessionConfiguration(sessionId), [client, lifetime]);
  useEffect(() => {
    const system = configurationSystem(client);
    system.retainSession(actor);
    return () => system.releaseSession(actor);
  }, [client, actor]);
  return { actor, transport };
}

/** Subscribe to one unit's live editing transaction, which exists only while
 * this browser holds a transaction for that unit. `undefined` is exactly "this
 * browser authored nothing for the unit and fences on native authority". */
export function useUnitTransaction(actor: SettingsTargetActor, identity: string): UnitTransactionSnapshot | undefined {
  const unit: UnitTransactionRef | undefined = useSelector(actor, snapshot => snapshot.context.units[identity]);
  const subscribe = useCallback((notify: () => void) => {
    if (!unit) return () => {};
    const subscription = unit.subscribe(notify);
    return () => subscription.unsubscribe();
  }, [unit]);
  const read = useCallback(() => unit?.getSnapshot(), [unit]);
  return useSyncExternalStore(subscribe, read, read);
}

export type { SessionConfigurationActor, SettingsTargetActor };
