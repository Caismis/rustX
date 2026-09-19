import type { SourceSettings, EffectiveConfiguration } from '../../protocol/app-server/v12';
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
