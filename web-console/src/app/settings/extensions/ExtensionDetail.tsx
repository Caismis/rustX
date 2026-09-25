/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource editor controls and inventory cards; see PROVENANCE.md. */
import type {
  AgentProfileDocument, AgentSkillSelection, McpWrite, ResourceFamily,
  SourceScope, SourceSettings, SourceToolSelection,
} from '../../../../../protocol/app-server/v24';
import { mcpTransport } from '../../../bindings/mcp';
import { Badge, Facts } from '../../../presentation/settings/SettingsContent';
import { Button } from '../../../presentation/primitives/Button';
import { NativeFacts } from '../../components/NativeFacts';
import { toolSourceId } from '../capability';
import { TypedUnitForm, UnitForm } from '../forms/bridge';
import { CheckboxList, Names, Selection, nativeTools } from '../forms/controls';
import { Entries, Strings, Text } from '../forms/fields';
import { Advanced, Choice } from '../primitives/aria';
import { documentAuthoring, extensionFamilyLabel, type ExtensionFamily } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { ModelSelection } from '../models/ModelsPage';
import { extensionEntries, preparationLabel, preparationTitle, relationshipLabel, selectionLabel, validityLabel } from './inventory';
import css from '../../../presentation/settings/SettingsContent.module.css';
import workflow from '../../../presentation/settings/SettingsWorkflow.module.css';

export interface ExtensionDetailProps {
  source: SourceSettings; scope: SourceScope; models: string[];
  /** The exact revision of this scope's `rustx.toml`, or `undefined` while it
   * does not parse. Root availability is a unit of that document, so it is
   * editable only while the document is; the definition documents are not. */
  revision?: string;
  family: ExtensionFamily; name: string; onFocus: (focus?: PageFocus['extensions']) => void;
}

/** One extension resource.
 *
 * The page presents two independent things about the resource and never merges
 * them into one transaction:
 *
 * 1. its **definition** — a complete resource document, where native has an
 *    operation that authors one;
 * 2. its **availability to the root Agent** — a `rustx.toml` semantic unit.
 *
 * Each is its own form with its own Save, its own exact CAS base and its own
 * reported outcome, because each is its own native mutation. Saving a
 * definition never grants it, and granting it never authors a definition, so
 * no acknowledgement of one is ever presented as success of the other.
 *
 * The availability section edits exactly the same native unit as the primary
 * Tools & Permissions control, through the same transaction identity — so
 * there is one draft, one CAS base and one settlement, not a second cache. */
export function ExtensionDetail(props: ExtensionDetailProps) {
  const { source, scope, family, name, onFocus } = props;
  const entry = family === 'native' ? undefined : extensionEntries(source, scope, family as ResourceFamily).find(item => item.name === name);
  const preparation = entry && preparationLabel(entry);
  const selection = entry && selectionLabel(entry);
  return <section aria-label={`${extensionFamilyLabel(family)} ${name}`} className={workflow.detail}>
    <div className={workflow.breadcrumb}><Button size="sm" onClick={() => onFocus(undefined)}>← Extensions</Button><span>{extensionFamilyLabel(family)} {name}</span></div>
    <h3>{name}</h3>
    {entry ? <>
      <div className={workflow.rowFacts}>
        <Badge>{extensionFamilyLabel(family)}</Badge>
        <Badge>{entry.owner === 'user' ? 'User' : 'Workspace'}</Badge>
        <Badge>{relationshipLabel(entry, scope)}</Badge>
        <Badge tone={entry.valid === undefined ? undefined : entry.valid ? 'success' : 'error'}>{validityLabel(entry)}</Badge>
        {preparation && <Badge>{preparation}</Badge>}
        {selection && <Badge>{selection}</Badge>}
      </div>
      <Facts rows={[
        ['Source', entry.path],
        ...(entry.shadowed ? [['Shadows', entry.shadowed] as const] : []),
        ...(preparation ? [[preparationTitle(entry), preparation] as const] : []),
        ...(selection ? [['Root Agent', selection] as const] : []),
      ]} />
      {entry.diagnostics.map((reason, index) => <p className={css.error} key={index}>{reason}</p>)}
    </> : <p className={css.hint}>This identity is not present in the current native inventory. Nothing has been authored for it yet.</p>}

    {family === 'mcp' && <McpDefinition {...props} />}
    {family === 'agent' && <AgentDefinition {...props} />}
    {(family === 'skill' || family === 'workflow' || family === 'managed_python') && <UnauthorableResource {...props} />}
    <RootAvailability {...props} />
  </section>;
}

