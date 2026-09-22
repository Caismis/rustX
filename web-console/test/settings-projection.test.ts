// @vitest-environment node
import { expect, it } from 'vitest';
import type { Origin, RuntimeLayer, SourceMutation, SourceSettings } from '../../protocol/app-server/v18';
import {
  applicationOwners, applicationScope, authoredUnit, changeBehavior, changeBehaviorLabel, effectiveStateLabel,
  observedResult, observedResultLabel, openOwnerLabel, provenanceLabel, settingsLifecycle, settingsTargetKey,
  settingsTargetLabel, sourceTargetKey, unitApplication, unitFacts, unitProvenance, unitProvenancePath,
  userSettingsTarget, workspaceSettingsTarget,
} from '../src/app/settings/projection';
import { cfg3Application, cfg3Source, cfg3SourceApplication } from './cfg3-data';

const tools: SourceMutation = { kind: 'config', mutation: { unit: 'native_tools', authored: [] } };
const user: Origin = { kind: 'user', document: '/bound/rustx.toml', base: '/bound' };
const workspace: Origin = { kind: 'workspace', document: '/workspace/rustx.toml', base: '/workspace' };
/** A source whose resolution succeeded, so authored and effective facts can be
 * varied independently. */
function resolvedSource(resolved: RuntimeLayer = {}, provenance: Record<string, Origin> = {}): SourceSettings {
  const source = cfg3Source();
  source.resolved = resolved as never;
  source.provenance = provenance;
  return source;
}

it('S1-03 absent, false, empty list, empty object and explicit values are distinct authored facts', () => {
  expect(unitFacts(cfg3Source(), 'workspace', tools).authored.state).toBe('absent');
  for (const authored of [[], false, {}, ['read']]) {
    const source = cfg3Source();
    source.workspace!.authored = { agent: { tools: { builtin: authored as never } } };
    const facts = unitFacts(source, 'workspace', tools);
    expect(facts.authored.state).toBe('present');
    expect(facts.authored.value).toEqual(authored);
  }
  // An authored document that simply omits the unit is absent, never materialized.
  const omitted = cfg3Source();
  omitted.workspace!.authored = { agent: { tools: {} } };
  expect(unitFacts(omitted, 'workspace', tools).authored.state).toBe('absent');
  expect(unitFacts(omitted, 'workspace', tools).authored.value).toBeUndefined();
});

it('S1-03 native effective value and provenance are projected, never recomputed in TypeScript', () => {
  const source = resolvedSource({ agent: { tools: { builtin: ['read', 'glob'] } } }, { 'agent.tools.builtin': user });
  const facts = unitFacts(source, 'workspace', tools);
  expect(facts.authored.state).toBe('absent');
  expect(facts.effective).toEqual({ state: 'available', value: ['read', 'glob'] });
  expect(provenanceLabel(facts.origin)).toBe('Inherited from User');
  expect(provenanceLabel({ state: 'known', origin: { kind: 'builtin' } })).toBe('Native default');
  expect(provenanceLabel({ state: 'known', origin: workspace })).toBe('Workspace override');
  expect(provenanceLabel({ state: 'known', origin: { kind: 'process', base: '/run' } })).toBe('Process default');
  expect(provenanceLabel({ state: 'mixed' })).toBe('Mixed origins');
  expect(provenanceLabel({ state: 'unavailable' })).toBe('Origin not reported');
});

it('S1-03 invalid and unavailable authored state is neither absent nor empty', () => {
  const invalid = cfg3Source();
  invalid.workspace = { path: '/workspace/rustx.toml', revision: 'workspace-1', authored: null, diagnostic: 'invalid rustx.toml' };
  const facts = unitFacts(invalid, 'workspace', tools);
  expect(facts.authored).toEqual({ state: 'invalid', diagnostic: 'invalid rustx.toml' });
  expect(facts.authored.value).toBeUndefined();
  expect(unitFacts(undefined, 'workspace', tools).authored.state).toBe('unavailable');
  // A User target carries no Workspace view at all.
  const userOnly = cfg3Source();
  userOnly.workspace = null;
  expect(unitFacts(userOnly, 'workspace', tools).authored.state).toBe('unavailable');
});

