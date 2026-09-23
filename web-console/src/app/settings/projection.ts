import type {
  ConfigurationApplication, Origin, ProcessPolicyImpact, ResourceFamily, RuntimeLayer,
  SourceMutation, SourceScope, SourceSettings, SourceTarget, SourceView, UnitApplication,
} from '../../../../protocol/app-server/v18';
import type { ConnectionState } from '../../client/app-server';

/** The single owner a Settings instance is bound to for its whole lifetime.
 * There is deliberately no ordinary User/Workspace selector: the global entry
 * opens User authoring and the exact Workspace object entry opens Workspace
 * authoring. Presenting one target's facts never merges another target's
 * meaning. */
export interface UserSettingsTarget { kind: 'user' }
export interface WorkspaceSettingsTarget { kind: 'workspace'; id: string; displayName: string }
export type SettingsTarget = UserSettingsTarget | WorkspaceSettingsTarget;

export const userSettingsTarget: UserSettingsTarget = { kind: 'user' };
export function workspaceSettingsTarget(id: string, displayName: string = id): WorkspaceSettingsTarget {
  return { kind: 'workspace', id, displayName };
}
export function settingsTargetScope(target: SettingsTarget): SourceScope { return target.kind; }
export function settingsTargetKey(target: SettingsTarget): string {
  return target.kind === 'user' ? 'user' : `workspace:${target.id}`;
}
export function settingsTargetLabel(target: SettingsTarget): string {
  return target.kind === 'user' ? 'User Settings' : `Workspace Settings — ${target.displayName}`;
}

/** The native application scope this source target publishes under, exactly as
 * `SourceTarget::application_scope` names it. Application versions are u64
 * counters comparable only inside one scope, authority and connection lifetime. */
export function applicationScope(target: SourceTarget): string {
  return target.kind === 'user' ? 'source:user' : `source:workspace:${target.directory}`;
}

/** The native projection of this unit carries its presence but never its
 * value, because the value is a secret-bearing literal that native authority
 * never releases. Presence and absence stay distinguishable; the literal is not
 * representable at all, so no surface can render, copy or retain it. */
export const REDACTED = Symbol('native value not projected');

/** Project authored membership only. Defaults and merging remain native. */
export function authoredUnit(document: RuntimeLayer | null | undefined, mutation: SourceMutation): unknown {
  if (!document || mutation.kind !== 'config') return undefined;
  const unit = mutation.mutation;
  switch (unit.unit) {
    case 'provider': return document.providers?.[unit.id];
    case 'model': return document.models?.[unit.id];
    case 'root_model': return document.agent?.model;
    case 'native_tools': return document.agent?.tools?.builtin;
    case 'source_tools': return document.agent?.tools?.sources?.[unit.id];
    case 'skills': return document.agent?.skills;
    case 'todo': return document.agent?.plugins?.todo;
    case 'goal': return document.agent?.plugins?.goal;
    case 'agent_status': return document.agent?.plugins?.agent_status;
    case 'agents': return document.agent?.agents;
    case 'workflows': return document.agent?.workflows;
    case 'agent_identity': return document.agent_id;
    case 'description': return document.agent?.description;
    case 'instructions': return document.agent?.instructions;
    case 'project_guidance': return document.agent?.agents_md;
    case 'approval': return document.approval_mode;
    case 'context': return document.context;
    case 'model_timeout': return document.model_timeout_policy;
    case 'tool_deadline': return document.tool_deadline_policy;
    case 'capacity': return document.subagents;
    case 'native_policy': return document.native_tools?.[unit.id];
    case 'mcp_policy': return document.mcp_tool_policies?.[unit.id];
    case 'environment': return document.environment?.includes(unit.name) ? REDACTED : undefined;
    case 'app_server': return document.app_server;
  }
}

/** The exact native provenance key one semantic unit owns.
 *
 * Native `RuntimeLayer::overlay` records provenance by dotted field path:
 * `replace` records the whole-unit path it moved, and `named` records
 * `<container>.<identity>` for every identity it replaced. `replace_origin`
 * then drops that path's descendants, so an identity-bearing unit is always
 * addressed by its own exact key and never by its container. Sibling
 * identities are independent facts; their names, lengths and insertion order
 * are irrelevant to this lookup.
 *
 * `app_server` is deliberately absent: native composition assigns the User
 * document's process policy directly and records no origin for it, so this
 * projection reports no origin rather than inventing one. Named Agent and MCP
 * resources are separate documents outside `RuntimeLayer`; only MCP server
 * definitions carry a `mcp_servers.<id>` default origin. */
