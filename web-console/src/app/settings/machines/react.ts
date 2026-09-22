import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useSyncExternalStore } from 'react';
import { useSelector } from '@xstate/react';
import type { AppServerClient, ClientView } from '../../../client/app-server';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { settingsTargetKey, type SettingsTarget } from '../projection';
import { configurationSystem, type SessionConfigurationActor, type SettingsTargetActor } from './system';
import type { UnitTransactionRef } from './settings-target';
import type { UnitTransactionContext } from './unit-transaction';

/** The Settings authority actor the editors of one Settings instance submit
 * intent to. Provided by `Settings`; an editor never reaches for a client. */
export const SettingsActorContext = createContext<SettingsTargetActor | undefined>(undefined);
export function useSettingsActor(): SettingsTargetActor {
  const actor = useContext(SettingsActorContext);
  if (!actor) throw new Error('A Settings editor must be rendered inside a Settings authority actor.');
  return actor;
}

/** A stable key for the native application publications, so the machine is told
 * about a publication exactly when one really changed — never once per render
 * and never once per unrelated client event. */
function publicationKey(transport: ClientView): string {
  return Object.entries(transport.configuration ?? {}).map(([scope, value]) => `${scope}=${value.version}`).join(' ');
}

/** Bind one Settings presentation to the authority actor of its exact target.
 *
 * The actor is addressed by (endpoint, authority revision, target) and outlives
 * this component: closing Settings detaches the presentation; it never cancels
 * a native mutation and never discards an editing transaction. */
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
    return () => actor.send({ type: 'DETACH' });
  }, [actor]);
  const publications = publicationKey(transport);
  const connection = transport.connection, generation = transport.generation, configuration = transport.configuration;
  useEffect(() => {
    actor.send({ type: 'TRANSPORT', connection, generation, publications: configuration });
  }, [actor, connection, generation, publications, configuration]);
  return { actor, transport };
}

/** Bind one Session configuration presentation to its Session's actor. */
export function useSessionConfiguration(client: AppServerClient, sessionId: string) {
  const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const lifetime = `${transport.endpoint ?? ''}|${transport.authorityRevision ?? 0}|${sessionId}`;
  const actor = useMemo(() => configurationSystem(client).sessionConfiguration(sessionId), [client, lifetime]);
  useEffect(() => {
    const system = configurationSystem(client);
    system.retainSession(actor);
    return () => system.releaseSession(actor);
  }, [client, actor]);
  useEffect(() => {
    actor.send({ type: 'TRANSPORT', connection: transport.connection, generation: transport.generation });
  }, [actor, transport.connection, transport.generation]);
  return { actor, transport };
}

/** Subscribe to one unit's transaction actor, which exists only while this
 * browser holds a transaction for that unit. */
function useUnitSnapshot(actor: SettingsTargetActor, identity: string) {
  const unit: UnitTransactionRef | undefined = useSelector(actor, snapshot => snapshot.context.units[identity]);
  const subscribe = useCallback((notify: () => void) => {
    if (!unit) return () => {};
    const subscription = unit.subscribe(notify);
    return () => subscription.unsubscribe();
  }, [unit]);
  const read = useCallback(() => unit?.getSnapshot(), [unit]);
  return useSyncExternalStore(subscribe, read, read);
}

/** One unit's live editing transaction. `undefined` is exactly "this browser
 * authored nothing for the unit and fences on native authority". */
export function useUnitTransaction(actor: SettingsTargetActor, identity: string): UnitTransactionContext | undefined {
  return useUnitSnapshot(actor, identity)?.context;
}

/** Whether one unit's last submitted mutation is natively confirmed. */
export function useUnitCommitted(actor: SettingsTargetActor, identity: string): boolean {
  const snapshot = useUnitSnapshot(actor, identity);
  return !!snapshot && (snapshot.matches({ mutation: 'acknowledged' }) || snapshot.matches({ mutation: 'settled' }));
}

export type { SessionConfigurationActor, SettingsTargetActor };
