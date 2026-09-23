/* Copyright (c) 2026 DeepSeek. MIT. Adapted model request controls and settings cards; see PROVENANCE.md. */
import { useState } from 'react';
import type {
  Model, ModelLayer, Modality, ProviderView, ProviderWrite, SourceScope, SourceSettings,
} from '../../../../../protocol/app-server/v18';
import { Badge } from '../../../presentation/settings/SettingsContent';
import { Button } from '../../../presentation/primitives/Button';
import { TypedUnitForm, UnitForm, type TypedUnitForm as TypedForm } from '../forms/bridge';
import { Bool, Enum, Numeric, NumericText, RequestParameters, RequestParameterRows, Text } from '../forms/fields';
import { TextField } from '../forms/controls';
import { Advanced, Choice, ResourceList, Search, type ResourceRow } from '../primitives/aria';
import { catalogEntries, provenanceLabel, sourceView, type CatalogEntry } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { observedResult, observedResultLabel, unitApplication } from '../projection';
import css from '../../../presentation/settings/SettingsContent.module.css';
import workflow from '../../../presentation/settings/SettingsWorkflow.module.css';

export interface ModelsPageProps {
  source: SourceSettings; scope: SourceScope; revision: string;
  /** Every model identity this scope can reach, for the selectors. */
  models: string[];
  focus?: PageFocus['models'];
  onFocus: (focus?: PageFocus['models']) => void;
}

const emptyModel = (provider = ''): Model => ({
  provider, id: '', protocol: 'openai_responses', context_window: '128000', max_output_tokens: 8192,
  capabilities: { input_modalities: ['text'], output_modalities: ['text'], tool_calls: true, reasoning: false },
});

function credentialLabel(provider: ProviderView | undefined): string {
  return !provider ? 'Definition not resolved'
    : provider.credential.type === 'environment' ? `Environment: ${provider.credential.variable}` : 'Literal secret (redacted)';
}

/** The Models product page: Providers, their Models, and the default model for
 * new Sessions.
 *
 * The interaction structure — a Provider list that drills into one Provider
 * and from there into one of its Models, with each object's status and actions
 * kept next to that object — follows the hierarchy Z.ai ZCode documents. Its
 * configuration precedence, account and billing concepts are deliberately not
 * adopted: rustX scope authority, native identities and exact-CAS semantic-unit
 * writes are unchanged, and the visual family remains the existing
 * Harness-derived one. */
export function ModelsPage({ source, scope, revision, models, focus, onFocus }: ModelsPageProps) {
  if (focus?.kind === 'provider') return <ProviderDetail source={source} scope={scope} revision={revision} id={focus.id} onFocus={onFocus} />;
  if (focus?.kind === 'model') return <ModelDetail source={source} scope={scope} revision={revision} id={focus.id} provider={focus.provider} onFocus={onFocus} />;
  return <ProviderList source={source} scope={scope} revision={revision} models={models} onFocus={onFocus} />;
}