export function unitProvenancePath(mutation: SourceMutation): string | undefined {
  if (mutation.kind === 'mcp') return `mcp_servers.${mutation.id}`;
  if (mutation.kind === 'agent' || mutation.kind === 'repair_config') return undefined;
  const unit = mutation.mutation;
  switch (unit.unit) {
    case 'provider': return `providers.${unit.id}`;
    case 'model': return `models.${unit.id}`;
    case 'root_model': return 'agent.model';
    case 'native_tools': return 'agent.tools.builtin';
    case 'source_tools': return `agent.tools.sources.${unit.id}`;
    case 'skills': return 'agent.skills';
    case 'todo': return 'agent.plugins.todo';
    case 'goal': return 'agent.plugins.goal';
    case 'agent_status': return 'agent.plugins.agent_status';
    case 'agents': return 'agent.agents';
    case 'workflows': return 'agent.workflows';
    case 'agent_identity': return 'agent_id';
    case 'description': return 'agent.description';
    case 'instructions': return 'agent.instructions';
    case 'project_guidance': return 'agent.agents_md';
    case 'approval': return 'approval_mode';
    case 'context': return 'context';
    case 'model_timeout': return 'model_timeout_policy';
    case 'tool_deadline': return 'tool_deadline_policy';
    case 'capacity': return 'subagents';
    case 'native_policy': return `native_tools.${unit.id}`;
    case 'mcp_policy': return `mcp_tool_policies.${unit.id}`;
    case 'environment': return `environment.${unit.name}`;
    case 'app_server': return undefined;
  }
}

/** One unit's native origin. A container whose identities were recorded by
 * different layers has no single origin; that is reported as `mixed` rather
 * than resolved by electing a sibling key. */
export type UnitOrigin =
  | { state: 'known'; origin: Origin }
  | { state: 'mixed' }
  | { state: 'unavailable' };
function sameOrigin(left: Origin, right: Origin): boolean {
  if (left.kind !== right.kind) return false;
  if (left.kind === 'builtin' || right.kind === 'builtin') return true;
  const base = 'base' in left && 'base' in right && left.base === right.base;
  const document = ('document' in left ? left.document : undefined) === ('document' in right ? right.document : undefined);
  return base && document;
}
/** Native provenance keyed by exact dotted field path.
 *
 * 1. the unit's own key, when a layer replaced exactly this unit;
 * 2. otherwise its nearest recorded ancestor, because a member omitted from a
 *    replaced object belongs to that winning object (native resolves the same
 *    container chain in `record_default_origins`);
 * 3. otherwise the unit's recorded members, which have one origin only when
 *    they agree. */
export function unitProvenance(source: SourceSettings | undefined, mutation: SourceMutation): UnitOrigin {
  const provenance = source?.provenance;
  const path = unitProvenancePath(mutation);
  if (!provenance || path === undefined) return { state: 'unavailable' };
  const exact = provenance[path];
  if (exact) return { state: 'known', origin: exact };
  for (let parent = path; parent.includes('.');) {
    parent = parent.slice(0, parent.lastIndexOf('.'));
    const owner = provenance[parent];
    if (owner) return { state: 'known', origin: owner };
  }
  const members = Object.entries(provenance).filter(([key]) => key.startsWith(path + '.')).map(([, origin]) => origin);
  if (!members.length) return { state: 'unavailable' };
  return members.every(origin => sameOrigin(origin, members[0])) ? { state: 'known', origin: members[0] } : { state: 'mixed' };
}
export function provenanceLabel(origin: UnitOrigin): string {
  if (origin.state === 'unavailable') return 'Origin not reported';
  if (origin.state === 'mixed') return 'Mixed origins';
  return origin.origin.kind === 'builtin' ? 'Native default'
    : origin.origin.kind === 'user' ? 'Inherited from User'
      : origin.origin.kind === 'workspace' ? 'Workspace override'
        : 'Process default';
}

