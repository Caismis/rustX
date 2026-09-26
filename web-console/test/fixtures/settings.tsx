// Deterministic generated-protocol projection, never imported by production.
import { createRoot } from 'react-dom/client';
import { App } from '../../src/app/App';
import { Server, endpoint } from '../fixture';
import { cfg3Application, cfg3Effective, cfg3Source } from '../cfg3-data';
import { RpcFailure } from '../../src/client/app-server';
import type { ConfigurationApplication, SourceMutation } from '../../../protocol/app-server/v23';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/scrollbar.css';
import '../../src/presentation/theme/corner-shape.css';
import '../../src/presentation/theme/shiki.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
const server = new Server(), source = cfg3Source(), effective = cfg3Effective();
// One deterministic variant per page load, named in the query string, so every
// browser reference is a real rendered state of the same fixture:
//   scenario=conflict    a User save meets an external edit (source_conflict)
//   scenario=loading     the User source read never answers
//   scenario=read-error  the User source read fails natively
//   write=held           every source write commits only once the page calls
//                        `rustxReleaseWrites()`, so the browser is observed
//                        while the write is in flight
//   session=preparing|ready|blocked|failed  the focused Session's application
const variant = new URLSearchParams(location.search);
// Long identities, endpoints, paths and native diagnostics are part of the
// reference data: narrow layouts are proven against them, never only against
// short names. None of them is a credential; credentials stay redacted.
const longProvider = 'enterprise-inference-gateway-eu-central-primary-with-an-exceptionally-long-provider-identity';
const longModel = 'enterprise-reasoning-model-2026-09-long-context-preview-with-an-exceptionally-long-identity';
source.user.authored!.providers![longProvider] = {
  base_url: 'https://inference-gateway.eu-central-1.enterprise.example.invalid/v1/organizations/rustx-platform-team/deployments/primary',
  credential: { type: 'environment', variable: 'ENTERPRISE_INFERENCE_GATEWAY_EU_CENTRAL_PRIMARY_API_KEY' },
};
source.user.authored!.models![longModel] = { provider: longProvider, id: longModel, protocol: 'openai_chat_completions', context_window: '1048576', max_output_tokens: 65536,
  capabilities: { input_modalities: ['text', 'image'], output_modalities: ['text'], tool_calls: true, reasoning: true } };
