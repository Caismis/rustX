import type { SourceSettings, EffectiveConfiguration, ConfigurationApplication, SourceTarget } from '../../protocol/app-server/v18';
export function cfg3Source(): SourceSettings {
  return { absent_resource_revision: 'missing', resource_revisions: {}, target: { kind: 'user' }, provenance: {}, process_policy_impacts: {},
    user: { path: '/bound/rustx.toml', revision: 'user-1', authored: { providers: { transport: { base_url: 'https://user.invalid', credential: { type: 'environment', variable: 'USER_KEY' } } }, models: {} } },
    workspace: { path: '/workspace/rustx.toml', revision: 'workspace-1', authored: {} },
    user_resource_root: '/home/user/rustx/.agents', workspace_resource_root: '/workspace/.agents', runtime_root: '/home/user/rustx/runtime',
    user_mcp: { path: '/home/user/rustx/.agents/mcp.toml', revision: 'mcp-1', authored: {} }, workspace_mcp: { path: '/workspace/.agents/mcp.toml', revision: 'mcp-2', authored: {} }, agents: [],
  };
}
export function cfg3Effective(): EffectiveConfiguration {
  const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false };
  return { adopted_binding: '1', generation: '7', source_revisions: {}, document: { models: { main: { provider: 'transport', id: 'wire', protocol: 'openai_responses', context_window: '128000', max_output_tokens: 8192, capabilities: { input_modalities: ['text'], output_modalities: ['text'], tool_calls: true, reasoning: false } } } },
    root_agent: { model: { model: 'main' }, tools: { builtin: [], sources: {} }, skills: [], plugins: {}, agents: [], workflows: [] },
    context: { reserveTokens: '4096', keepRecentTokens: '8192' }, model_timeout: {}, tool_deadline: {}, child_capacity: { maxConcurrent: 4 }, approval_mode: 'policy', provenance: {},
    resources: { definitions: [], resource_diagnostics: [], agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [] }, available_tools: [],
    effective_model: { configured: { model: 'server-frozen-model' }, effective: { model: 'server-frozen-model', protocol: 'openai_responses', contextWindow: 128000, modelMaxOutputTokens: 8192, maxOutputTokens: 8192, reasoningEnabled: false, capabilities, declaredCapabilities: capabilities }, summary: { mode: 'session' } },
  };
}

/** A real Session application: `scope` is the Session identity the App Server
 * reads it under, and `sources` names the authored owners separately. The two
 * are never the same fact. */
export function cfg3Application(): ConfigurationApplication {
  const identity = { input_revision: 'input-2', attempt: '2' };
  return { eligibility: { status: 'eligible' }, scope: 'ses_00000000-0000-7000-8000-000000000001', version: '2', desired: identity,
    sources: [{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/A' }],
    units: { execution_policy: { status: 'applied' }, instructions: { status: 'ready', impact: 'prefix_changed' } },
    candidate: { identity, expected_binding: '1', impact: 'prefix_changed' } };
}
/** A source application, exactly as native publishes one for a source scope. */
export function cfg3SourceApplication(target: SourceTarget = { kind: 'user' }): ConfigurationApplication {
  return { eligibility: { status: 'unavailable' }, scope: target.kind === 'user' ? 'source:user' : `source:workspace:${target.directory}`,
    version: '2', desired: { input_revision: 'input-2', attempt: '2' },
    sources: target.kind === 'user' ? [{ kind: 'user' }] : [{ kind: 'user' }, target],
    units: { process_bindings: { status: 'applied' } }, candidate: null };
}
