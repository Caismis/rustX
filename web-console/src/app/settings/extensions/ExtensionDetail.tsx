import { useTranslation } from '../../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource editor controls and inventory cards; see PROVENANCE.md. */
import type {
  AgentProfileDocument, AgentSkillSelection, McpWrite, ResourceFamily,
  SourceScope, SourceSettings, SourceToolSelection,
} from '../../../../../protocol/app-server/v23';
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
  const tx = useTranslation();
  const { source, scope, family, name, onFocus } = props;
  const entry = family === 'native' ? undefined : extensionEntries(source, scope, family as ResourceFamily).find(item => item.name === name);
  const preparation = entry && preparationLabel(tx, entry);
  const selection = entry && selectionLabel(tx, entry);
  return <section aria-label={tx('settings:extension-detail.value-value', { p0: extensionFamilyLabel(tx, family), p1: name })} className={workflow.detail}>
    <div className={workflow.breadcrumb}><Button size="sm" onClick={() => onFocus(undefined)}>{tx('settings:extension-detail.extensions')}</Button><span>{extensionFamilyLabel(tx, family)} {name}</span></div>
    <h3>{name}</h3>
    {entry ? <>
      <div className={workflow.rowFacts}>
        <Badge>{extensionFamilyLabel(tx, family)}</Badge>
        <Badge>{entry.owner === 'user' ? tx('settings:extension-detail.user') : tx('settings:extension-detail.workspace')}</Badge>
        <Badge>{relationshipLabel(tx, entry, scope)}</Badge>
        <Badge tone={entry.valid === undefined ? undefined : entry.valid ? 'success' : 'error'}>{validityLabel(tx, entry)}</Badge>
        {preparation && <Badge>{preparation}</Badge>}
        {selection && <Badge>{selection}</Badge>}
      </div>
      <Facts rows={[
        [tx('settings:copy.source'), entry.path],
        ...(entry.shadowed ? [[tx('settings:extensions-page.shadows'), entry.shadowed] as const] : []),
        ...(preparation ? [[preparationTitle(tx, entry), preparation] as const] : []),
        ...(selection ? [[tx('settings:copy.root-agent'), selection] as const] : []),
      ]} />
      {entry.diagnostics.map((reason, index) => <p className={css.error} key={index}>{reason}</p>)}
    </> : <p className={css.hint}>{tx('settings:extension-detail.this-identity-is-not-present-in-the-current-native-inventory-not')}</p>}

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
  const tx = useTranslation();
  const catalog = scope === 'user' ? source.user_mcp : source.workspace_mcp;
  // The MCP document is its own native authority: `rustx.toml` being malformed
  // says nothing about it, and it being malformed says nothing about any other
  // document. Native parses it before every MCP mutation and offers no repair
  // mutation for it, so a document that does not parse admits no editing here.
  const mcp = documentAuthoring(catalog);
  if (mcp.state === 'unavailable') return <p role="alert" className={css.error}>{tx('settings:extension-detail.workspace-source-authority-is-unavailable')}</p>;
  if (mcp.state === 'malformed') return <section aria-label={tx('settings:extension-detail.mcp-definition')}><h4>{tx('settings:extension-detail.mcp-definition')}</h4>
    <p>{mcp.path}</p><p role="alert" className={css.error}>{mcp.diagnostic ?? tx('settings:source.not-loaded')}</p>
    <p role="status">{tx('settings:extension-detail.mcp-editing-is-unavailable-because-this-document-does-not-parse')}</p></section>;
  // Presence of an identity is the parsed document's own fact: native closes
  // the whole document above when it does not parse, so a present MCP identity
  // always has an authored value.
  const authored = mcp.document[name];
  return <section aria-label={tx('settings:extension-detail.mcp-definition')}><h4>{tx('settings:extension-detail.mcp-definition')}</h4>
    <p>{tx('settings:extension-detail.a-definition-is-inert-it-is-prepared-and-connected-only-once-som')}</p>
    <p className={css.hint}>{mcp.path}</p>
    <TypedUnitForm<McpWrite> key={`mcp:${name}`} title={tx('settings:extension-detail.mcp-value', { p0: name })} revision={mcp.revision} authored={authored}
      blank={{ definition: { type: 'stdio', command: '', args: [] }, retained_env: [], retained_headers: [] }}
      removalNotice={<p>{tx('settings:extension-detail.nothing-that-is-already-running-is-stopped-by-this-and-no-sessio')}</p>}
      mutation={value => ({ kind: 'mcp', id: name, authored: value })}>
      {form => <McpFields form={form} />}
    </TypedUnitForm>
  </section>;
}

