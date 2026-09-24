import { cfg3Source, cfg3Effective, cfg3Application } from './cfg3-data';
import { vi } from 'vitest';
import type { AttachmentTarget, EffectiveConfiguration, MethodResult, Request1, SourceSettings } from '../../protocol/app-server/v21';
import { AppServerClient, type ClientView } from '../src/client/app-server';
export const cfg3Session = 'ses_00000000-0000-7000-8000-000000000001';
export const cfg3Target: AttachmentTarget = { session_id: cfg3Session, conversation_id: 'conv_00000000-0000-7000-8000-000000000001', runtime_incarnation: '1', attachment_id: 'attachment-1' };
export function cfg3Client(handler?: (operation: Request1, source: SourceSettings, effective: EffectiveConfiguration) => Promise<MethodResult | void>) { const source = cfg3Source(), effective = cfg3Effective(), client = new AppServerClient(); let state: ClientView = { connection: 'connected', generation: 1, sessions: [], uncertain: [], interactionOperations: {}, views: { [cfg3Session]: { id: cfg3Session, attachmentIntent: 'wanted', attachment: 'attached', target: cfg3Target } } }; vi.spyOn(client, 'getSnapshot').mockImplementation(() => state);
    // Like the real client, a published snapshot change notifies every subscriber synchronously.
    const listeners = new Set<() => void>(), subscribe = client.subscribe; vi.spyOn(client, 'subscribe').mockImplementation(listener => { listeners.add(listener); const release = subscribe(listener); return () => { listeners.delete(listener); release(); }; });
    const publish = (patch: Partial<ClientView>) => { state = { ...state, ...patch }; for (const listener of listeners) listener(); }; const request = vi.spyOn(client, 'request').mockImplementation(async (operation) => { const overridden = await handler?.(operation, source, effective); if (overridden)
    return overridden as never; if (operation.method === 'session/configuration')
    return { type: 'session_configuration', application: source.application ?? null } as never; if (operation.method === 'session/effectiveConfiguration')
    return { type: 'effective_configuration', projection: structuredClone(effective) } as never; if (operation.method === 'session/adoptConfiguration') {
    source.application = { ...source.application!, version: '3', candidate: null, units: { instructions: { status: 'applied' } } };
    effective.generation = '8';
    effective.adopted_binding = '2';
    return { type: 'configuration_application', application: source.application } as never;
} if (operation.method === 'configuration/reconcile')
    return { type: 'configuration_application', application: source.application ?? cfg3Application() } as never; if (operation.method === 'configuration/sourceWrite') {
    source.application = cfg3Application();
    source[operation.params.target.kind]!.revision = 'saved-2';
} if ('target' in (operation.params ?? {}) && operation.method.startsWith('configuration/'))
    source.target = (operation as Extract<Request1, {
        method: 'configuration/sourcesRead';
    }>).params.target; return { type: 'source_settings', projection: structuredClone(source) } as never; }); return { client, request, source, effective, get state() { return state; }, publish, host: undefined as import('../src/workspaces/host').ProductHostWorkspaces | undefined }; }
export function cfg3Host(subject: ReturnType<typeof cfg3Client>): import('../src/workspaces/host').ProductHostWorkspaces { return { listWorkspaces: async () => ({ endpoint: '', workspaces: ['A', 'B'].map(id => ({ id, displayName: id, location: id, displayPath: '/workspace/' + id })), picker: { kind: 'unavailable', reason: 'fixture' } }), configureWorkspace: async (id, _endpoint, operation) => { const target = { kind: 'workspace' as const, directory: '/workspace/' + id }; if (operation.kind === 'write') { const acknowledgement = (await subject.client.request({ method: 'configuration/sourceWrite', params: { target, expected_revision: operation.expected_revision, mutation: operation.mutation } }, 'source_settings')).projection; let reread: import('../src/workspaces/host').WorkspaceConfigurationReread; try { reread = { status: 'observed', projection: (await subject.client.request({ method: 'configuration/sourcesRead', params: { target } }, 'source_settings')).projection }; } catch (error) { reread = { status: 'failed', error }; } return { kind: 'write', commit: { acknowledgement, reread } }; } if (operation.kind === 'reconcile')
        await subject.client.request({ method: 'configuration/reconcile', params: { target } }, 'configuration_application'); return { kind: operation.kind, projection: (await subject.client.request({ method: 'configuration/sourcesRead', params: { target } }, 'source_settings')).projection }; }, adoptWorkspace: async () => { }, renameWorkspace: async () => { }, reorderWorkspace: async () => { }, removeWorkspace: async () => { }, resolveWorkspace: async (id) => ({ cwd: '/workspace/' + id }), classifyLocations: async (cwds) => cwds.map(() => ({ authorized: true })), }; }
