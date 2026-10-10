import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { McpProbeOutcome, SourceSettings } from '../../../../../protocol/app-server/v44';
import type { AppServerClient } from '../../../client/app-server';
import { useClientSelector, transportSelection, sameValue } from '../../../client/selectors';
import type { WorkspaceAssociations } from '../../../workspaces/associations';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { settingsTargetKey, type SettingsTarget } from '../projection';

const noSubscribe = () => () => {};
const noCatalog = () => undefined;

export type ProbeState = McpProbeOutcome | 'checking' | 'unknown';
/** Only an explicit row action may execute a finite native diagnostic.
 * Observations and pending replies belong to one exact source and authority. */
export function useMcpProbes(client: AppServerClient, host: ProductHostWorkspaces | undefined, target: SettingsTarget, source: SourceSettings | undefined, active: boolean, associations?: WorkspaceAssociations) {
  const catalog = useSyncExternalStore(associations?.subscribe ?? noSubscribe, associations?.getSnapshot ?? noCatalog)?.catalog;
  const registrationKey = () => {
    if (target.kind !== 'workspace' || !associations) return undefined;
    const current = associations.getSnapshot().catalog;
    const row = current?.workspaces.find(row => row.id === target.id);
    return row && JSON.stringify([current?.authorityId, current?.endpoint, row.id, row.location]);
  };
  const registration = registrationKey();
  const registered = target.kind !== 'workspace' || !associations || !!catalog?.workspaces.some(row => row.id === target.id);
  const transport = useClientSelector(client, transportSelection, sameValue);
  const document = target.kind === 'user' ? source?.user_mcp : source?.workspace_mcp;
  const revision = document?.revision;
  const key = JSON.stringify([transport.endpoint, transport.authorityRevision, transport.generation, transport.connection, settingsTargetKey(target), revision, active, registration]);
  const currentKey = useRef(key);
  currentKey.current = key;
  const owner = useRef<{ key: string; host: ProductHostWorkspaces | undefined; states: Record<string, ProbeState> } | undefined>(undefined);
  const [observation, setObservation] = useState<typeof owner.current>();
  useEffect(() => () => { owner.current = undefined; }, [key, host]);
  const testConnection = async (id: string) => {
    if (!active || !registered || !revision || transport.connection !== 'connected' || !document?.authored?.[id]) return;
    if (owner.current?.key === key && owner.current.states[id] === 'checking') return;
    const operation = owner.current?.key === key && owner.current.host === host ? owner.current : {key, host, states:{}};
    owner.current = operation;
    const publish = (state: ProbeState) => {
      operation.states = {...operation.states, [id]:state};
      setObservation({...operation});
    };
    const current = () => {
      const live = client.getSnapshot();
      return owner.current === operation && currentKey.current === key && live.connection === 'connected'
        && live.generation === transport.generation && live.endpoint === transport.endpoint
        && live.authorityRevision === transport.authorityRevision && registrationKey() === registration;
    };
    if (!current()) return;
    publish('checking');
    let state: ProbeState = 'unknown';
    try {
      let result;
      if (target.kind === 'workspace') {
        if (!host?.configureWorkspace) throw new Error('Workspace Host unavailable');
        const response = await host.configureWorkspace(target.id, transport.endpoint ?? '', {kind:'mcp_probe', id, expected_revision:revision});
        if (response.kind !== 'mcp_probe') throw new Error('Unexpected probe result');
        result = response.result;
      } else {
        result = (await client.request({method:'mcp/probe', params:{target:{kind:'user'},id,expected_revision:revision}}, 'mcp_probe', undefined, current)).result;
      }
      if (result.id === id && result.revision === revision) state = result.outcome;
    } catch { /* Admission failures provide no connectivity evidence. */ }
    if (current()) publish(state);
  };
  const states = owner.current?.key === key && owner.current.host === host && observation?.key === key ? observation.states : {};
  return {states, testConnection, available: active && registered && !!revision && transport.connection === 'connected'};
}
