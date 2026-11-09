import { useRef, useState } from 'react';
import type { ModelProbeOutcome, ModelProbeSource, SourceSettings } from '../../../../../protocol/app-server/v77';
import type { AppServerClient } from '../../../client/app-server';
import { sameValue, transportSelection, useClientSelector } from '../../../client/selectors';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { settingsTargetKey, type SettingsTarget } from '../projection';

type ProbeState = ModelProbeOutcome | 'checking';
type ProbeObservation = { key: string; states: Record<string, ProbeState> };

export function useModelProbe(client: AppServerClient, host: ProductHostWorkspaces | undefined, target: SettingsTarget, source: SourceSettings) {
  const transport = useClientSelector(client, transportSelection, sameValue);
  const revisions: ModelProbeSource = {
    user_revision: source.user.revision,
    workspace_revision: source.workspace?.revision,
  };
  const key = JSON.stringify([transport.endpoint, transport.authorityRevision, transport.generation, settingsTargetKey(target), revisions]);
  const liveKey = useRef(key);
  liveKey.current = key;
  const [observation, setObservation] = useState<ProbeObservation>();
  const available = transport.connection === 'connected' && (target.kind === 'user' || !!host?.configureWorkspace && !!revisions.workspace_revision);
  const states = available && observation?.key === key ? observation.states : {};
  const test = async (model: string) => {
    if (!available || states[model] === 'checking') return;
    const current = () => {
      const live = client.getSnapshot();
      return liveKey.current === key && live.connection === 'connected' && live.endpoint === transport.endpoint
        && live.authorityRevision === transport.authorityRevision && live.generation === transport.generation;
    };
    const publish = (state: ProbeState) => {
      if (current()) setObservation(previous => ({ key, states: { ...(previous?.key === key ? previous.states : {}), [model]: state } }));
    };
    publish('checking');
    try {
      if (target.kind === 'workspace') {
        const response = await host?.configureWorkspace?.(target.id, transport.endpoint ?? '', { kind: 'model_probe', model, source: revisions });
        publish(response?.kind === 'model_probe' ? response.outcome : 'unavailable');
      } else {
        const response = await client.request({ method: 'model/probe', params: { target: { kind: 'user' }, model, source: revisions } }, 'model_probe', undefined, current);
        publish(response.outcome);
      }
    } catch {
      publish('unavailable');
    }
  };
  return { available, states, test };
}