/** One semantic unit's authored and effective facts, kept orthogonal.
 *
 * Authored state answers "does this exact scope author this unit"; effective
 * state answers "did native resolution produce a value for it". They are
 * independent: a valid Workspace document that authors nothing still has an
 * unavailable effective value when the User document does not parse. Absent is
 * never `false`, `[]` or `{}`, and invalid/unavailable are never empty or a
 * client fallback. */
export type AuthoredState = 'present' | 'redacted' | 'absent' | 'invalid' | 'unavailable';
export type EffectiveState = 'available' | 'redacted' | 'unset' | 'invalid' | 'unavailable';
export interface AuthoredFacts { state: AuthoredState; value?: unknown; diagnostic?: string }
export interface EffectiveFacts { state: EffectiveState; value?: unknown; diagnostic?: string }
export interface UnitFacts { authored: AuthoredFacts; effective: EffectiveFacts; origin: UnitOrigin }
export function sourceView(source: SourceSettings | undefined, scope: SourceScope): SourceView | undefined {
  if (!source) return undefined;
  return scope === 'user' ? source.user : source.workspace ?? undefined;
}

/** Which mutations one native document admits right now.
 *
 * Native parses a document before applying any structured mutation to it, and
 * reports a parse failure as `diagnostic` with no `authored` layer. A document
 * that does not parse therefore admits no structured mutation at all, and the
 * browser must not present editors for mutations native will inevitably
 * reject. That fact belongs to the document, not to each editor, so it is
 * decided here once:
 *
 * - `structured` — the document parsed; semantic-unit editing is available;
 * - `malformed` — the document did not parse; for `rustx.toml` the one
 *   mutation native accepts is `repair_config`, fenced on this revision;
 * - `unavailable` — this scope has no view of the document at all.
 *
 * Each native document is its own authority: a malformed `rustx.toml` says
 * nothing about an MCP document, a named Agent resource or any inventory. */
export type DocumentAuthoring<T> =
  | { state: 'structured'; path: string; revision: string; document: T }
  | { state: 'malformed'; path: string; revision: string; diagnostic: string }
  | { state: 'unavailable' };
export function documentAuthoring<T>(view: { path: string; revision: string; authored?: T | null; diagnostic?: string | null } | null | undefined): DocumentAuthoring<T> {
  if (!view) return { state: 'unavailable' };
  const { path, revision } = view;
  if (view.authored) return { state: 'structured', path, revision, document: view.authored };
  return { state: 'malformed', path, revision, diagnostic: view.diagnostic ?? 'Source document was not loaded.' };
}
/** This scope's `rustx.toml`. */
export function configAuthoring(source: SourceSettings | undefined, scope: SourceScope): DocumentAuthoring<RuntimeLayer> {
  return documentAuthoring(sourceView(source, scope));
}
/** Native authored membership for exactly this scope. A parse failure means the
 * document was not loaded, so membership is unknown, never absent. */
export function authoredFacts(source: SourceSettings | undefined, scope: SourceScope, mutation: SourceMutation): AuthoredFacts {
  const view = sourceView(source, scope);
  if (!source || !view) return { state: 'unavailable' };
  if (view.diagnostic) return { state: 'invalid', diagnostic: view.diagnostic };
  if (!view.authored) return { state: 'unavailable' };
  const value = authoredUnit(view.authored, mutation);
  if (value === undefined) return { state: 'absent' };
  return value === REDACTED ? { state: 'redacted' } : { state: 'present', value };
}
/** Native source resolution for this unit. `resolved` is absent exactly when a
 * participating document failed to parse, and `prospective_diagnostic` reports
 * a merged configuration that parsed but does not resolve. Neither is an empty
 * value, an unset unit or a native default. */
