import { useLayoutEffect, useRef, useState } from 'react';
import type { ModelProbeOutcome, ModelProbeSource, SourceSettings } from '../../../../../protocol/app-server/v78';
import type { AppServerClient } from '../../../client/app-server';
import { RpcFailure } from '../../../client/errors';
import { sameValue, transportSelection, useClientSelector } from '../../../client/selectors';
import { useTranslation } from '../../../locale/react';
import { useOverlayActive } from '../../../presentation/primitives/DialogSurface';
import { WorkspaceHostError, type ProductHostWorkspaces } from '../../../workspaces/host';
import { settingsTargetKey, type SettingsTarget } from '../projection';
import { useSettingsNotification } from '../SettingsFeedback';

type ProbeState = ModelProbeOutcome | 'checking' | 'unsupported';
type ProbeObservation = { key: string; states: Record<string, ProbeState> };

export function useModelProbe(client: AppServerClient, host: ProductHostWorkspaces | undefined, target: SettingsTarget, source: SourceSettings) {
  const tx = useTranslation();
  const notify = useSettingsNotification();
  const active = useOverlayActive();
  const transport = useClientSelector(client, transportSelection, sameValue);
  const revisions: ModelProbeSource = {
    user_revision: source.user.revision,
    workspace_revision: target.kind === 'workspace' ? source.workspace?.revision : undefined,
  };
  const key = JSON.stringify([transport.endpoint, transport.authorityRevision, transport.generation, settingsTargetKey(target), revisions]);
  const view = useRef<{ key: string; active: boolean; notify: typeof notify }>(undefined);
  useLayoutEffect(() => {
    view.current = { key, active, notify };
    return () => {
      view.current = undefined;
      notify();
    };
  }, [key, active, notify]);
  const [observation, setObservation] = useState<ProbeObservation>();
  const directory = source.target.kind === 'workspace' ? source.target.directory : undefined;
  const available = transport.connection === 'connected' && (target.kind === 'user' || !!host?.configureWorkspace && !!revisions.workspace_revision && !!directory);
  const states = available && observation?.key === key ? observation.states : {};
  const test = async (model: string) => {
    if (!available || states[model] === 'checking') return;
    const current = () => {
      const live = client.getSnapshot();
      return view.current?.key === key && live.connection === 'connected' && live.endpoint === transport.endpoint
        && live.authorityRevision === transport.authorityRevision && live.generation === transport.generation;
    };
    const publish = (state: ProbeState) => {
      if (!current()) return;
      setObservation(previous => ({ key, states: { ...(previous?.key === key ? previous.states : {}), [model]: state } }));
      if (!view.current?.active) return;
      if (state === 'checking') view.current.notify();
      else view.current.notify(tx(`settings:models.probe-${state}`), state === 'connected' ? 'success' : undefined);
    };
    publish('checking');
    try {
      if (target.kind === 'workspace') {
        const response = await host?.configureWorkspace?.(target.id, transport.endpoint ?? '', { kind: 'model_probe', model, source: revisions, expected_directory: directory });
        publish(response?.kind === 'model_probe' ? response.outcome : 'unavailable');
      } else {
        const response = await client.request({ method: 'model/probe', params: { target: { kind: 'user' }, model, source: revisions } }, 'model_probe', undefined, current);
        publish(response.outcome);
      }
    } catch (error) {
      const unsupported = error instanceof RpcFailure && error.error.code === -32601
        || error instanceof WorkspaceHostError && error.kind === 'model_probe_unsupported';
      publish(unsupported ? 'unsupported' : 'unavailable');
    }
  };
  return { available, states, test };
}