function McpFields({ form }: { form: import('../forms/bridge').TypedUnitForm<McpWrite> }) {
  const tx = useTranslation();
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
    <Choice label={tx('settings:extension-detail.transport')} value={transport} options={[['stdio', 'stdio'], ['http', 'HTTP']]}
      onChange={next => { if (next !== transport) setTransport(next); }} />
    {transport === 'http'
      ? <Text form={form} name="definition.url" label={tx('settings:extension-detail.mcp-url')} required url />
      : <>
        <Text form={form} name="definition.command" label={tx('settings:extension-detail.mcp-command')} required />
        <Strings form={form} name="definition.args" label={tx('settings:extension-detail.arguments')} />
        <Text form={form} name="definition.cwd" label={tx('settings:extension-detail.working-directory')} />
      </>}
    <Entries form={form} name="definition.sensitive_env" label={tx('settings:extension-detail.environment-references-variable')} />
    {transport === 'http' && <Entries form={form} name="definition.sensitive_headers" label={tx('settings:extension-detail.header-references-variable')} />}
    {/* Literal environment values and headers are secrets on the same terms as
        a Provider credential: memory-only while being authored, never read
        back from native, and dropped once the commit is confirmed. */}
    <Entries form={form} name="definition.env" label={tx('settings:extension-detail.literal-environment')} secret />
    {transport === 'http' && <Entries form={form} name="definition.headers" label={tx('settings:extension-detail.literal-headers')} secret />}
    <Strings form={form} name="retained_env" label={tx('settings:extension-detail.retain-existing-environment-keys')} />
    <Strings form={form} name="retained_headers" label={tx('settings:extension-detail.retain-existing-header-keys')} />
  </>}</Subscribe>;
}

/** One named Agent's complete profile document.
 *
 * Each named Agent is its own file, so whether this scope defines it and what
 * native parsed it into are two facts: a file that does not parse is still this
 * scope's definition — winning, and shadowing any same-name User one — and it
 * is replaced or removed on its own revision. */
function AgentDefinition({ source, scope, name, models }: ExtensionDetailProps) {
  const tx = useTranslation();
  const current = source.agents.find(agent => agent.scope === scope && agent.name === name);
  return <section aria-label={tx('settings:extension-detail.agent-definition')}><h4>{tx('settings:extension-detail.agent-definition')}</h4>
    <p>{tx('settings:extension-detail.each-named-agent-is-an-independent-complete-profile-a-workspace')}</p>
    {current?.source.diagnostic && <p role="alert" className={css.error}>{current.source.diagnostic}</p>}
    <TypedUnitForm<AgentProfileDocument> key={`agent:${name}`} title={tx('settings:extension-detail.agent-value', { p0: name })}
      authoredPresent={current !== undefined} authored={current?.source.authored ?? undefined} blank={{}}
      revision={current?.source.revision ?? source.absent_resource_revision}
      removalNotice={<p>{tx('settings:extension-detail.the-root-agent-s-delegation-allowlist-is-a-separate-unit-and-is')}</p>}
      mutation={value => ({ kind: 'agent', name, authored: value })}>
      {form => <AgentFields form={form} models={models} />}
    </TypedUnitForm>
  </section>;
}

