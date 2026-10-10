import { useEffect, useRef, useState } from 'react';
import type { McpProbeOutcome, SourceSettings } from '../../../../../protocol/app-server/v40';
import type { AppServerClient } from '../../../client/app-server';
import { useClientSelector, transportSelection, sameValue } from '../../../client/selectors';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { settingsTargetKey, type SettingsTarget } from '../projection';

export type ProbeState = McpProbeOutcome | 'checking' | 'unknown';
/** Results describe finite configuration checks, never a Session connection.
 * Each observation checks sequentially; obsolete queues cannot dispatch or publish. */
export function useMcpProbes(client: AppServerClient, host: ProductHostWorkspaces | undefined, target: SettingsTarget, source: SourceSettings | undefined, active: boolean) {
  const transport = useClientSelector(client, transportSelection, sameValue);
  const [refresh, setRefresh] = useState(0);
  const [observation, setObservation] = useState<{ key: string; states: Record<string, ProbeState>; busy: boolean }>();
  const inputs = useRef({host, target});
  inputs.current = {host, target};
  const document = target.kind === 'user' ? source?.user_mcp : source?.workspace_mcp;
  const selections = source?.[target.kind]?.authored?.agent?.tools?.sources ?? {};
  const ids = Object.keys(document?.authored ?? {}).filter(id => {
    const selection = selections[id];
    return selection === 'all' || (Array.isArray(selection) && selection.length > 0);
  }).sort();
  const revision = document?.revision;
  const key = JSON.stringify([transport.endpoint, transport.authorityRevision, transport.generation, transport.connection, settingsTargetKey(target), revision, ids, active, refresh]);
  const currentKey = useRef(key);
  currentKey.current = key;
  useEffect(() => {
    if (!active || !source || !revision || transport.connection !== 'connected') return;
    let disposed = false;
    const {host: ownerHost, target: ownerTarget} = inputs.current;
    const current = () => {
      const live = client.getSnapshot();
      return !disposed && currentKey.current === key && live.connection === 'connected'
        && live.generation === transport.generation && live.endpoint === transport.endpoint
        && live.authorityRevision === transport.authorityRevision;
    };
    setObservation({key, states: Object.fromEntries(ids.map(id => [id, 'checking'])), busy: ids.length > 0});
    void (async () => {
      for (const id of ids) {
        if (!current()) return;
        let state: ProbeState = 'unknown';
        try {
          let result;
          if (ownerTarget.kind === 'workspace') {
            if (!ownerHost?.configureWorkspace) throw new Error('Workspace Host unavailable');
            const response = await ownerHost.configureWorkspace(ownerTarget.id, transport.endpoint ?? '', {kind:'mcp_probe', id, expected_revision:revision});
            if (response.kind !== 'mcp_probe') throw new Error('Unexpected probe result');
            result = response.result;
          } else {
            result = (await client.request({method:'mcp/probe', params:{target:{kind:'user'},id,expected_revision:revision}}, 'mcp_probe', undefined, current)).result;
          }
          if (result.id === id && result.revision === revision) state = result.outcome;
        } catch { /* Transport/admission failures are not proof of MCP failure. */ }
        if (!current()) return;
        setObservation(previous => previous?.key === key ? {...previous, states:{...previous.states,[id]:state}} : previous);
      }
      if (current()) setObservation(previous => previous?.key === key ? {...previous,busy:false} : previous);
    })();
    return () => { disposed = true; };
    // key contains the exact authority, source revision, selection and presentation.
    // Object identity changes on an otherwise identical settings read do not probe.
  }, [client, key]);
  return {states: observation?.key === key ? observation.states : {}, busy: observation?.key === key && observation.busy, refresh: () => setRefresh(value => value + 1)};
}
