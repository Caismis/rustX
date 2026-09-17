import { vi } from 'vitest';
import type { AttachmentTarget, EffectiveConfiguration, MethodResult, Request1, SourceSettings } from '../../protocol/app-server/v6';
import { AppServerClient, type ClientView } from '../src/client/app-server';
export const cfg3Session = 'ses_00000000-0000-7000-8000-000000000001';
export const cfg3Target: AttachmentTarget = { session_id: cfg3Session, conversation_id: 'conv_00000000-0000-7000-8000-000000000001', runtime_incarnation: '1', attachment_id: 'attachment-1' };
export function cfg3Source(): SourceSettings {
  return { absent_resource_revision: 'missing', resource_revisions: {}, loaded: { generation: '7', pending_reload: false, changed_sources: [] },
    user: { path: '/bound/rustx.toml', revision: 'user-1', authored: { providers: { transport: { base_url: 'https://user.invalid', credential: { type: 'environment', variable: 'USER_KEY' } } }, models: {} } },
    workspace: { path: '/workspace/rustx.toml', revision: 'workspace-1', authored: {} },
    user_resource_root: '/home/user/rustx/.agents', workspace_resource_root: '/workspace/.agents', runtime_root: '/home/user/rustx/runtime',
    user_mcp: { path: '/home/user/rustx/.agents/mcp.toml', revision: 'mcp-1', authored: {} }, workspace_mcp: { path: '/workspace/.agents/mcp.toml', revision: 'mcp-2', authored: {} }, agents: [],
  };
}
export function cfg3Effective(): EffectiveConfiguration {
  const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false };
  return { generation: '7', source_revisions: {}, document: { models: { main: { provider: 'transport', id: 'wire', protocol: 'openai_responses', context_window: '128000', max_output_tokens: 8192, capabilities: { input_modalities: ['text'], output_modalities: ['text'], tool_calls: true, reasoning: false } } } },
    root_agent: { model: { model: 'main' }, tools: { builtin: [], sources: {} }, skills: [], plugins: {}, agents: [], workflows: [] },
    context: { reserveTokens: '4096', keepRecentTokens: '8192' }, model_timeout: {}, tool_deadline: {}, child_capacity: { maxConcurrent: 4 }, approval_mode: 'policy', provenance: {},
    resources: { definitions: [], resource_diagnostics: [], agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [] }, available_tools: [],
    effective_model: { configured: { model: 'server-frozen-model' }, effective: { model: 'server-frozen-model', protocol: 'openai_responses', contextWindow: 128000, modelMaxOutputTokens: 8192, maxOutputTokens: 8192, reasoningEnabled: false, capabilities, declaredCapabilities: capabilities }, summary: { mode: 'session' } },
  };
}
export function cfg3Client(handler?: (operation: Request1, source: SourceSettings, effective: EffectiveConfiguration) => Promise<MethodResult | void>) {
  const source = cfg3Source(), effective = cfg3Effective(), client = new AppServerClient();
  const state: ClientView = { connection: 'connected', generation: 1, sessions: [], uncertain: [], interactionOperations: {}, views: { [cfg3Session]: { id: cfg3Session, attachmentIntent: 'wanted', attachment: 'attached', target: cfg3Target } } };
  vi.spyOn(client, 'getSnapshot').mockReturnValue(state);
  const request = vi.spyOn(client, 'request').mockImplementation(async operation => {
    const overridden = await handler?.(operation, source, effective); if (overridden) return overridden as never;
    if (operation.method === 'configuration/effective') return { type: 'effective_configuration', projection: structuredClone(effective) } as never;
    if (operation.method === 'configuration/reload') { effective.generation = '8'; source.loaded = { generation: '8', pending_reload: false, changed_sources: [] }; return { type: 'configuration_reloaded', resource_revision: '8', capability_revision: '8' } as never; }
    if (operation.method === 'configuration/sourceWrite') { source.loaded = { generation: '7', pending_reload: true, changed_sources: ['/workspace/rustx.toml'] }; source[operation.params.mutation.scope].revision = 'saved-2'; }
    return { type: 'source_settings', projection: structuredClone(source), session_revision: '1', session_selection: null } as never;
  });
  return { client, request, source, effective, state };
}