function AgentFields({ form, models }: { form: import('../forms/bridge').TypedUnitForm<AgentProfileDocument>; models: string[] }) {
  const tx = useTranslation();
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <>
    <Text form={form} name="description" label={tx('settings:extension-detail.description')} />
    <label>{tx('settings:agent-page.instructions')}<Field name="instructions">{field => <textarea value={(field.state.value as string | undefined) ?? ''}
      onChange={event => field.handleChange(event.target.value as never)} />}</Field></label>
    <Field name="model">{field => {
      const model = field.state.value as AgentProfileDocument['model'];
      return <>
        <label><input type="checkbox" checked={!!model} onChange={event => field.handleChange((event.target.checked ? {} : null) as never)} />{tx('settings:extension-detail.explicit-child-model')}</label>
        {model ? <ModelSelection value={model} change={next => field.handleChange(next as never)} models={models} />
          : <p>{tx('settings:extension-detail.inherit-the-invoking-attempt-s-already-frozen-effective-model')}</p>}
      </>;
    }}</Field>
    <Field name="tools">{field => {
      const tools = (field.state.value as AgentProfileDocument['tools']) ?? {};
      return <>
        <CheckboxList label={tx('settings:extension-detail.native-tools')} values={nativeTools} selected={tools.builtin ?? []}
          change={builtin => field.handleChange({ ...tools, builtin } as never)} />
        <SourceSelections value={tools.sources ?? {}} change={sources => field.handleChange({ ...tools, sources } as never)} />
      </>;
    }}</Field>
    <Field name="agents">{field => <Names label={tx('settings:extension-detail.delegated-agents')} value={(field.state.value as string[] | undefined) ?? []} change={value => field.handleChange(value as never)} />}</Field>
    <Field name="workflows">{field => <Names label={tx('settings:extension-detail.delegated-workflows')} value={(field.state.value as string[] | undefined) ?? []} change={value => field.handleChange(value as never)} />}</Field>
    <p>{tx('settings:extension-detail.native-scope-validation-decides-which-capabilities-this-profile')}</p>
    <Field name="skills">{field => <Selection label={tx('settings:extension-detail.skill-prompt-visibility')} value={(field.state.value as AgentSkillSelection | undefined) ?? []} change={value => field.handleChange(value as never)} />}</Field>
    <Advanced title={tx('settings:extension-detail.extensions-worktree-and-guidance')}>
      <Field name="plugins">{field => {
        const plugins = (field.state.value as AgentProfileDocument['plugins']) ?? {};
        return <fieldset><legend>{tx('settings:extension-detail.extensions-default-off')}</legend>
          {(['todo', 'goal'] as const).map(id => <label key={id}>
            <input type="checkbox" checked={plugins[id]?.enabled ?? false}
              onChange={event => field.handleChange({ ...plugins, [id]: { enabled: event.target.checked } } as never)} />{id}
          </label>)}
          <label><input type="checkbox" checked={plugins.agent_status?.enabled ?? false}
            onChange={event => field.handleChange({ ...plugins, agent_status: { ...plugins.agent_status, enabled: event.target.checked } } as never)} />{tx('settings:extension-detail.agent-status')}</label>
        </fieldset>;
      }}</Field>
      <Field name="timeout_ms">{field => <label>{tx('settings:extension-detail.child-timeout-ms')}<input type="number" min="1"
        value={(field.state.value as string | null | undefined) ?? ''}
        onChange={event => field.handleChange((event.target.value || null) as never)} /></label>}</Field>
      <Field name="worktree">{field => {
        const worktree = (field.state.value as AgentProfileDocument['worktree']) ?? {};
        return <>
          <label><input type="checkbox" checked={worktree.enabled ?? false} onChange={event => field.handleChange({ ...worktree, enabled: event.target.checked } as never)} />{tx('settings:extension-detail.use-child-worktree')}</label>
          <label><input type="checkbox" checked={worktree.require_clean_parent ?? true} onChange={event => field.handleChange({ ...worktree, require_clean_parent: event.target.checked } as never)} />{tx('settings:extension-detail.require-clean-parent')}</label>
        </>;
      }}</Field>
      <Field name="agents_md">{field => {
        const guidance = (field.state.value as AgentProfileDocument['agents_md']) ?? {};
        return <>
          <label><input type="checkbox" checked={guidance.inherit ?? true} onChange={event => field.handleChange({ ...guidance, inherit: event.target.checked } as never)} />{tx('settings:agent-page.include-project-guidance')}</label>
          <Names label={tx('settings:extension-detail.project-guidance-files')} value={guidance.files ?? []} change={files => field.handleChange({ ...guidance, files } as never)} />
        </>;
      }}</Field>
    </Advanced>
  </>;
}