/** The MCP server definition of exactly one authoring scope.
 *
 * An MCP identity is owned as a whole definition — a Workspace definition
 * shadows the entire same-name User one — so there is no value to merge and no
 * effective document to reconstruct. An inherited identity authors nothing
 * until an explicit override replaces the whole definition. */
function McpDefinition({ source, scope, name }: ExtensionDetailProps) {
  const catalog = scope === 'user' ? source.user_mcp : source.workspace_mcp;
  // The MCP document is its own native authority: `rustx.toml` being malformed
  // says nothing about it, and it being malformed says nothing about any other
  // document. Native parses it before every MCP mutation and offers no repair
  // mutation for it, so a document that does not parse admits no editing here.
  const mcp = documentAuthoring(catalog);
  if (mcp.state === 'unavailable') return <p role="alert" className={css.error}>Workspace source authority is unavailable.</p>;
  if (mcp.state === 'malformed') return <section aria-label="MCP definition"><h4>MCP definition</h4>
    <p>{mcp.path}</p><p role="alert" className={css.error}>{mcp.diagnostic}</p>
    <p role="status">MCP editing is unavailable because this document does not parse. Correct the file, then rescan configuration files on Advanced.</p></section>;
  // Presence of an identity is the parsed document's own fact: native closes
  // the whole document above when it does not parse, so a present MCP identity
  // always has an authored value.
  const authored = mcp.document[name];
  return <section aria-label="MCP definition"><h4>MCP definition</h4>
    <p>A definition is inert. It is prepared and connected only once something selects it, and this section never connects to it.</p>
    <p className={css.hint}>{mcp.path}</p>
    <TypedUnitForm<McpWrite> key={`mcp:${name}`} title={`MCP ${name}`} revision={mcp.revision} authored={authored}
      blank={{ definition: { type: 'stdio', command: '', args: [] }, retained_env: [], retained_headers: [] }}
      removalNotice={<p>Nothing that is already running is stopped by this, and no Session is rewritten. Native resolution stops finding a definition under this identity for this source.</p>}
      mutation={value => ({ kind: 'mcp', id: name, authored: value })}>
      {form => <McpFields form={form} />}
    </TypedUnitForm>
  </section>;
}

function McpFields({ form }: { form: import('../forms/bridge').TypedUnitForm<McpWrite> }) {
  const Subscribe = form.Subscribe as unknown as (props: { selector: (state: { values: McpWrite }) => string; children: (transport: string) => React.ReactNode }) => React.ReactNode;
  // Choosing a transport replaces the whole definition, exactly as native
  // owns it: an HTTP definition has no command and a stdio one has no URL, and
  // the retained secret-key lists belong to the definition being replaced.
  const setTransport = (next: string) => {
    form.setFieldValue('definition' as never, (next === 'http'
      ? { type: 'http', url: '' } : { type: 'stdio', command: '', args: [] }) as never);
    form.setFieldValue('retained_env' as never, [] as never);
    form.setFieldValue('retained_headers' as never, [] as never);
  };
  return <Subscribe selector={state => mcpTransport(state.values.definition)}>{transport => <>
    <Choice label="Transport" value={transport} options={[['stdio', 'stdio'], ['http', 'HTTP']]}
      onChange={next => { if (next !== transport) setTransport(next); }} />
    {transport === 'http'
      ? <Text form={form} name="definition.url" label="MCP URL" required url />
      : <>
        <Text form={form} name="definition.command" label="MCP command" required />
        <Strings form={form} name="definition.args" label="Arguments" />
        <Text form={form} name="definition.cwd" label="Working directory" />
      </>}
    <Entries form={form} name="definition.sensitive_env" label="Environment references ($VARIABLE)" />
    {transport === 'http' && <Entries form={form} name="definition.sensitive_headers" label="Header references ($VARIABLE)" />}
    {/* Literal environment values and headers are secrets on the same terms as
        a Provider credential: memory-only while being authored, never read
        back from native, and dropped once the commit is confirmed. */}
    <Entries form={form} name="definition.env" label="Literal environment" secret />
    {transport === 'http' && <Entries form={form} name="definition.headers" label="Literal headers" secret />}
    <Strings form={form} name="retained_env" label="Retain existing environment keys" />
    <Strings form={form} name="retained_headers" label="Retain existing header keys" />
  </>}</Subscribe>;
}