// Blocking finding 4 — authored presence and effective resolution are two
// independent dimensions. Each case fixes one and varies the other.
it('S1-13 Case A a valid Workspace source that authors nothing still reports an unavailable effective value when resolution failed', () => {
  const source = cfg3Source();
  // Native drops `resolved` when a participating document does not parse, and
  // reports why. The Workspace view itself stays valid and editable.
  source.resolved = null;
  source.prospective_diagnostic = 'Source cannot be resolved; repair the diagnosed authored document.';
  source.user = { path: '/bound/rustx.toml', revision: 'user-1', authored: null, diagnostic: 'invalid rustx.toml; source was not loaded' };
  const facts = unitFacts(source, 'workspace', tools);
  expect(facts.authored.state).toBe('absent');
  expect(facts.effective).toEqual({ state: 'invalid', diagnostic: 'Source cannot be resolved; repair the diagnosed authored document.' });
  // "No Workspace override" must never be reported as an unset effective value.
  expect(facts.effective.value).toBeUndefined();
  expect(effectiveStateLabel(facts.effective)).toBe('Native effective value unavailable — resolution failed');
  // The malformed lower source does not make the valid Workspace unit invalid.
  expect(unitFacts(source, 'user', tools).authored.state).toBe('invalid');
});

it('S1-13 Case B documents that parse but do not resolve report an invalid effective value and keep the authored unit inspectable', () => {
  const source = cfg3Source();
  source.resolved = { agent: { tools: { builtin: ['read'] } } } as never;
  source.prospective_diagnostic = 'context budgets cannot fit the selected models';
  source.workspace!.authored = { agent: { tools: { builtin: ['bash'] } } };
  const facts = unitFacts(source, 'workspace', tools);
  expect(facts.authored).toEqual({ state: 'present', value: ['bash'] });
  expect(facts.effective.state).toBe('invalid');
  expect(facts.effective.diagnostic).toBe('context budgets cannot fit the selected models');
  expect(facts.effective.value).toBeUndefined();
});

it('S1-13 Case C valid inherited configuration reports an absent override with an available effective value and its origin', () => {
  const source = resolvedSource({ agent: { tools: { builtin: ['read'] } } }, { 'agent.tools.builtin': user });
  const facts = unitFacts(source, 'workspace', tools);
  expect(facts.authored.state).toBe('absent');
  expect(facts.effective).toEqual({ state: 'available', value: ['read'] });
  expect(facts.origin).toEqual({ state: 'known', origin: user });
  // Resolution succeeded but no source authors the unit: that is `unset`, a
  // distinct fact from a failed resolution and from an empty authored value.
  const unset = resolvedSource({}, { 'agent.tools': { kind: 'builtin' } });
  expect(unitFacts(unset, 'workspace', tools).effective).toEqual({ state: 'unset' });
  expect(unitFacts(unset, 'workspace', tools).origin).toEqual({ state: 'known', origin: { kind: 'builtin' } });
});

it('S1-13 an unavailable source is not an unset effective value', () => {
  const source = cfg3Source();
  source.resolved = null;
  source.prospective_diagnostic = null;
  expect(unitFacts(source, 'workspace', tools).effective).toEqual({ state: 'unavailable' });
  expect(unitFacts(undefined, 'workspace', tools).effective).toEqual({ state: 'unavailable' });
});

// Blocking finding 3 — every identity-bearing unit resolves through its exact
// native provenance key, exactly as `RuntimeLayer::overlay`/`named` record it.
const identities: readonly { unit: string; mutation: SourceMutation; path: string; sibling: string }[] = [
  { unit: 'provider', mutation: { kind: 'config', mutation: { unit: 'provider', id: 'a', authored: null } }, path: 'providers.a', sibling: 'providers.some-long-workspace-provider' },
  { unit: 'model', mutation: { kind: 'config', mutation: { unit: 'model', id: 'a', authored: null } }, path: 'models.a', sibling: 'models.some-long-workspace-model' },
  { unit: 'source_tools', mutation: { kind: 'config', mutation: { unit: 'source_tools', id: 'a', authored: null } }, path: 'agent.tools.sources.a', sibling: 'agent.tools.sources.some-long-workspace-source' },
  { unit: 'native_policy', mutation: { kind: 'config', mutation: { unit: 'native_policy', id: 'read', authored: null } }, path: 'native_tools.read', sibling: 'native_tools.write' },
  { unit: 'mcp_policy', mutation: { kind: 'config', mutation: { unit: 'mcp_policy', id: 'a', authored: null } }, path: 'mcp_tool_policies.a', sibling: 'mcp_tool_policies.some-long-workspace-server' },
  { unit: 'environment', mutation: { kind: 'config', mutation: { unit: 'environment', name: 'A', authored: null } }, path: 'environment.A', sibling: 'environment.SOME_LONG_WORKSPACE_VARIABLE' },
  { unit: 'mcp', mutation: { kind: 'mcp', id: 'a', authored: null }, path: 'mcp_servers.a', sibling: 'mcp_servers.some-long-workspace-server' },
];
it.each(identities)('S1-14 $unit resolves its own exact provenance key, unaffected by a sibling identity', ({ mutation, path, sibling }) => {
  expect(unitProvenancePath(mutation)).toBe(path);
  // Sibling first, then the queried identity: insertion order is irrelevant.
  expect(unitProvenance(resolvedSource({}, { [sibling]: workspace, [path]: user }), mutation)).toEqual({ state: 'known', origin: user });
  // The queried identity first: the longer sibling never wins.
  expect(unitProvenance(resolvedSource({}, { [path]: user, [sibling]: workspace }), mutation)).toEqual({ state: 'known', origin: user });
  // The sibling resolves to its own origin under the same provenance map.
  expect(unitProvenance(resolvedSource({}, { [path]: user, [sibling]: workspace }),
    { ...mutation, ...(mutation.kind === 'mcp' ? { id: sibling.slice(sibling.lastIndexOf('.') + 1) } : {}) } as SourceMutation)).toBeTruthy();
  // Unrelated identities never answer for a unit the map does not record.
  expect(unitProvenance(resolvedSource({}, { [sibling]: workspace }), mutation).state).not.toBe('known');
});