source.workspace!.authored = { agent_id: 'rustx', agent: { description: 'A native coding assistant', instructions: 'Inspect the source, implement the change, and verify native boundaries.', tools: { builtin: ['read', 'glob'], sources: { 'python:analysis': 'all' } }, plugins: { goal: { enabled: true }, todo: { enabled: true } } }, models: effective.document.models, providers: source.user.authored!.providers };
effective.document.providers = source.user.authored!.providers;
effective.provenance = { 'models.main': { kind: 'workspace', base: '/workspace', document: '/workspace/rustx.toml' }, 'providers.transport': { kind: 'user', base: '/bound', document: '/bound/rustx.toml' } };
effective.resources.definitions = [
 { family: 'skill', name: 'review', valid: true, location: { scope: 'workspace', path: '/workspace/.agents/skills/review/SKILL.md', shadowed: '/home/user/rustx/.agents/skills/review/SKILL.md' } },
 { family: 'skill', name: 'incomplete', valid: false, location: { scope: 'workspace', path: '/workspace/.agents/skills/incomplete/SKILL.md' } },
 { family: 'managed_python', name: 'analysis', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/python/analysis' } },
];
effective.resources.definitions.push({ family: 'skill', name: 'repository-wide-architecture-review-with-a-long-package-identity', valid: true, location: { scope: 'user', path: '/home/user/rustx/.agents/skills/repository-wide-architecture-review-with-a-long-package-identity/SKILL.md' } });
effective.resources.resource_diagnostics = [{ subject: { kind: 'resource', family: 'skill', name: 'incomplete' }, file: '/workspace/.agents/skills/incomplete/SKILL.md', field: 'description', reason: 'Missing package description: the SKILL.md front matter must declare a non-empty description before this package can be advertised in the prompt (/workspace/.agents/skills/incomplete/SKILL.md:1:1)' }];
effective.resources.sources = { 'python:analysis': { status: 'unprepared' } };
source.prospective_resources = effective.resources;
source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/workspace/.agents/agents/reviewer.toml', revision: 'agent-1', authored: { description: 'Independent code review', instructions: 'Inspect changed boundaries and report findings.', tools: { builtin: ['read', 'grep'] }, skills: ['review'] } } }];
source.resolved = { ...source.user.authored, ...source.workspace!.authored };
server.handlers.set('configuration/sourcesRead', () => {
  if (variant.get('scenario') === 'read-error') throw new RpcFailure({ code: -32000, message: '/bound/rustx.toml is not readable by the App Server process (permission denied while opening the canonical configuration directory /bound)', data: { kind: 'operation_failed' } });
  return { type: 'source_settings', projection: { ...source, target: { kind: 'user' } } };
});
if (variant.get('scenario') === 'loading') server.held.add('configuration/sourcesRead');
// A User save meets an external edit of the same document: native refuses it
// on the exact revision, and the draft and its base survive for review.
if (variant.get('scenario') === 'conflict') server.handlers.set('configuration/sourceWrite', request => {
  source.user.revision = 'user-external-edit';
  throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'user', expected: 'expected_revision' in request.params ? request.params.expected_revision : '', actual: source.user.revision } });
});
const session = variant.get('session');
// After an explicit adoption native reports the candidate adopted; the
// browser learns that only from the authoritative reread the adoption owes.
let adopted = false;
if (session) server.handlers.set('session/configuration', () => ({ type: 'session_configuration', application: adopted ? { ...sessionApplication(session), candidate: null } : sessionApplication(session) }));
if (session) server.handlers.set('session/adoptConfiguration', () => { adopted = true; return { type: 'configuration_application', application: { ...sessionApplication(session), candidate: null } }; });
/** The focused Session's native application in one banner state. */
function sessionApplication(state: string): ConfigurationApplication {
  const application = cfg3Application();
  if (state === 'preparing') return { ...application, candidate: null, units: { capabilities: { status: 'preparing' }, provider: { status: 'preparing' } } };
  if (state === 'blocked') return { ...application, eligibility: { status: 'busy' } };
  // The failed unit is authored by User and by Workspace B, while the focused
  // Session lives in /workspace/A: owner navigation must open exactly B.
  if (state === 'failed') return { ...application, candidate: null, sources: [{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/B' }], units: { capabilities: { status: 'failed', diagnostic: 'MCP server repository-index failed to start: /usr/local/bin/repository-index-mcp exited with status 127 (command not found) before completing the initialize handshake' }, execution_policy: { status: 'applied' } } };
  return application;
}
// A draft conversation needs the native Session model catalog and prospective
// permission; ordinary Settings references deliberately do not publish these.
if (variant.get('conversation') === 'ready') {
  source.prospective_approval_mode = 'policy';
  const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], toolCalls: true, reasoning: false };
  source.session_models = { kind: 'available', default_model: { model: 'fixture/model' }, catalog: { models: [{
    model: 'fixture/model', protocol: 'openai_responses', contextWindow: 128000, maxOutputTokens: 8192,
    declaredCapabilities: capabilities, effectiveCapabilities: capabilities, reasoningProfiles: [],
    credentialSource: { type: 'environment', variable: 'KEY' },
  }] } };
}
server.workspaceHost.configureWorkspace = async (_id, _endpoint, operation) => {
  const projection = { ...source, target: { kind: 'workspace' as const, directory: '/workspace' } };
  if (operation.kind === 'write') return { kind: 'write', commit: { acknowledgement: projection, reread: { status: 'observed', projection } } };
  return { kind: operation.kind, projection };
};
// A held write stays in flight — the User request unanswered, the Workspace
// host call unresolved — until the test releases it. Only then does native
// commit it: the removal is applied, the revision advances and the projection
// the write acknowledges is the one every later read observes.
if (variant.get('write') === 'held') {
  const commit = (scope: 'user' | 'workspace', mutation: SourceMutation) => {
    const view = source[scope]!, authored = view.authored!;
    if (mutation.kind !== 'config' || !('authored' in mutation.mutation) || mutation.mutation.authored !== null) throw new Error('The held-write fixture commits removals only');
    if (mutation.mutation.unit === 'provider') delete authored.providers![mutation.mutation.id];
    else if (mutation.mutation.unit === 'agent_identity') delete authored.agent_id;
    else throw new Error(`The held-write fixture has no removal of ${mutation.mutation.unit}`);
    view.revision = `${scope}-committed`;
    source.resolved = { ...source.user.authored, ...source.workspace!.authored };
  };
  server.handlers.set('configuration/sourceWrite', request => {
    if (request.method !== 'configuration/sourceWrite') throw new Error('Unreachable');
    commit('user', request.params.mutation);
    return { type: 'source_settings', projection: { ...structuredClone(source), target: { kind: 'user' } } };
  });
  server.held.add('configuration/sourceWrite');
  const releases: (() => void)[] = [];
  let workspaceWrites = 0;
  // Workspace writes use the Product Host boundary, not the User RPC socket.
  (window as unknown as { rustxHeldWorkspaceWrites: () => number }).rustxHeldWorkspaceWrites = () => workspaceWrites;
  const configure = server.workspaceHost.configureWorkspace!;
  server.workspaceHost.configureWorkspace = async (id, at, operation) => {
    if (operation.kind === 'write') {
      workspaceWrites++;
      await new Promise<void>(resolve => releases.push(resolve));
      commit('workspace', operation.mutation);
    }
    return configure(id, at, operation);
  };
  const answered = new WeakSet<object>();
  (window as unknown as { rustxReleaseWrites: () => void }).rustxReleaseWrites = () => {
    for (const { request, socket } of server.requests) {
      if (request.method !== 'configuration/sourceWrite' || answered.has(request)) continue;
      answered.add(request); server.reply(request, socket);
    }
    for (const release of releases.splice(0)) release();
  };
}
server.handlers.set('session/effectiveConfiguration', () => ({ type: 'effective_configuration', projection: effective }));
// The native requests this page issued, for browser assertions of exact
// identity, ordering and request counts. Test fixture only.
(window as unknown as { rustxNativeRequests: () => unknown[] }).rustxNativeRequests = () => server.requests.map(item => item.request);
await server.attached('A');
localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
createRoot(document.getElementById('root')!).render(<App client={server.client} workspaceHost={server.workspaceHost} />);