function SourceSelections({ value, change }: { value: Record<string, SourceToolSelection>; change: (value: Record<string, SourceToolSelection>) => void }) {
  const tx = useTranslation();
  return <>
    <p>{tx('settings:extension-detail.mcp-and-managed-python-definitions-do-not-activate-tools-select')}</p>
    {Object.entries(value).map(([id, selection]) => <div key={id}>
      <Selection label={id} value={selection} change={next => change({ ...value, [id]: next })} />
      <Button onClick={() => { const next = { ...value }; delete next[id]; change(next); }}>{tx('settings:extension-detail.remove-source-selection')}{' '}{id}</Button>
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
  const tx = useTranslation();
  const resources = source.prospective_resources;
  const label = extensionFamilyLabel(tx, family);
  const inspection = family === 'workflow' ? resources?.workflows[name]
    : family === 'managed_python' ? resources?.sources[toolSourceId('managed_python', name)] : undefined;
  const skill = family === 'skill' ? resources?.skills.find(entry => entry.name === name) : undefined;
  return <section aria-label={tx('settings:extension-detail.value-definition', { p0: label })}><h4>{label} {tx('settings:extension-detail.definition')}</h4>
    <p role="status">{tx('settings:extension-detail.this-protocol-has-no-operation-that-authors-a')}{' '}{label} {tx('settings:extension-detail.definition-this-detail-shows-the-native-inventory-its-diagnostic')}</p>
    {family === 'skill' && <p>{tx('settings:extension-detail.skill-selection-controls-which-skill-descriptions-are-advertised')}</p>}
    {skill && <>
      <Facts rows={[[tx('settings:copy.effective-package'), skill.location], [tx('settings:copy.owning-source'), skill.source]]} />
      {!!skill.shadowed.length && <Advanced title={tx('settings:extension-detail.shadowed-same-identity-packages-value', { p0: skill.shadowed.length })}><NativeFacts value={skill.shadowed} /></Advanced>}
    </>}
    {inspection && <Advanced title={tx('settings:extension-detail.native-inspection')} expanded><NativeFacts value={inspection} /></Advanced>}
    {/* Skill discovery diagnostics name packages, not this Skill, so they are
        the Skills family's and are listed on the Extensions Skills filter —
        never here, where another package's failure would read as this one's. */}
    {family === 'workflow' && resources?.workflows[name]?.status === 'disabled'
      && <Advanced title={tx('settings:extension-detail.workflow-admission-diagnostics')} expanded><NativeFacts value={resources.workflows[name]} /></Advanced>}
  </section>;
}

/** Whether the root/default Agent may use this resource.
 *
 * This is a `rustx.toml` semantic unit and an entirely separate native
 * mutation from the definition above. It is edited here through exactly the
 * same unit identity as the primary Tools & Permissions control, so both
 * surfaces share one draft, one pinned CAS base and one settlement. */
function RootAvailability({ source, scope, revision, family, name }: ExtensionDetailProps) {
  const tx = useTranslation();
  if (family === 'native') return null;
  if (revision === undefined) return <section aria-label={tx('settings:extension-detail.root-agent-availability')}><h4>{tx('settings:extension-detail.availability-to-the-root-agent')}</h4>
    <ConfigUnavailable /></section>;
  const document = (scope === 'user' ? source.user : source.workspace)?.authored;
  const agent = document?.agent;
  if (family === 'mcp' || family === 'managed_python') {
    const id = toolSourceId(family as ResourceFamily, name);
    return <section aria-label={tx('settings:extension-detail.root-agent-availability')}><h4>{tx('settings:extension-detail.availability-to-the-root-agent')}</h4>
      <p>{tx('settings:extension-detail.saved-separately-from-the-definition-granting-access-and-definin')}</p>
      <UnitForm<SourceToolSelection> key={`source_tools:${id}`} title={tx('settings:extension-detail.source-value', { p0: id })}
        authored={agent?.tools?.sources?.[id] ?? undefined} blank={[]} revision={revision}
        mutation={authored => ({ kind: 'config', mutation: { unit: 'source_tools', id, authored } })}>
        {(value, change) => <Selection label={id} value={value} change={change} />}
      </UnitForm>
    </section>;
  }
  if (family === 'agent' || family === 'workflow') {
    const unit = family === 'agent' ? 'agents' as const : 'workflows' as const;
    return <section aria-label={tx('settings:extension-detail.root-agent-availability')}><h4>{tx('settings:extension-detail.availability-to-the-root-agent')}</h4>
      <p>{tx('settings:extension-detail.this-edits-the-root-agent-s')}{' '}{unit === 'agents' ? tx('settings:agent-page.agent') : tx('settings:extension-detail.workflow')} {tx('settings:extension-detail.allowlist-the-same-unit-as-the-one-on-tools-amp-permissions-shar')}</p>
      <UnitForm<string[]> key={unit} title={unit === 'agents' ? tx('settings:extension-detail.agent-allowlist') : tx('settings:extension-detail.workflow-allowlist')}
        authored={agent?.[unit] ?? undefined} blank={[]} revision={revision}
        mutation={authored => ({ kind: 'config', mutation: { unit, authored } })}>
        {(value, change) => <>
          <MembershipToggle name={name} value={value} change={change} />
          <Names label={unit} value={value} change={change} />
        </>}
      </UnitForm>
    </section>;
  }
  return <section aria-label={tx('settings:extension-detail.root-agent-availability')}><h4>{tx('settings:extension-detail.availability-to-the-root-agent')}</h4>
    <p>{tx('settings:extension-detail.this-edits-the-root-agent-s-skill-visibility-the-same-unit-as-th')}</p>
    <UnitForm<AgentSkillSelection> key="skills" title={tx('settings:extension-detail.skill-visibility')} authored={agent?.skills ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'skills', authored } })}>
      {(value, change) => <>
        {value === 'all'
          ? <p role="status">{tx('settings:extension-detail.every-skill-is-visible-because-this-source-authors-the-explicit')}{' '}<code>{tx('settings:extension-detail.all')}</code> {tx('settings:extension-detail.selection-changing-that-for-one-skill-means-choosing-exact-ident')}</p>
          : <MembershipToggle name={name} value={value} change={change} />}
        <Selection label={tx('settings:extension-detail.visible-skills')} value={value} change={change} />
      </>}
    </UnitForm>
  </section>;
}

/** Add or remove exactly this identity from a list-valued unit, leaving every
 * other identity in the list untouched. */
function MembershipToggle({ name, value, change }: { name: string; value: string[]; change: (value: string[]) => void }) {
  const tx = useTranslation();
  const member = value.includes(name);
  return <label><input type="checkbox" checked={member}
    onChange={event => change(event.target.checked ? [...value, name] : value.filter(item => item !== name))} />{tx('settings:extension-detail.include')}{' '}{name}</label>;
}

/** Shown in place of an editor for a `rustx.toml` semantic unit while that
 * document does not parse. Native admits only its repair, so no other
 * mutation of it is offered here. */
export function ConfigUnavailable() {
  const tx = useTranslation();
  return <p role="status">{tx('settings:extension-detail.structured-editing-of-this-source-s-rustx-toml-is-unavailable-be')}</p>;
}