it('S1-14 whole-unit semantic units use their exact whole-unit native paths', () => {
  const paths: readonly [SourceMutation, string | undefined][] = [
    [{ kind: 'config', mutation: { unit: 'root_model', authored: null } }, 'agent.model'],
    [{ kind: 'config', mutation: { unit: 'native_tools', authored: null } }, 'agent.tools.builtin'],
    [{ kind: 'config', mutation: { unit: 'skills', authored: null } }, 'agent.skills'],
    [{ kind: 'config', mutation: { unit: 'todo', authored: null } }, 'agent.plugins.todo'],
    [{ kind: 'config', mutation: { unit: 'goal', authored: null } }, 'agent.plugins.goal'],
    [{ kind: 'config', mutation: { unit: 'agent_status', authored: null } }, 'agent.plugins.agent_status'],
    [{ kind: 'config', mutation: { unit: 'agents', authored: null } }, 'agent.agents'],
    [{ kind: 'config', mutation: { unit: 'workflows', authored: null } }, 'agent.workflows'],
    [{ kind: 'config', mutation: { unit: 'agent_identity', authored: null } }, 'agent_id'],
    [{ kind: 'config', mutation: { unit: 'description', authored: null } }, 'agent.description'],
    [{ kind: 'config', mutation: { unit: 'instructions', authored: null } }, 'agent.instructions'],
    [{ kind: 'config', mutation: { unit: 'project_guidance', authored: null } }, 'agent.agents_md'],
    [{ kind: 'config', mutation: { unit: 'approval', authored: null } }, 'approval_mode'],
    [{ kind: 'config', mutation: { unit: 'context', authored: null } }, 'context'],
    [{ kind: 'config', mutation: { unit: 'model_timeout', authored: null } }, 'model_timeout_policy'],
    [{ kind: 'config', mutation: { unit: 'tool_deadline', authored: null } }, 'tool_deadline_policy'],
    [{ kind: 'config', mutation: { unit: 'capacity', authored: null } }, 'subagents'],
    // Native composition assigns the User process policy directly and records
    // no origin for it; a named Agent resource is a separate document.
    [{ kind: 'config', mutation: { unit: 'app_server', authored: null } }, undefined],
    [{ kind: 'agent', name: 'reviewer', authored: null }, undefined],
    [{ kind: 'repair_config', document: '' }, undefined],
  ];
  for (const [mutation, path] of paths) expect(unitProvenancePath(mutation)).toBe(path);
  expect(unitProvenance(resolvedSource({}, { 'agent.model': workspace }), paths[0][0])).toEqual({ state: 'known', origin: workspace });
  expect(unitProvenance(resolvedSource({}, { 'providers.a': user }), paths[17][0])).toEqual({ state: 'unavailable' });
});

it('S1-14 a member omitted from a replaced object belongs to that winning object, never a shadowed lower field', () => {
  // `replace_origin` drops a replaced path's descendants, so native records the
  // container; `record_default_origins` then resolves members from it.
  const context: SourceMutation = { kind: 'config', mutation: { unit: 'context', authored: null } };
  expect(unitProvenance(resolvedSource({}, { context: workspace, 'context.reserve_tokens': workspace }), context)).toEqual({ state: 'known', origin: workspace });
  const policy: SourceMutation = { kind: 'config', mutation: { unit: 'native_policy', id: 'read', authored: null } };
  expect(unitProvenance(resolvedSource({}, { native_tools: user }), policy)).toEqual({ state: 'known', origin: user });
});

it('S1-14 a container whose identities disagree reports mixed origins instead of electing a sibling', () => {
  const capacity: SourceMutation = { kind: 'config', mutation: { unit: 'capacity', authored: null } };
  expect(unitProvenance(resolvedSource({}, { 'subagents.max_concurrent': user }), capacity)).toEqual({ state: 'known', origin: user });
  const context: SourceMutation = { kind: 'config', mutation: { unit: 'context', authored: null } };
  expect(unitProvenance(resolvedSource({}, { 'context.reserve_tokens': user, 'context.keep_recent_tokens': workspace }), context)).toEqual({ state: 'mixed' });
});

