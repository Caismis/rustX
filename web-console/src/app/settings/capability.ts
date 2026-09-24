import type { ResourceFamily } from '../../../../protocol/app-server/v21';
import type { ExtensionFamily } from './projection';

/** What the native App Server actually lets a client do to one resource family.
 *
 * This is derived from the generated protocol, not from the shape the pages
 * would like to have. At App Server v21 the complete set of source-authoring
 * operations is `SourceMutation`:
 *
 * ```text
 * repair_config   one malformed `rustx.toml` document
 * config          one `ConfigMutation` semantic unit of `rustx.toml`
 * mcp             one complete MCP server definition, by identity
 * agent           one complete named Agent profile document, by identity
 * ```
 *
 * There is no Workflow-program, Skill-package or Managed Python source write on
 * the wire at all — not a restricted one, not a differently named one. Those
 * families are therefore inventory, diagnostics and root selection only, and
 * this module is what stops a card from rendering an Edit, Save or Delete that
 * no native operation could ever accept.
 *
 * The resulting matrix, with the exact native operation behind each cell:
 *
 * ```text
 * family      inventory  authoring                       root selection unit
 * ---------------------------------------------------------------------------
 * provider    resolved   config/provider                 (not a selection)
 * model       resolved   config/model                    config/root_model
 * mcp         native     mcp                             config/source_tools
 * agent       native     agent                           config/agents
 * skill       native     none                            config/skills
 * workflow    native     none                            config/workflows
 * python      native     none                            config/source_tools
 * native      n/a        config/todo|goal|agent_status   (it is the config)
 * ```
 *
 * `inventory` distinguishes where identities come from: `resolved` means the
 * native resolved `RuntimeLayer` names them, `native` means the native
 * `CapabilityInspection` resource inventory does.
 *
 * Authoring and selection are deliberately two columns, because they are two
 * native mutations with two settlements. Saving an MCP definition never grants
 * it to the root Agent, and granting it never authors a definition. */
export interface ResourceCapability {
  readonly family: ExtensionFamily;
  /** Native names every identity of this family without any client discovery. */
  readonly inventory: 'resolved' | 'native' | 'config';
  /** The native operation that authors a complete definition, or `none` when
   * this protocol has no write for the family at all. A card must not offer
   * Edit, Save or Delete when this is `none`. */
  readonly authoring: 'config' | 'mcp' | 'agent' | 'none';
  /** The `rustx.toml` semantic unit that makes this family available to the
   * root/default Agent, or `none` when the family has no selection concept.
   * It is always an independent mutation from `authoring`. */
  readonly selection: 'source_tools' | 'skills' | 'agents' | 'workflows' | 'none';
  /** Native reports a preparation/runtime status for identities of this family. */
  readonly preparation: boolean;
  /** Native reports per-identity diagnostics for this family. */
  readonly diagnostics: boolean;
}

const capabilities: Readonly<Record<ExtensionFamily, ResourceCapability>> = {
  // A complete MCP server definition is replaced by `SourceMutation::mcp`;
  // `SourceInspection` carries its preparation status and its Tools.
  mcp: { family: 'mcp', inventory: 'native', authoring: 'mcp', selection: 'source_tools', preparation: true, diagnostics: true },
  // A named Agent profile is replaced by `SourceMutation::agent`. Its
  // availability to the root Agent is the independent `agents` allowlist.
  agent: { family: 'agent', inventory: 'native', authoring: 'agent', selection: 'agents', preparation: false, diagnostics: true },
  // Skill packages are discovered from the native Skill roots. No protocol
  // operation authors one; `skills` selects which are visible in the prompt.
  skill: { family: 'skill', inventory: 'native', authoring: 'none', selection: 'skills', preparation: false, diagnostics: true },
  // Workflow programs live in the native resource source. `WorkflowInspection`
  // carries admission status and diagnostics; no operation authors a program.
  workflow: { family: 'workflow', inventory: 'native', authoring: 'none', selection: 'workflows', preparation: true, diagnostics: true },
  // Managed Python sources are prepared natively and selected per source, on
  // the same `source_tools` terms as MCP. No operation authors a package.
  managed_python: { family: 'managed_python', inventory: 'native', authoring: 'none', selection: 'source_tools', preparation: true, diagnostics: true },
  // Native extensions are not resource documents: each one *is* a `rustx.toml`
  // semantic unit, so authoring it and enabling it are the same mutation.
  native: { family: 'native', inventory: 'config', authoring: 'config', selection: 'none', preparation: false, diagnostics: false },
};

export function resourceCapability(family: ExtensionFamily): ResourceCapability {
  return capabilities[family];
}

/** Whether this exact scope may author a complete definition of the family.
 *
 * Scope is a native authority question, so it is answered once here. Every
 * authorable family is authorable at both scopes — a Workspace definition
 * shadows the whole same-name User one — and a family with no native write is
 * authorable at neither. */
export function admitsAuthoring(family: ExtensionFamily): boolean {
  return resourceCapability(family).authoring !== 'none';
}

/** The native `source_tools` identity of one resource, which is the Tool-source
 * identity native itself uses: a Managed Python package is addressed as
 * `python:<name>`, every other source by its bare identity. */
export function toolSourceId(family: ResourceFamily, name: string): string {
  return family === 'managed_python' ? `python:${name}` : name;
}

/** The closed set of native extension units, which are configuration semantic
 * units rather than resource documents. */
export type NativeExtension = 'todo' | 'goal' | 'agent_status';
export const nativeExtensions: readonly NativeExtension[] = ['todo', 'goal', 'agent_status'];
export function nativeExtensionLabel(extension: NativeExtension): string {
  return extension === 'todo' ? 'Todo' : extension === 'goal' ? 'Goal' : 'Agent Status';
}

/** The User-only semantic units. App Server process policy is assigned from the
 * User document by native composition and records no Workspace origin, so a
 * Workspace surface must not offer it at all. */
export function userOnlyUnit(unit: string): boolean {
  return unit === 'app_server';
}
