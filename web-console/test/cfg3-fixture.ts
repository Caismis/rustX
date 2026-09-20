import { cfg3Source, cfg3Effective, cfg3Application } from './cfg3-data';
import { vi } from 'vitest';
import type { AttachmentTarget, EffectiveConfiguration, MethodResult, Request1, SourceSettings } from '../../protocol/app-server/v14';
import { AppServerClient, type ClientView } from '../src/client/app-server';
export const cfg3Session = 'ses_00000000-0000-7000-8000-000000000001';
export const cfg3Target: AttachmentTarget = { session_id: cfg3Session, conversation_id: 'conv_00000000-0000-7000-8000-000000000001', runtime_incarnation: '1', attachment_id: 'attachment-1' };
export function cfg3Client(handler?: (operation: Request1, source: SourceSettings, effective: EffectiveConfiguration) => Promise<MethodResult | void>) {
  const source = cfg3Source(), effective = cfg3Effective(), client = new AppServerClient();
  const state: ClientView = { connection: 'connected', generation: 1, sessions: [], uncertain: [], interactionOperations: {}, views: { [cfg3Session]: { id: cfg3Session, attachmentIntent: 'wanted', attachment: 'attached', target: cfg3Target } } };
  vi.spyOn(client, 'getSnapshot').mockReturnValue(state);
  const request = vi.spyOn(client, 'request').mockImplementation(async operation => {
    const overridden = await handler?.(operation, source, effective); if (overridden) return overridden as never;
    if (operation.method === 'configuration/effective') return { type: 'effective_configuration', projection: structuredClone(effective) } as never;
    if (operation.method === 'session/adoptConfiguration') {
      source.application = { ...source.application!, version: '3', candidate: null, units: { instructions: { status: 'applied' } } };
      effective.generation = '8'; effective.adopted_binding = '2'; source.loaded = { generation: '8', changed_sources: [] };
      return { type: 'configuration_application', application: source.application } as never;
    }
    if (operation.method === 'configuration/reconcile') return { type: 'configuration_application', application: source.application ?? cfg3Application() } as never;
    if (operation.method === 'configuration/sourceWrite') { source.application = cfg3Application(); source[operation.params.mutation.scope].revision = 'saved-2'; }
    return { type: 'source_settings', projection: structuredClone(source), session_revision: '1', session_selection: null } as never;
  });
  return { client, request, source, effective, state };
}