export function effectiveFacts(source: SourceSettings | undefined, mutation: SourceMutation): EffectiveFacts {
  if (!source) return { state: 'unavailable' };
  const diagnostic = source.prospective_diagnostic ?? undefined;
  if (!source.resolved) return diagnostic ? { state: 'invalid', diagnostic } : { state: 'unavailable' };
  if (diagnostic) return { state: 'invalid', diagnostic };
  const value = authoredUnit(source.resolved, mutation);
  if (value === undefined) return { state: 'unset' };
  return value === REDACTED ? { state: 'redacted' } : { state: 'available', value };
}
export function unitFacts(source: SourceSettings | undefined, scope: SourceScope, mutation: SourceMutation): UnitFacts {
  return { authored: authoredFacts(source, scope, mutation), effective: effectiveFacts(source, mutation), origin: unitProvenance(source, mutation) };
}
export function authoredStateLabel(authored: AuthoredFacts, scope: SourceScope): string {
  const workspace = scope === 'workspace';
  return authored.state === 'present' ? (workspace ? 'Workspace override — empty selections remain explicit' : 'User authored value')
    : authored.state === 'redacted' ? (workspace ? 'Workspace override — value never projected' : 'User authored value — value never projected')
      : authored.state === 'absent' ? (workspace ? 'Inherited — no Workspace override' : 'No User authored value')
        : authored.state === 'invalid' ? 'Authored source is invalid'
          : 'Authored source unavailable';
}
export function effectiveStateLabel(effective: EffectiveFacts): string {
  return effective.state === 'available' ? 'Native effective value available'
    : effective.state === 'redacted' ? 'Native effective value exists — the literal is never projected'
      : effective.state === 'unset' ? 'No source authors this unit — native default applies'
        : effective.state === 'invalid' ? 'Native effective value unavailable — resolution failed'
          : 'Native effective value not observed';
}

/** The non-sensitive native selector that names which source revision settles
 * one submitted mutation.
 *
 * A mutation's authored payload may carry Provider credentials, MCP literal
 * environment values or literal headers. Settlement never needs any of them: it
 * needs only which native document the commit landed in, which is the mutation
 * *family* plus, for a named Agent resource, its identity. Deriving the selector
 * at submission time is what lets the transaction owner keep a submitted
 * mutation settleable without retaining its secret-bearing payload. */
export type RevisionSelector =
  | { kind: 'config' }
  | { kind: 'mcp' }
  | { kind: 'agent'; name: string };
export function revisionSelector(mutation: SourceMutation): RevisionSelector {
  return mutation.kind === 'agent' ? { kind: 'agent', name: mutation.name }
    : mutation.kind === 'mcp' ? { kind: 'mcp' } : { kind: 'config' };
}

/** The exact revision this projection carries for the selected native document
 * of this projection's own scope. An identity the scope does not author yet has
 * the native absent-resource revision, never a fabricated one. */
export function selectedRevision(source: SourceSettings, selector: RevisionSelector): string {
  const scope = source.target.kind;
  if (selector.kind === 'config') return source[scope]!.revision;
  if (selector.kind === 'mcp') return (scope === 'user' ? source.user_mcp : source.workspace_mcp)!.revision;
  return source.agents.find(agent => agent.scope === scope && agent.name === selector.name)?.source.revision ?? source.absent_resource_revision;
}

/** One named-catalog identity, with this scope's authored ownership kept
 * strictly separate from the native effective fact.
 *
 * `authored` is what this exact scope's document declares for the identity and
 * is `undefined` when it declares none; `effective` is what native resolution
 * produced. The browser never merges two documents to manufacture either one:
 * enumeration comes from the native resolved layer, ownership from this scope's
 * own authored layer, and provenance from native `provenance`. */
export interface CatalogEntry<T> { id: string; authored?: T; effective?: T; origin: UnitOrigin }
export type CatalogContainer = 'providers' | 'models';
export function catalogMutation(container: CatalogContainer, id: string): SourceMutation {
  return { kind: 'config', mutation: container === 'providers' ? { unit: 'provider', id } : { unit: 'model', id } };
}
/** Every identity of one named catalog this Settings surface must be able to
 * reach.
 *
 * A Workspace reaches the native effective identities as well as the ones it
 * authors, so an identity it inherits is discoverable without being retyped;
 * an authored identity native resolution did not produce is still listed, since
 * an unresolvable lower document leaves `resolved` absent without making this
 * scope's own authoring vanish. User authoring inherits from nothing — it is the
 * lowest authored source — so its catalog is exactly what it authors and never
 * presents a Workspace-owned identity as something User may override. Native
 * order is preserved; no client ordering is invented. */