function ProviderList({ source, scope, revision, models, onFocus }: Omit<ModelsPageProps, 'focus'>) {
  const [query, setQuery] = useState('');
  const [identity, setIdentity] = useState('');
  const providers = catalogEntries<ProviderView>(source, scope, 'providers');
  const catalog = catalogEntries<Model>(source, scope, 'models');
  const matches = providers.filter(entry => entry.id.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  // The one native availability fact about this scope's Providers and Models.
  // It is an application observation of the whole unit, never a per-Provider
  // reachability claim and never the result of a probe this page issued.
  const application = observedResult(unitApplication(source.application, 'provider'));
  const rows: ResourceRow[] = matches.map(entry => {
    const provider = entry.authored ?? entry.effective;
    return {
      id: entry.id, name: entry.id,
      facts: <>
        <Badge>{credentialLabel(provider)}</Badge>
        {scope === 'workspace' && <Badge>{provenanceLabel(entry.origin)}</Badge>}
        {scope === 'workspace' && !entry.authored && <Badge>No override in this Workspace</Badge>}
      </>,
      detail: <p className={css.hint}>{provider?.base_url} · {catalog.filter(model => (model.authored ?? model.effective)?.provider === entry.id).length} model(s)</p>,
      actions: [{ id: 'open', label: entry.authored ? `Edit Provider ${entry.id}` : `Override Provider ${entry.id}`, run: () => onFocus({ kind: 'provider', id: entry.id }) }],
    };
  });
  return <section aria-label="Models">
    <h3>Models</h3>
    <p>Manage the Providers that serve your models, and choose the model new Sessions start from.</p>

    <h4>Providers</h4>
    <p>A configured Provider is an authored definition. It is not evidence that the endpoint is reachable — nothing on this page contacts a Provider, and no model call is made to fill a badge. Credentials are never read back.</p>
    {scope === 'workspace' && <p>Identities this Workspace does not override are listed from the native effective projection. Opening one authors nothing.</p>}
    <p role="status">Native Providers &amp; Models application for this source: <strong>{observedResultLabel(application)}</strong>{application.state === 'failed' && <> — {application.diagnostic}</>}</p>
    <div className={workflow.toolbar}>
      <Search label="Find a Provider" value={query} onChange={setQuery} placeholder="Find by identity" />
    </div>
    <ResourceList label="Providers" rows={rows} onOpen={id => onFocus({ kind: 'provider', id })}
      empty={query ? `No Provider identity matches ${query}.` : 'No Provider is defined for this source yet.'} />
    <div className={css.actions}>
      <TextField label="New Provider identity" value={identity} change={setIdentity} />
      <Button disabled={!identity || providers.some(entry => entry.id === identity)}
        onClick={() => { onFocus({ kind: 'provider', id: identity }); setIdentity(''); }}>Add Provider</Button>
    </div>

    <Advanced title={`All Models (${catalog.length})`}>
      <p className={css.hint}>Every Model identity this source reaches, including any whose Provider identity is not defined here.</p>
      <ResourceList label="All Models" rows={catalog.map(entry => modelRow(entry, scope))} onOpen={id => onFocus({ kind: 'model', id })}
        empty="No Model is defined for this source yet." />
      <NewModel exists={catalog.map(entry => entry.id)} open={id => onFocus({ kind: 'model', id })} />
    </Advanced>

    <h4>Default model for new Sessions</h4>
    <UnitForm<ModelLayer> title="Default model" authored={sourceView(source, scope)?.authored?.agent?.model ?? undefined}
      blank={{}} revision={revision} mutation={authored => ({ kind: 'config', mutation: { unit: 'root_model', authored } })}
      removalNotice={<p>New Sessions fall back to the native default model once no source authors one.</p>}>
      {(value, change) => <>
        <p className={css.hint}>This is the model new Sessions start from. It is not the model an existing Session is already using: a Session keeps the model it was started or explicitly switched to, and saving here sends no Session mutation.</p>
        <ModelSelection value={value} change={change} models={models} />
      </>}
    </UnitForm>
  </section>;
}

function modelRow(entry: CatalogEntry<Model>, scope: SourceScope): ResourceRow {
  const model = entry.authored ?? entry.effective;
  return {
    id: entry.id, name: entry.id,
    facts: <>
      <Badge>{model?.protocol}</Badge>
      {scope === 'workspace' && <Badge>{provenanceLabel(entry.origin)}</Badge>}
      {scope === 'workspace' && !entry.authored && <Badge>No override in this Workspace</Badge>}
    </>,
    detail: <p className={css.hint}>Provider {model?.provider} · wire identity {model?.id} · {model?.context_window} context · {model?.max_output_tokens} output</p>,
  };
}

function NewModel({ exists, open, provider }: { exists: readonly string[]; open: (id: string) => void; provider?: string }) {
  const [identity, setIdentity] = useState('');
  return <div className={css.actions}>
    <TextField label="New Model identity" value={identity} change={setIdentity} />
    <Button disabled={!identity || exists.includes(identity)} onClick={() => { open(identity); setIdentity(''); }}>
      {provider ? `Add Model to ${provider}` : 'Add Model'}
    </Button>
  </div>;
}

/** One Provider: its connection fields, its credential, and its Models. */
function ProviderDetail({ source, scope, revision, id, onFocus }: {
  source: SourceSettings; scope: SourceScope; revision: string; id: string; onFocus: (focus?: PageFocus['models']) => void;
}) {
  const authored = sourceView(source, scope)?.authored ?? {};
  const providers = catalogEntries<ProviderView>(source, scope, 'providers');
  const catalog = catalogEntries<Model>(source, scope, 'models');
  const authoredProvider = authored.providers?.[id];
  // A Provider this scope authors is reconstructed from its own redacted view.
  // A credential is never inherited from a shadowed definition, so the native
  // effective Provider is deliberately not adapted into the editor; it is
  // reported as the redacted native fact it is.
  const inherited = !authoredProvider ? providers.find(entry => entry.id === id)?.effective : undefined;
  const owned = catalog.filter(entry => (entry.authored ?? entry.effective)?.provider === id);
  return <section aria-label={`Provider ${id}`} className={workflow.detail}>
    <div className={workflow.breadcrumb}><Button size="sm" onClick={() => onFocus(undefined)}>← Models</Button><span>Provider {id}</span></div>
    <h3>Provider {id}</h3>
    <p>Replace this scope's complete Provider definition. Rust validates and commits the authored unit; unrelated authored fields are preserved because the whole object is replaced.</p>
    {inherited && <p>Native effective Provider {id}: {inherited.base_url} · {credentialLabel(inherited)}. An override authors a complete new definition here; the inherited credential is never copied or read back.</p>}
    <TypedUnitForm<ProviderWrite> key={`provider:${id}`} title={`Provider ${id}`}
      authored={authoredProvider && { base_url: authoredProvider.base_url, credential: { kind: 'retain' } }}
      blank={{ base_url: '', credential: { kind: 'environment', variable: '' } }}
      inherited={() => undefined} revision={revision}
      removalNotice={<p>Models that name this Provider identity are not changed, and no Session is rewritten. Native resolution simply stops finding a Provider under this identity for this source.</p>}
      mutation={value => ({ kind: 'config', mutation: { unit: 'provider', id, authored: value } })}>
      {form => <ProviderFields form={form} authored={!!authoredProvider} />}
    </TypedUnitForm>
    <h4>Models served by {id}</h4>
    <ResourceList label={`Models of Provider ${id}`} rows={owned.map(entry => modelRow(entry, scope))}
      onOpen={modelId => onFocus({ kind: 'model', id: modelId, provider: id })}
      empty={`No Model in this source names Provider ${id}.`} />
    <NewModel provider={id} exists={catalog.map(entry => entry.id)} open={modelId => onFocus({ kind: 'model', id: modelId, provider: id })} />
  </section>;
}

function ProviderFields({ form, authored }: { form: TypedForm<ProviderWrite>; authored: boolean }) {
  const Subscribe = form.Subscribe as unknown as (props: { selector: (state: { values: ProviderWrite }) => string; children: (kind: string) => React.ReactNode }) => React.ReactNode;
  return <>
    <Text form={form} name="base_url" label="Endpoint" required url />
    <Enum form={form} name="credential.kind" label="Credential source"
      options={[...(authored ? [['retain', "Keep the credential this scope already authored"] as const] : []), ['environment', 'Read it from an environment variable'] as const, ['literal', 'Enter a literal secret'] as const]} />
    <Subscribe selector={state => state.values.credential.kind}>{kind => <>
      {kind === 'environment' && <Text form={form} name="credential.variable" label="Environment variable" required />}
      {kind === 'literal' && <LiteralCredential form={form} />}
    </>}</Subscribe>
    <p className={css.hint}>Credentials are never inherited from a shadowed Provider or read back as resolved values. A literal secret stays in page memory until it is saved, and is dropped once native confirms the write.</p>
  </>;
}
function LiteralCredential({ form }: { form: TypedForm<ProviderWrite> }) {
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <Field name="credential.value">{field => <label>New literal credential
    <input type="password" autoComplete="new-password" spellCheck={false} required value={(field.state.value as string | undefined) ?? ''}
      onChange={event => field.handleChange(event.target.value as never)} />
  </label>}</Field>;
}

/** One Model's complete typed contract.
 *
 * Every field native models is authored here, and the mutation replaces the
 * complete object. Editing the context window therefore cannot drop reasoning
 * profiles, request parameters, compatibility settings or capabilities: they
 * are all part of the one value the transaction owns. */
function ModelDetail({ source, scope, revision, id, provider, onFocus }: {
  source: SourceSettings; scope: SourceScope; revision: string; id: string; provider?: string;
  onFocus: (focus?: PageFocus['models']) => void;
}) {
  const authored = sourceView(source, scope)?.authored?.models?.[id];
  return <section aria-label={`Model ${id}`} className={workflow.detail}>
    <div className={workflow.breadcrumb}>
      <Button size="sm" onClick={() => onFocus(provider ? { kind: 'provider', id: provider } : undefined)}>← {provider ? `Provider ${provider}` : 'Models'}</Button>
      <span>Model {id}</span>
    </div>
    <h3>Model {id}</h3>
    <p>Replace this scope's complete Model definition. Unspecified fields use native defaults within that object.</p>
    <TypedUnitForm<Model> key={`model:${id}`} title={`Model ${id}`} authored={authored ?? undefined}
      blank={emptyModel(provider)} revision={revision}
      removalNotice={<p>Any source that names this Model identity keeps naming it; native resolution simply stops finding a definition for it in this source. No Session is rewritten.</p>}
      mutation={value => ({ kind: 'config', mutation: { unit: 'model', id, authored: value } })}>
      {form => <ModelFields form={form} />}
    </TypedUnitForm>
  </section>;
}

const modalities: readonly Modality[] = ['text', 'image', 'file'];
function ModelFields({ form }: { form: TypedForm<Model> }) {
  return <>
    <div className={css.grid}>
      <Text form={form} name="id" label="Wire model identity" required />
      <Text form={form} name="provider" label="Provider identity" required />
      <Enum form={form} name="protocol" label="Protocol"
        options={[['openai_chat_completions', 'openai_chat_completions'], ['openai_responses', 'openai_responses'], ['anthropic_messages', 'anthropic_messages']]} />
      <NumericText form={form} name="context_window" label="Context window" />
      <Numeric form={form} name="max_output_tokens" label="Maximum output tokens" />
    </div>
    <fieldset><legend>Explicit capabilities</legend>
      <Bool form={form} name="capabilities.tool_calls" label="tool_calls" />
      <Bool form={form} name="capabilities.reasoning" label="reasoning" />
      <ModalitySet form={form} name="capabilities.input_modalities" label="input_modalities" />
      <ModalitySet form={form} name="capabilities.output_modalities" label="output_modalities" />
    </fieldset>
    <Advanced title="Reasoning profiles"><ReasoningProfiles form={form} /></Advanced>
    <Advanced title="Request defaults and protocol compatibility">
      <RequestParameters form={form} name="request_params" />
      <Enum form={form} name="compat.chat_reasoning_replay" label="Chat reasoning replay" empty="Unspecified"
        options={[['omit', 'omit'], ['reasoning_content', 'reasoning_content'], ['reasoning', 'reasoning']]} />
      <Enum form={form} name="compat.chat_max_tokens_field" label="Chat output field" empty="Unspecified"
        options={[['max_tokens', 'max_tokens'], ['max_completion_tokens', 'max_completion_tokens']]} />
      <Enum form={form} name="compat.chat_stream_usage" label="Chat stream usage" empty="Unspecified"
        options={[['supported', 'supported'], ['unsupported', 'unsupported']]} />
      <Enum form={form} name="compat.chat_tool_protocol" label="Chat tool protocol" empty="Unspecified"
        options={[['native', 'native'], ['qwen_xml', 'qwen_xml']]} />
      <Enum form={form} name="compat.responses_storage" label="Responses storage" empty="Unspecified"
        options={[['stateless', 'stateless'], ['stored', 'stored']]} />
    </Advanced>
  </>;
}

function ModalitySet({ form, name, label }: { form: TypedForm<Model>; name: 'capabilities.input_modalities' | 'capabilities.output_modalities'; label: string }) {
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <Field name={name}>{field => {
    const value = (field.state.value as Modality[] | undefined) ?? [];
    return <div>{label}{modalities.map(modality => <label key={modality}>
      <input type="checkbox" checked={value.includes(modality)}
        onChange={event => field.handleChange((event.target.checked ? [...value, modality] : value.filter(item => item !== modality)) as never)} />{modality}
    </label>)}</div>;
  }}</Field>;
}

interface Profile { enabled: boolean; request_params?: Record<string, unknown> }
function ReasoningProfiles({ form }: { form: TypedForm<Model> }) {
  const [identity, setIdentity] = useState('');
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <Field name="reasoning">{field => {
    const reasoning = field.state.value as { default_profile: string; profiles: Record<string, Profile> } | null | undefined;
    const profiles = reasoning?.profiles ?? {};
    const set = (next: { default_profile: string; profiles: Record<string, Profile> } | null) => field.handleChange(next as never);
    return <>
      <label>Default profile<input value={reasoning?.default_profile ?? ''}
        onChange={event => set({ default_profile: event.target.value, profiles })} /></label>
      {Object.entries(profiles).map(([profileId, profile]) => <div key={profileId}>
        <label><input type="checkbox" checked={profile.enabled}
          onChange={event => set({ default_profile: reasoning!.default_profile, profiles: { ...profiles, [profileId]: { ...profile, enabled: event.target.checked } } })} />{profileId}</label>
        <RequestParameterRows value={profile.request_params ?? {}}
          change={request_params => set({ default_profile: reasoning!.default_profile, profiles: { ...profiles, [profileId]: { ...profile, request_params } } })} />
        <Button onClick={() => { const next = { ...profiles }; delete next[profileId]; set({ default_profile: reasoning!.default_profile, profiles: next }); }}>Delete profile {profileId}</Button>
      </div>)}
      <div className={css.actions}>
        <label>New reasoning profile<input value={identity} onChange={event => setIdentity(event.target.value)} /></label>
        <Button disabled={!identity || identity in profiles} onClick={() => {
          set({ default_profile: reasoning?.default_profile ?? identity, profiles: { ...profiles, [identity]: { enabled: true, request_params: {} } } });
          setIdentity('');
        }}>Add profile</Button>
      </div>
      {reasoning && <Button onClick={() => set(null)}>Remove reasoning profiles</Button>}
    </>;
  }}</Field>;
}

/** The model-selection fields shared by the default model and a named Agent's
 * explicit child model. Both author a `ModelLayer`, so both reach exactly the
 * same controls and the same identity-discovery rule. */
export function ModelSelection({ value, change, models }: { value: ModelLayer; change: (next: ModelLayer) => void; models: string[] }) {
  const summary = value.summary_model;
  const identities = (extra?: string) => [...new Set([...models, ...(extra ? [extra] : [])])];
  return <>
    <Choice label="Model" value={value.model ?? ''} options={[['', 'Select model'], ...identities(value.model ?? undefined).map(id => [id, id] as const)]}
      onChange={model => change({ ...value, model: model || null })} />
    <ModelRequestFields value={value} change={change} />
    <Choice label="Summary model" value={summary?.mode === 'explicit' ? summary.model : ''}
      options={[['', 'Follow selected model'], ...identities(summary?.mode === 'explicit' ? summary.model : undefined).map(id => [id, id] as const)]}
      onChange={model => change({ ...value, summary_model: model ? { ...(summary?.mode === 'explicit' ? summary : {}), mode: 'explicit', model } : { mode: 'session' } })} />
    {summary?.mode === 'explicit' && <fieldset><legend>Explicit Summary Model settings</legend>
      <ModelRequestFields value={summary} suffix=" (Summary)" change={summary_model => change({ ...value, summary_model })} />
    </fieldset>}
  </>;
}
type RequestFields = Pick<ModelLayer, 'reasoning_profile' | 'max_output_tokens' | 'request_params'>;
/** The labels are suffixed rather than prefixed so that no control's name is a
 * suffix of another's: the Summary model's fields and the outer model's fields
 * stay unambiguously distinguishable, for a reader and for a test alike. */
function ModelRequestFields<T extends RequestFields>({ value, change, suffix = '' }: { value: T; change: (next: T) => void; suffix?: string }) {
  return <>
    <Choice label={`Reasoning profile${suffix}`} value={value.reasoning_profile?.mode ?? ''}
      options={[['', 'Domain default'], ['catalog_default', 'Catalog default'], ['profile', 'Named profile']]}
      onChange={mode => change({ ...value, reasoning_profile: mode === 'profile' ? { mode: 'profile', name: '' } : mode === 'catalog_default' ? { mode: 'catalog_default' } : null })} />
    {value.reasoning_profile?.mode === 'profile' && <TextField label={`Profile identity${suffix}`} required value={value.reasoning_profile.name}
      change={name => change({ ...value, reasoning_profile: { mode: 'profile', name } })} />}
    <label>Output limit{suffix}<input type="number" min="1" value={value.max_output_tokens?.mode === 'limit' ? value.max_output_tokens.tokens : ''}
      onChange={event => change({ ...value, max_output_tokens: event.target.value ? { mode: 'limit', tokens: Number(event.target.value) } : { mode: 'catalog_default' } })} /></label>
    <RequestParameterRows value={value.request_params ?? {}} change={request_params => change({ ...value, request_params })} />
  </>;
}