/** One named Agent's complete profile document.
 *
 * Each named Agent is its own file, so whether this scope defines it and what
 * native parsed it into are two facts: a file that does not parse is still this
 * scope's definition — winning, and shadowing any same-name User one — and it
 * is replaced or removed on its own revision. */
function AgentDefinition({ source, scope, name, models }: ExtensionDetailProps) {
  const current = source.agents.find(agent => agent.scope === scope && agent.name === name);
  return <section aria-label="Agent definition"><h4>Agent definition</h4>
    <p>Each named Agent is an independent complete profile. A Workspace definition replaces the whole same-name User one, invalid definitions included.</p>
    {current?.source.diagnostic && <p role="alert" className={css.error}>{current.source.diagnostic}</p>}
    <TypedUnitForm<AgentProfileDocument> key={`agent:${name}`} title={`Agent ${name}`}
      authoredPresent={current !== undefined} authored={current?.source.authored ?? undefined} blank={{}}
      revision={current?.source.revision ?? source.absent_resource_revision}
      removalNotice={<p>The root Agent's delegation allowlist is a separate unit and is not changed by this. Removing the definition does not remove the name from that allowlist.</p>}
      mutation={value => ({ kind: 'agent', name, authored: value })}>
      {form => <AgentFields form={form} models={models} />}
    </TypedUnitForm>
  </section>;
}

function AgentFields({ form, models }: { form: import('../forms/bridge').TypedUnitForm<AgentProfileDocument>; models: string[] }) {
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <>
    <Text form={form} name="description" label="Description" />
    <label>Instructions<Field name="instructions">{field => <textarea value={(field.state.value as string | undefined) ?? ''}
      onChange={event => field.handleChange(event.target.value as never)} />}</Field></label>
    <Field name="model">{field => {
      const model = field.state.value as AgentProfileDocument['model'];
      return <>
        <label><input type="checkbox" checked={!!model} onChange={event => field.handleChange((event.target.checked ? {} : null) as never)} />Explicit child model</label>
        {model ? <ModelSelection value={model} change={next => field.handleChange(next as never)} models={models} />
          : <p>Inherit the invoking Attempt's already-frozen effective model.</p>}
      </>;
    }}</Field>
    <Field name="tools">{field => {
      const tools = (field.state.value as AgentProfileDocument['tools']) ?? {};
      return <>
        <CheckboxList label="Native Tools" values={nativeTools} selected={tools.builtin ?? []}
          change={builtin => field.handleChange({ ...tools, builtin } as never)} />
        <SourceSelections value={tools.sources ?? {}} change={sources => field.handleChange({ ...tools, sources } as never)} />
      </>;
    }}</Field>
    <Field name="agents">{field => <Names label="Delegated Agents" value={(field.state.value as string[] | undefined) ?? []} change={value => field.handleChange(value as never)} />}</Field>
    <Field name="workflows">{field => <Names label="Delegated Workflows" value={(field.state.value as string[] | undefined) ?? []} change={value => field.handleChange(value as never)} />}</Field>
    <p>Native scope validation decides which capabilities this profile may compose.</p>
    <Field name="skills">{field => <Selection label="Skill prompt visibility" value={(field.state.value as AgentSkillSelection | undefined) ?? []} change={value => field.handleChange(value as never)} />}</Field>
    <Advanced title="Extensions, worktree and guidance">
      <Field name="plugins">{field => {
        const plugins = (field.state.value as AgentProfileDocument['plugins']) ?? {};
        return <fieldset><legend>Extensions · default off</legend>
          {(['todo', 'goal'] as const).map(id => <label key={id}>
            <input type="checkbox" checked={plugins[id]?.enabled ?? false}
              onChange={event => field.handleChange({ ...plugins, [id]: { enabled: event.target.checked } } as never)} />{id}
          </label>)}
          <label><input type="checkbox" checked={plugins.agent_status?.enabled ?? false}
            onChange={event => field.handleChange({ ...plugins, agent_status: { ...plugins.agent_status, enabled: event.target.checked } } as never)} />Agent Status</label>
        </fieldset>;
      }}</Field>
      <Field name="timeout_ms">{field => <label>Child timeout (ms)<input type="number" min="1"
        value={(field.state.value as string | null | undefined) ?? ''}
        onChange={event => field.handleChange((event.target.value || null) as never)} /></label>}</Field>
      <Field name="worktree">{field => {
        const worktree = (field.state.value as AgentProfileDocument['worktree']) ?? {};
        return <>
          <label><input type="checkbox" checked={worktree.enabled ?? false} onChange={event => field.handleChange({ ...worktree, enabled: event.target.checked } as never)} />Use child worktree</label>
          <label><input type="checkbox" checked={worktree.require_clean_parent ?? true} onChange={event => field.handleChange({ ...worktree, require_clean_parent: event.target.checked } as never)} />Require clean parent</label>
        </>;
      }}</Field>
      <Field name="agents_md">{field => {
        const guidance = (field.state.value as AgentProfileDocument['agents_md']) ?? {};
        return <>
          <label><input type="checkbox" checked={guidance.inherit ?? true} onChange={event => field.handleChange({ ...guidance, inherit: event.target.checked } as never)} />Include project guidance</label>
          <Names label="Project guidance files" value={guidance.files ?? []} change={files => field.handleChange({ ...guidance, files } as never)} />
        </>;
      }}</Field>
    </Advanced>
  </>;
}

function SourceSelections({ value, change }: { value: Record<string, SourceToolSelection>; change: (value: Record<string, SourceToolSelection>) => void }) {
  return <>
    <p>MCP and Managed Python definitions do not activate Tools. Select each source explicitly.</p>
    {Object.entries(value).map(([id, selection]) => <div key={id}>
      <Selection label={id} value={selection} change={next => change({ ...value, [id]: next })} />
      <Button onClick={() => { const next = { ...value }; delete next[id]; change(next); }}>Remove source selection {id}</Button>
    </div>)}
  </>;
}

/** A resource family this protocol has no authoring operation for.
 *
 * Inventory, native diagnostics and the supported root selection are all real
 * and all useful. What is deliberately absent is a fabricated Edit, Save or
 * Delete: there is no native operation behind one, so rendering it would be a
 * promise the App Server cannot keep. */
function UnauthorableResource({ source, family, name }: ExtensionDetailProps) {
  const resources = source.prospective_resources;
  const label = extensionFamilyLabel(family);
  const inspection = family === 'workflow' ? resources?.workflows[name]
    : family === 'managed_python' ? resources?.sources[toolSourceId('managed_python', name)] : undefined;
  const skill = family === 'skill' ? resources?.skills.find(entry => entry.name === name) : undefined;
  return <section aria-label={`${label} definition`}><h4>{label} definition</h4>
    <p role="status">This protocol has no operation that authors a {label} definition. This detail shows the native inventory, its diagnostics and the supported root selection; the definition itself is authored in the native resource source.</p>
    {family === 'skill' && <p>Skill selection controls which Skill descriptions are advertised in the prompt. Package paths below are native inspection facts, not an access-control boundary.</p>}
    {skill && <>
      <Facts rows={[['Effective package', skill.location], ['Owning source', skill.source]]} />
      {!!skill.shadowed.length && <Advanced title={`Shadowed same-identity packages (${skill.shadowed.length})`}><NativeFacts value={skill.shadowed} /></Advanced>}
    </>}
    {inspection && <Advanced title="Native inspection" expanded><NativeFacts value={inspection} /></Advanced>}
    {/* Skill discovery diagnostics name packages, not this Skill, so they are
        the Skills family's and are listed on the Extensions Skills filter —
        never here, where another package's failure would read as this one's. */}
    {family === 'workflow' && resources?.workflows[name]?.status === 'disabled'
      && <Advanced title="Workflow admission diagnostics" expanded><NativeFacts value={resources.workflows[name]} /></Advanced>}
  </section>;
}

/** Whether the root/default Agent may use this resource.
 *
 * This is a `rustx.toml` semantic unit and an entirely separate native
 * mutation from the definition above. It is edited here through exactly the
 * same unit identity as the primary Tools & Permissions control, so both
 * surfaces share one draft, one pinned CAS base and one settlement. */
function RootAvailability({ source, scope, revision, family, name }: ExtensionDetailProps) {
  if (family === 'native') return null;
  if (revision === undefined) return <section aria-label="Root Agent availability"><h4>Availability to the root Agent</h4>
    <ConfigUnavailable /></section>;
  const document = (scope === 'user' ? source.user : source.workspace)?.authored;
  const agent = document?.agent;
  if (family === 'mcp' || family === 'managed_python') {
    const id = toolSourceId(family as ResourceFamily, name);
    return <section aria-label="Root Agent availability"><h4>Availability to the root Agent</h4>
      <p>Saved separately from the definition. Granting access and defining the resource are two native mutations with two outcomes.</p>
      <UnitForm<SourceToolSelection> key={`source_tools:${id}`} title={`Source ${id}`}
        authored={agent?.tools?.sources?.[id] ?? undefined} blank={[]} revision={revision}
        mutation={authored => ({ kind: 'config', mutation: { unit: 'source_tools', id, authored } })}>
        {(value, change) => <Selection label={id} value={value} change={change} />}
      </UnitForm>
    </section>;
  }
  if (family === 'agent' || family === 'workflow') {
    const unit = family === 'agent' ? 'agents' as const : 'workflows' as const;
    return <section aria-label="Root Agent availability"><h4>Availability to the root Agent</h4>
      <p>This edits the root Agent's {unit === 'agents' ? 'Agent' : 'Workflow'} allowlist — the same unit as the one on Tools &amp; Permissions, sharing one draft and one exact CAS base. It is saved separately from the definition.</p>
      <UnitForm<string[]> key={unit} title={unit === 'agents' ? 'Agent allowlist' : 'Workflow allowlist'}
        authored={agent?.[unit] ?? undefined} blank={[]} revision={revision}
        mutation={authored => ({ kind: 'config', mutation: { unit, authored } })}>
        {(value, change) => <>
          <MembershipToggle name={name} value={value} change={change} />
          <Names label={unit} value={value} change={change} />
        </>}
      </UnitForm>
    </section>;
  }
  return <section aria-label="Root Agent availability"><h4>Availability to the root Agent</h4>
    <p>This edits the root Agent's Skill visibility — the same unit as the one on Tools &amp; Permissions, sharing one draft and one exact CAS base.</p>
    <UnitForm<AgentSkillSelection> key="skills" title="Skill visibility" authored={agent?.skills ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'skills', authored } })}>
      {(value, change) => <>
        {value === 'all'
          ? <p role="status">Every Skill is visible because this source authors the explicit <code>all</code> selection. Changing that for one Skill means choosing exact identities for all of them.</p>
          : <MembershipToggle name={name} value={value} change={change} />}
        <Selection label="Visible Skills" value={value} change={change} />
      </>}
    </UnitForm>
  </section>;
}

/** Add or remove exactly this identity from a list-valued unit, leaving every
 * other identity in the list untouched. */
function MembershipToggle({ name, value, change }: { name: string; value: string[]; change: (value: string[]) => void }) {
  const member = value.includes(name);
  return <label><input type="checkbox" checked={member}
    onChange={event => change(event.target.checked ? [...value, name] : value.filter(item => item !== name))} />Include {name}</label>;
}

/** Shown in place of an editor for a `rustx.toml` semantic unit while that
 * document does not parse. Native admits only its repair, so no other
 * mutation of it is offered here. */
export function ConfigUnavailable() {
  return <p role="status">Structured editing of this source's rustx.toml is unavailable because it does not parse. Repair it from any other page; resource definitions keep their own documents and stay editable.</p>;
}