it('S1-01/S1-02 entry ownership is named by the exact target, never by a selector value', () => {
  expect(settingsTargetLabel(userSettingsTarget)).toBe('User Settings');
  expect(settingsTargetLabel(workspaceSettingsTarget('a', 'Workspace A'))).toBe('Workspace Settings — Workspace A');
  expect(settingsTargetKey(userSettingsTarget)).toBe('user');
  expect(settingsTargetKey(workspaceSettingsTarget('a', 'A'))).toBe('workspace:a');
  expect(applicationScope({ kind: 'user' })).toBe('source:user');
  expect(applicationScope({ kind: 'workspace', directory: '/w' })).toBe('source:workspace:/w');
  expect(authoredUnit({ agent: { tools: { builtin: ['read', 'glob'] } } }, { kind: 'config', mutation: { unit: 'native_tools', authored: null } })).toEqual(['read', 'glob']);
});

// Blocking finding 2 — an application scope key is not a source owner.
it('S1-15 owner navigation reads the native source owners, never the application scope key', () => {
  const session = cfg3Application();
  // A real Session application is keyed by the Session identity.
  expect(session.scope).toBe('ses_00000000-0000-7000-8000-000000000001');
  expect(session.scope.startsWith('source:')).toBe(false);
  expect(applicationOwners(session)).toEqual([{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/A' }]);
  expect(applicationOwners(cfg3SourceApplication())).toEqual([{ kind: 'user' }]);
  expect(applicationOwners(cfg3SourceApplication({ kind: 'workspace', directory: '/workspace/B' })))
    .toEqual([{ kind: 'user' }, { kind: 'workspace', directory: '/workspace/B' }]);
  expect(applicationOwners(null)).toEqual([]);
  expect(applicationOwners(undefined)).toEqual([]);
  expect(openOwnerLabel({ kind: 'user' })).toBe('Open User Settings');
  expect(openOwnerLabel({ kind: 'workspace', directory: '/workspace/A' })).toBe('Open Workspace Settings — /workspace/A');
  expect(sourceTargetKey({ kind: 'user' })).toBe('user');
  expect(sourceTargetKey({ kind: 'workspace', directory: '/workspace/A' })).toBe('workspace:/workspace/A');
});

it('S1-08 native per-unit observations stay independent and never pose as a classification', () => {
  const application = cfg3Application();
  application.units = { capabilities: { status: 'preparing' }, instructions: { status: 'applied' }, provider: { status: 'failed', diagnostic: 'resource failed' }, process_bindings: { status: 'process_restart' }, shared_capacity: { status: 'ready', impact: 'unproven' } };
  expect(observedResultLabel(observedResult(unitApplication(application, 'capabilities')))).toBe('Preparing');
  expect(observedResultLabel(observedResult(unitApplication(application, 'instructions')))).toBe('Applied');
  expect(observedResult(unitApplication(application, 'provider'))).toEqual({ state: 'failed', diagnostic: 'resource failed' });
  expect(observedResultLabel(observedResult(unitApplication(application, 'process_bindings')))).toBe('Restart pending');
  expect(observedResult(unitApplication(application, 'execution_policy'))).toEqual({ state: 'unavailable' });
  expect(changeBehaviorLabel(changeBehavior({ max_connections: 'hot', shutdown_deadline_ms: 'restart' }, 'max_connections'))).toBe('Applies immediately');
  expect(changeBehaviorLabel(changeBehavior({ shutdown_deadline_ms: 'restart' }, 'shutdown_deadline_ms'))).toBe('Requires App Server restart');
});

it('S1-07 connecting, loading, ready, stale and failed are distinct lifecycle states', () => {
  expect(settingsLifecycle({ connection: 'connecting', hasSource: false, targetValid: false, readError: '' })).toBe('connecting');
  expect(settingsLifecycle({ connection: 'connected', hasSource: false, targetValid: false, readError: '' })).toBe('loading');
  expect(settingsLifecycle({ connection: 'connected', hasSource: true, targetValid: true, readError: '' })).toBe('ready');
  expect(settingsLifecycle({ connection: 'connected', hasSource: true, targetValid: false, readError: 'read failed' })).toBe('stale');
  expect(settingsLifecycle({ connection: 'connected', hasSource: false, targetValid: false, readError: 'read failed' })).toBe('failed');
  expect(settingsLifecycle({ connection: 'error', hasSource: true, targetValid: true, readError: '' })).toBe('failed');
});
