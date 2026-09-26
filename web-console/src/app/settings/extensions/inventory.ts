import type { Translate } from '../../../locale/translation';
import type {
  CapabilityInspection1, ResourceDiagnostic, ResourceFamily, SourceInspection, SourceScope, SourceSettings, WorkflowInspection,
} from '../../../../../protocol/app-server/v23';
import { resourceCapability, toolSourceId, type ResourceCapability } from '../capability';
import type { ExtensionFamily } from '../projection';

/** How one resource identity relates to the scope currently being edited.
 *
 * These are four different native facts, not four renderings of one:
 *
 * - `authored`   — this scope owns the winning definition, and nothing lost;
 * - `overriding` — this scope's definition shadows a same-name one elsewhere;
 * - `inherited`  — another scope owns it and this scope authors no override;
 * - `shadowed`   — this scope authors one, and a higher-precedence scope's
 *                  definition won. The losing definition is not parsed or
 *                  prepared, and hiding the winner behind it would be a lie. */
export type ExtensionRelationship = 'authored' | 'overriding' | 'inherited' | 'shadowed';

/** Whether native produced an effective value for this identity, and whether
 * this scope can act on it. Every field is an independent native fact: valid
 * is not prepared, prepared is not selected, and selected is not connected. */
export interface ExtensionEntry {
  readonly family: ExtensionFamily;
  readonly name: string;
  /** The scope that owns the winning definition. */
  readonly owner: SourceScope;
  readonly relationship: ExtensionRelationship;
  /** The winning definition's source path. */
  readonly path: string;
  /** The losing same-name definition's path, when native recorded one. */
  readonly shadowed?: string;
  /** Native's own validity verdict for the winning definition, or `undefined`
   * when the native resource inventory has not reported one for this identity
   * yet — which is what a definition this scope has just authored looks like
   * until the next inspection. Unobserved is never rendered as invalid. */
  readonly valid?: boolean;
  /** The native preparation status of an MCP or Managed Python source, or the
   * native admission status of a Workflow, exactly as native published it.
   * `undefined` is not a negative status: it is the distinct fact that native
   * published no observation for this identity. It is never inferred from
   * validity and never produced by probing. */
  readonly preparation?: Preparation;
  /** Whether the root/default Agent may use this resource, as the native
   * capability inspection reports it. `undefined` is not "no": it is the
   * distinct fact that native published no root inspection to read it from. */
  readonly selected?: boolean;
  /** The diagnostics native attributes to exactly this identity. A sibling
   * defined in the same file never contributes one. */
  readonly diagnostics: readonly string[];
  readonly capability: ResourceCapability;
}

/** A native preparation or admission status, never a presentation state. */
export type Preparation = SourceInspection['status'] | WorkflowInspection['status'];

export function validityLabel(tx: Translate, entry: ExtensionEntry): string {
  return entry.valid === undefined ? tx('settings:copy.validity-not-observed') : entry.valid ? tx('settings:copy.valid-definition') : tx('settings:copy.invalid-definition');
}

export const resourceFamilies: readonly ResourceFamily[] = ['mcp', 'skill', 'agent', 'workflow', 'managed_python'];

/** The native observation for this identity, or `undefined` when native
 * published none. Absence stays absence: it is never read as `unprepared`,
 * `disabled` or any other status native did not say. */
function preparationOf(resources: CapabilityInspection1, family: ResourceFamily, name: string): Preparation | undefined {
  if (family === 'mcp' || family === 'managed_python') return resources.sources[toolSourceId(family, name)]?.status;
  if (family === 'workflow') return resources.workflows[name]?.status;
  return undefined;
}

/** The diagnostics native attributed to exactly this resource identity. */
function diagnosticsOf(resources: CapabilityInspection1, family: ResourceFamily, name: string): readonly string[] {
  return resources.resource_diagnostics
    .filter(item => item.subject.kind === 'resource' && item.subject.family === family && item.subject.name === name)
    .map(item => item.reason);
}

/** The diagnostics of one family's source documents or collections as a whole.
 * Native attributes them to no identity, so they are presented for the family
 * and never copied onto the resources that family's documents hold. */
export function collectionDiagnostics(source: SourceSettings | undefined, families: readonly ResourceFamily[]): readonly ResourceDiagnostic[] {
  return (source?.prospective_resources?.resource_diagnostics ?? [])
    .filter(item => item.subject.kind === 'collection' && families.includes(item.subject.family));
}

function selectedOf(resources: CapabilityInspection1, family: ResourceFamily, name: string): boolean | undefined {
  const main = resources.main;
  if (!main) return undefined;
  if (family === 'skill') return main.skills.some(skill => skill.name === name);
  if (family === 'agent') return main.agents.includes(name);
  if (family === 'workflow') return main.workflows.includes(name);
  const id = toolSourceId(family, name);
  return main.tool_selection.some(selection => selection.origin !== 'builtin' && selection.source_id === id);
}

/** Every resource identity of one family, with each native fact kept apart.
 *
 * Enumeration is a native fact: `CapabilityInspection.definitions` names the
 * winning scope of every identity, so the browser never merges two authored
 * catalogs, never recomputes precedence and never decides ownership of its own.
 * A shadowed same-name definition is listed as the losing definition it is
 * rather than used to stand in for the winner. */