export function catalogEntries<T>(source: SourceSettings | undefined, scope: SourceScope, container: CatalogContainer): CatalogEntry<T>[] {
  const authored = (sourceView(source, scope)?.authored?.[container] ?? undefined) as Record<string, T> | undefined;
  const effective = (source?.resolved?.[container] ?? undefined) as Record<string, T> | undefined;
  return catalogIdentities(source, scope, container)
    .map(id => ({ id, authored: authored?.[id], effective: effective?.[id], origin: unitProvenance(source, catalogMutation(container, id)) }));
}
/** The one identity-discovery rule of a named catalog, shared by every Settings
 * surface that lists or selects its identities — the Providers & Models
 * catalog, the Root model selector and a named Agent's explicit-model
 * selector all reach exactly these, so they can never disagree.
 *
 * - User: exactly the identities the User document authors.
 * - Workspace: exactly the identities the Workspace document authors, together
 *   with the native effective identities whenever native resolution produced
 *   them.
 *
 * Authored and effective facts are orthogonal. A native resolution failure —
 * say, a malformed User document — removes the effective identities and
 * nothing else: what this exact scope's own valid document authors is still
 * observed, and still listed. No other scope's authored layer ever stands in for
 * a missing effective one, so no inheritance is reconstructed here. */
export function catalogIdentities(source: SourceSettings | undefined, scope: SourceScope, container: CatalogContainer): string[] {
  return reachableIdentities(scope, sourceView(source, scope)?.authored?.[container], source?.resolved?.[container]);
}
/** The identities of one named semantic-unit container this scope must be able
 * to reach, on the same terms as a catalog: a Workspace reaches the native
 * effective identities as well as its own, User reaches exactly its own. Native
 * order is preserved. */
export function reachableIdentities(scope: SourceScope, authored: object | null | undefined, effective: object | null | undefined): string[] {
  return [...new Set([...(scope === 'workspace' ? Object.keys(effective ?? {}) : []), ...Object.keys(authored ?? {})])];
}
/** The environment identities this scope must be able to reach, on exactly the
 * same terms as any other named container. Native projects the identities
 * alone — `RuntimeLayer.environment` is a list, not a map — so enumeration here
 * cannot expose a literal value even by accident. */
export function reachableEnvironment(scope: SourceScope, authored: readonly string[] | null | undefined, effective: readonly string[] | null | undefined): string[] {
  return [...new Set([...(scope === 'workspace' ? effective ?? [] : []), ...(authored ?? [])])];
}

/** One native whole-file resource identity, exactly as the native inventory
 * reports it. Resource families are owned as whole identities — a Workspace
 * definition shadows the entire same-name User definition — so the winning
 * scope is a native fact and is never recomputed from two authored catalogs. */
export interface ResourceIdentity { name: string; scope: SourceScope; path: string; valid: boolean; shadowed?: string }
export function resourceIdentities(source: SourceSettings | undefined, family: ResourceFamily): readonly ResourceIdentity[] {
  return (source?.prospective_resources?.definitions ?? []).filter(entry => entry.family === family)
    .map(entry => ({ name: entry.name, scope: entry.location.scope, path: entry.location.path, valid: entry.valid, shadowed: entry.location.shadowed ?? undefined }));
}
/** The resource identities a Workspace surface must be able to reach although
 * this Workspace authors none of them. Only a Workspace inherits: a Workspace
 * definition shadows the User one, so User authoring is never presented as
 * inheriting from a Workspace. */
export function inheritedResources(source: SourceSettings | undefined, scope: SourceScope, family: ResourceFamily, authored: readonly string[]): readonly ResourceIdentity[] {
  if (scope !== 'workspace') return [];
  return resourceIdentities(source, family).filter(entry => entry.scope === 'user' && !authored.includes(entry.name));
}

/** Owner navigation from native facts only.
 *
 * `ConfigurationApplication.scope` is an application-scope key — a Session
 * identity for a Session application — and is never a source owner. Native
 * names the authored owners separately in `sources`, lowest authority first,
 * so the browser never parses a scope string, guesses from a Session cwd or
 * rebuilds source ownership of its own. */
export function applicationOwners(application: ConfigurationApplication | null | undefined): readonly SourceTarget[] {
  return application?.sources ?? [];
}
export function sourceTargetKey(target: SourceTarget): string {
  return target.kind === 'user' ? 'user' : `workspace:${target.directory}`;
}
export function openOwnerLabel(target: SourceTarget): string {
  return target.kind === 'user' ? 'Open User Settings' : `Open Workspace Settings — ${target.directory}`;
}

