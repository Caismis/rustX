import type {
  ConfigurationApplication, Origin, ProcessPolicyImpact, RuntimeLayer,
  SourceMutation, SourceScope, SourceSettings, SourceTarget, SourceView, UnitApplication,
} from '../../../../protocol/app-server/v17';
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
    case 'environment': return document.environment?.[unit.name];
    case 'app_server': return document.app_server;
  }
}

/** The native provenance prefixes one semantic unit owns. Native resolution and
 * the overlay remain the sole authority; this only names which authored unit a
 * computed origin belongs to. */
export function unitProvenancePrefixes(mutation: SourceMutation): string[] {
  if (mutation.kind === 'mcp') return ['mcp_servers'];
  if (mutation.kind === 'agent') return ['agents'];
  if (mutation.kind === 'repair_config') return [];
  switch (mutation.mutation.unit) {
    case 'provider': return ['providers'];
    case 'model': return ['models'];
    case 'root_model': return ['agent.model'];
    case 'native_tools': return ['agent.tools.builtin'];
    case 'source_tools': return ['agent.tools.sources'];
    case 'skills': return ['agent.skills'];
    case 'todo': return ['agent.plugins.todo'];
    case 'goal': return ['agent.plugins.goal'];
    case 'agent_status': return ['agent.plugins.agent_status'];
    case 'agents': return ['agent.agents'];
    case 'workflows': return ['agent.workflows'];
    case 'agent_identity': return ['agent_id'];
    case 'description': return ['agent.description'];
    case 'instructions': return ['agent.instructions'];
    case 'project_guidance': return ['agent.agents_md'];
    case 'approval': return ['approval_mode'];
    case 'context': return ['context'];
    case 'model_timeout': return ['model_timeout_policy'];
    case 'tool_deadline': return ['tool_deadline_policy'];
    case 'capacity': return ['subagents'];
    case 'native_policy': return ['native_tools'];
    case 'mcp_policy': return ['mcp_tool_policies'];
    case 'environment': return ['environment'];
    case 'app_server': return ['app_server'];
  }
}

/** Native provenance keyed by dotted field path. The longest matching prefix is
 * the unit's origin; an unknown unit reports no origin rather than inventing a
 * User/Workspace/builtin claim. */
export function unitProvenance(source: SourceSettings | undefined, mutation: SourceMutation): Origin | undefined {
  const provenance = source?.provenance;
  if (!provenance) return undefined;
  const prefixes = unitProvenancePrefixes(mutation);
  let best: string | undefined;
  for (const key of Object.keys(provenance)) {
    if (!prefixes.some(prefix => key === prefix || key.startsWith(prefix + '.') || prefix.startsWith(key + '.'))) continue;
    if (best === undefined || key.length > best.length) best = key;
  }
  return best === undefined ? undefined : provenance[best];
}
export type ProvenanceOrigin = Origin | undefined;
export function provenanceLabel(origin: ProvenanceOrigin): string {
  if (!origin) return 'Native-resolved value';
  return origin.kind === 'builtin' ? 'Native default'
    : origin.kind === 'user' ? 'Inherited from User'
      : origin.kind === 'workspace' ? 'Workspace override'
        : 'Process default';
}

/** One semantic unit's distinct authored/effective/availability facts. These are
 * never collapsed: absent is not `false`, `[]` or `{}`; invalid and unavailable
 * are neither empty nor a client fallback. */
export type UnitPresence = 'authored' | 'absent' | 'invalid' | 'unavailable';
export interface UnitFacts {
  presence: UnitPresence;
  /** Native authored membership for this exact unit; `undefined` when omitted. */
  authored: unknown;
  /** Native effective/resolved value for this exact unit; never a client merge. */
  effective: unknown;
  /** Native provenance of the effective value, when the DTO carries one. */
  origin: ProvenanceOrigin;
  /** Native diagnostic when the owning source document is malformed. */
  diagnostic?: string;
}
export function sourceView(source: SourceSettings | undefined, scope: SourceScope): SourceView | undefined {
  if (!source) return undefined;
  return scope === 'user' ? source.user : source.workspace ?? undefined;
}
export function unitFacts(source: SourceSettings | undefined, scope: SourceScope, mutation: SourceMutation): UnitFacts {
  const view = sourceView(source, scope);
  if (!source || !view) return { presence: 'unavailable', authored: undefined, effective: undefined, origin: undefined };
  const effective = authoredUnit(source.resolved, mutation);
  const authored = authoredUnit(view.authored, mutation);
  const origin = unitProvenance(source, mutation);
  if (view.diagnostic) return { presence: 'invalid', authored, effective, origin, diagnostic: view.diagnostic };
  if (authored === undefined) return { presence: 'absent', authored: undefined, effective, origin };
  return { presence: 'authored', authored, effective, origin };
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