export function extensionEntries(source: SourceSettings | undefined, scope: SourceScope, family: ResourceFamily): readonly ExtensionEntry[] {
  const resources = source?.prospective_resources;
  const capability = resourceCapability(family);
  const inventoried = !resources ? [] : resources.definitions.filter(entry => entry.family === family).map(entry => {
    const owner = entry.location.scope;
    const relationship: ExtensionRelationship = owner === scope
      ? (entry.location.shadowed ? 'overriding' : 'authored')
      : scope === 'workspace' ? 'inherited' : 'shadowed';
    return {
      family, name: entry.name, owner, relationship,
      path: entry.location.path,
      ...(entry.location.shadowed ? { shadowed: entry.location.shadowed } : {}),
      valid: entry.valid,
      ...(capability.preparation ? { preparation: preparationOf(resources, family, entry.name) } : {}),
      ...(capability.selection !== 'none' ? { selected: selectedOf(resources, family, entry.name) } : {}),
      diagnostics: diagnosticsOf(resources, family, entry.name),
      capability,
    };
  });
  // A definition this scope authors in its own resource document is a native
  // fact of that document, and it must stay reachable even before the resource
  // inspection reports it — otherwise saving a new MCP server or named Agent
  // would make it vanish from the surface that just created it. The inventory
  // stays authoritative for every identity it does name.
  const named = new Set(inventoried.map(entry => entry.name));
  const authored = authoredDefinitions(source, scope, family).filter(entry => !named.has(entry.name));
  return [...inventoried, ...authored.map(entry => ({
    family, name: entry.name, owner: scope, relationship: 'authored' as const, path: entry.path,
    ...(entry.valid === undefined ? {} : { valid: entry.valid }),
    ...(capability.preparation && resources ? { preparation: preparationOf(resources, family, entry.name) } : {}),
    ...(capability.selection !== 'none' && resources ? { selected: selectedOf(resources, family, entry.name) } : {}),
    diagnostics: entry.diagnostic ? [entry.diagnostic] : [],
    capability,
  }))];
}

/** The identities this exact scope authors in the family's own resource
 * document, for the two families this protocol can author at all. Native
 * validity is a verdict of the resource inspection, so it is reported here
 * only when the document itself failed to parse. */
function authoredDefinitions(source: SourceSettings | undefined, scope: SourceScope, family: ResourceFamily):
readonly { name: string; path: string; valid?: boolean; diagnostic?: string }[] {
  if (!source) return [];
  if (family === 'mcp') {
    const view = scope === 'user' ? source.user_mcp : source.workspace_mcp;
    if (!view?.authored) return [];
    return Object.keys(view.authored).map(name => ({ name, path: view.path }));
  }
  if (family === 'agent') {
    return source.agents.filter(entry => entry.scope === scope).map(entry => ({
      name: entry.name, path: entry.source.path,
      ...(entry.source.diagnostic ? { valid: false, diagnostic: entry.source.diagnostic } : {}),
    }));
  }
  return [];
}

/** Every resource identity of every family this Extensions surface manages. */
export function allExtensionEntries(source: SourceSettings | undefined, scope: SourceScope): readonly ExtensionEntry[] {
  return resourceFamilies.flatMap(family => extensionEntries(source, scope, family));
}

export function relationshipLabel(tx: Translate, entry: ExtensionEntry, scope: SourceScope): string {
  return entry.relationship === 'authored' ? (scope === 'workspace' ? tx('settings:copy.workspace-definition') : tx('settings:copy.user-definition'))
    : entry.relationship === 'overriding' ? tx('settings:copy.overrides-the-inherited-definition')
      : entry.relationship === 'inherited' ? tx('settings:copy.inherited-from-user-no-override-in-this-workspace')
        : tx('settings:copy.shadowed-by-the-workspace-definition');
}

/** What a Workflow's observation is called: native admits a Workflow program,
 * and prepares an MCP or Managed Python source. */
export function preparationTitle(tx: Translate, entry: ExtensionEntry): string {
  return entry.family === 'workflow' ? tx('settings:copy.admission') : tx('settings:copy.preparation');
}

export function preparationLabel(tx: Translate, entry: ExtensionEntry): string | undefined {
  if (!entry.capability.preparation) return undefined;
  // Native published no observation for this identity, so its status is
  // unknown. Unknown is never rendered as a negative status.
  if (entry.preparation === undefined) return tx('settings:copy.value-not-observed', { p0: preparationTitle(tx, entry) });
  switch (entry.preparation) {
    case 'ready': return tx('settings:copy.prepared');
    case 'unprepared': return tx('settings:copy.not-prepared');
    case 'unavailable': return tx('settings:copy.preparation-unavailable');
    case 'enabled': return tx('settings:copy.admitted');
    case 'disabled': return tx('settings:copy.not-admitted');
  }
}

export function selectionLabel(tx: Translate, entry: ExtensionEntry): string | undefined {
  if (entry.capability.selection === 'none') return undefined;
  // Native published no root inspection, so whether the root Agent may use
  // this resource is unknown. Unknown is never rendered as "no".
  if (entry.selected === undefined) return tx('settings:copy.root-selection-not-observed');
  if (entry.family === 'skill') return entry.selected ? tx('settings:copy.visible-to-the-root-agent') : tx('settings:copy.not-visible-to-the-root-agent');
  return entry.selected ? tx('settings:copy.allowed-for-the-root-agent') : tx('settings:copy.not-allowed-for-the-root-agent');
}