/** Per-unit native application/process observation. A unit is never inferred
 * from a sibling: independent units may simultaneously be applied, preparing,
 * failed and restart-pending. */
export type ObservedUnit = 'capabilities' | 'execution_policy' | 'instructions' | 'provider' | 'shared_capacity' | 'process_bindings';
export const observedUnits: readonly ObservedUnit[] = ['capabilities', 'execution_policy', 'instructions', 'provider', 'shared_capacity', 'process_bindings'];
export function unitApplication(application: ConfigurationApplication | null | undefined, unit: ObservedUnit): UnitApplication | undefined {
  return application?.units?.[unit];
}
export function observedUnitLabel(unit: ObservedUnit): string {
  return unit === 'process_bindings' ? 'Process bindings'
    : unit === 'capabilities' ? 'Capabilities'
      : unit === 'execution_policy' ? 'Execution policy'
        : unit === 'instructions' ? 'Instructions'
          : unit === 'provider' ? 'Providers & Models'
            : 'Shared capacity';
}
/** A truthful observed result. `ready` carries native cache impact; `applied`
 * is the native-confirmed state and is never called a classification. */
export type ObservedResult =
  | { state: 'applied' }
  | { state: 'preparing' }
  | { state: 'ready'; impact: string }
  | { state: 'failed'; diagnostic: string }
  | { state: 'restart_pending' }
  | { state: 'unavailable' };
export function observedResult(unit: UnitApplication | undefined): ObservedResult {
  if (!unit) return { state: 'unavailable' };
  if (unit.status === 'applied') return { state: 'applied' };
  if (unit.status === 'preparing') return { state: 'preparing' };
  if (unit.status === 'failed') return { state: 'failed', diagnostic: unit.diagnostic };
  if (unit.status === 'process_restart') return { state: 'restart_pending' };
  return { state: 'ready', impact: unit.impact };
}
export function observedResultLabel(result: ObservedResult): string {
  return result.state === 'applied' ? 'Applied'
    : result.state === 'preparing' ? 'Preparing'
      : result.state === 'failed' ? 'Failed'
        : result.state === 'restart_pending' ? 'Restart pending'
          : result.state === 'ready' ? 'Ready'
            : 'Not observed';
}

/** Native change behavior, distinct from an observed result. */
export type ChangeBehavior = 'immediate' | 'restart';
export function changeBehavior(impacts: Record<string, ProcessPolicyImpact> | undefined, key: string): ChangeBehavior | undefined {
  const impact = impacts?.[key];
  return impact === undefined ? undefined : impact === 'hot' ? 'immediate' : 'restart';
}
export function changeBehaviorLabel(behavior: ChangeBehavior | undefined): string {
  return behavior === 'immediate' ? 'Applies immediately'
    : behavior === 'restart' ? 'Requires App Server restart'
      : 'Change behavior not reported';
}

/** Distinct connection/read lifecycle states. `loading` is not a synonym for
 * every missing source, and `stale` keeps the last authoritative observation
 * visible instead of fabricating a fresh one. */
export type SettingsLifecycle = 'connecting' | 'loading' | 'ready' | 'stale' | 'failed';
export function settingsLifecycle(input: { connection: ConnectionState; hasSource: boolean; targetValid: boolean; readError: string }): SettingsLifecycle {
  if (input.connection === 'connecting' || input.connection === 'reconnecting' || input.connection === 'resynchronizing') return 'connecting';
  if (input.connection === 'connected') {
    if (!input.hasSource) return input.readError ? 'failed' : 'loading';
    return input.targetValid ? 'ready' : 'stale';
  }
  return 'failed';
}
export function settingsLifecycleLabel(lifecycle: SettingsLifecycle): string {
  return lifecycle === 'connecting' ? 'Connecting to the App Server…'
    : lifecycle === 'loading' ? 'Loading authoritative sources…'
      : lifecycle === 'ready' ? 'Authoritative source observed'
        : lifecycle === 'stale' ? 'Last observation retained; current status uncertain'
          : 'Source authority unavailable';
}
