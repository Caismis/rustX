import type { Translate } from '../../../locale/translation';
import { useTranslation } from '../../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted model request controls and settings cards; see PROVENANCE.md. */
import { useRef, useState } from 'react';
import { GridList, GridListItem, Button as AriaButton } from 'react-aria-components';
import type {
  Model, ModelLayer, Modality, ProviderView, ProviderWrite, SourceScope, SourceSettings,
} from '../../../../../protocol/app-server/v23';
import { Badge } from '../../../presentation/settings/SettingsContent';
import { Button } from '../../../presentation/primitives/Button';
import { TypedUnitForm, UnitForm, useUnitEditing, type TypedUnitForm as TypedForm } from '../forms/bridge';
import { Bool, Enum, Numeric, NumericText, RequestParameters, RequestParameterRows, Text } from '../forms/fields';
import { TextField } from '../forms/controls';
import { Advanced, Choice, ConfirmAction, ResourceList, Search, type ResourceRow } from '../primitives/aria';
import { catalogEntries, provenanceLabel, sourceView, type CatalogEntry } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { observedResult, observedResultLabel, unitApplication } from '../projection';
import css from '../../../presentation/settings/SettingsContent.module.css';
import cards from '../../../presentation/settings/ModelsCards.module.css';
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

function credentialLabel(tx: Translate, provider: ProviderView | undefined): string {
  return !provider ? tx('settings:copy.definition-not-resolved')
    : provider.credential.type === 'environment' ? tx('settings:copy.environment-value', { p0: provider.credential.variable }) : tx('settings:copy.literal-secret-redacted');
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
  const tx = useTranslation();
  const landing = useRef<HTMLElement>(null);
  const [query, setQuery] = useState('');
  const [identity, setIdentity] = useState('');
  const providers = catalogEntries<ProviderView>(source, scope, 'providers');
  const catalog = catalogEntries<Model>(source, scope, 'models');
  const matches = providers.filter(entry => entry.id.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  // The one native availability fact about this scope's Providers and Models.
  // It is an application observation of the whole unit, never a per-Provider
  // reachability claim and never the result of a probe this page issued.
  const application = observedResult(unitApplication(source.application, 'provider'));
  return <section ref={landing} tabIndex={-1} aria-label={tx('settings:models-page.models')} className={cards.section}>
    <h3 className={cards.title}>{tx('settings:models-page.models')}</h3>
    <p className={cards.intro} role="status">{tx('settings:models.status', { status: observedResultLabel(tx, application) })}{application.state === 'failed' && <> — {application.diagnostic}</>}</p>
    <Search label={tx('settings:models-page.find-a-provider')} value={query} onChange={setQuery} placeholder={tx('settings:extensions-page.find-by-identity')} />
    <GridList className={cards.rows} aria-label={tx('settings:models-page.providers')} onAction={id => onFocus({ kind: 'provider', id: String(id) })}>{matches.map(entry => <GridListItem id={entry.id} key={entry.id} textValue={entry.id} aria-label={entry.id} className={cards.rowCard}>
      <ProviderCard entry={entry} revision={revision} open={() => onFocus({ kind: 'provider', id: entry.id })} settle={landing}/>
    </GridListItem>)}</GridList>
    {!matches.length && <p className={cards.intro}>{query ? tx('settings:models-page.no-provider-identity-matches-value', { p0: query }) : tx('settings:models-page.no-provider-is-defined-for-this-source-yet')}</p>}
    <Advanced title={tx('settings:models-page.new-provider')}>
      <div className={css.actions}><TextField label={tx('settings:models-page.new-provider-identity')} value={identity} change={setIdentity} />
        <Button disabled={!identity || providers.some(entry => entry.id === identity)} onClick={() => { onFocus({ kind: 'provider', id: identity }); setIdentity(''); }}>{tx('settings:models-page.add-provider')}</Button>
      </div>
    </Advanced>

    <Advanced title={tx('settings:models-page.all-models-value', { p0: catalog.length })}>
      <p className={css.hint}>{tx('settings:models-page.every-model-identity-this-source-reaches-including-any-whose-pro')}</p>
      <ResourceList label={tx('settings:models-page.all-models')} rows={catalog.map(entry => modelRow(tx, entry, scope))} onOpen={id => onFocus({ kind: 'model', id })}
        empty={tx('settings:copy.no-model-is-defined-for-this-source-yet')} />
      <NewModel exists={catalog.map(entry => entry.id)} open={id => onFocus({ kind: 'model', id })} />
    </Advanced>

    <Advanced title={tx('settings:models-page.default-model-for-new-sessions')}>
    <UnitForm<ModelLayer> title={tx('settings:models-page.default-model')} authored={sourceView(source, scope)?.authored?.agent?.model ?? undefined}
      blank={{}} revision={revision} mutation={authored => ({ kind: 'config', mutation: { unit: 'root_model', authored } })}
      removalNotice={<p>{tx('settings:models-page.new-sessions-fall-back-to-the-native-default-model-once-no-sourc')}</p>}>
      {(value, change) => <>
        <p className={css.hint}>{tx('settings:models-page.this-is-the-model-new-sessions-start-from-it-is-not-the-model-an')}</p>
        <ModelSelection value={value} change={change} models={models} />
      </>}
    </UnitForm>
    </Advanced>
  </section>;
}

function ProviderCard({ entry, revision, open, settle }: { entry: CatalogEntry<ProviderView>; revision: string; open: () => void; settle: React.RefObject<HTMLElement | null> }) {
  const tx = useTranslation();
  const authored = entry.authored;
  const unit = useUnitEditing<ProviderWrite>({ authored: authored ? { base_url: authored.base_url, credential: { kind: 'retain' } } : undefined,
    blank: { base_url: '', credential: { kind: 'environment', variable: '' } }, revision,
    mutation: value => ({ kind: 'config', mutation: { unit: 'provider', id: entry.id, authored: value } }) });
  return <><div className={cards.rowHead}>
    <div className={cards.rowIdentity}><span className={cards.rowName} title={entry.id}>{entry.id}</span><span className={cards.rowTag}>{authored ? tx('settings:models-page.configured') : provenanceLabel(tx, entry.origin)}</span></div>
    <div className={cards.rowActions}><AriaButton className={cards.secondaryButton} aria-label={tx('settings:models-page.value-provider-value', { p0: authored ? tx('settings:models-page.edit') : tx('settings:bridge.override'), p1: entry.id })} onPress={open}>{authored ? tx('settings:models-page.edit') : tx('settings:models-page.details')}</AriaButton>
      {authored && <ConfirmAction label={tx('settings:models-page.delete-provider-value', { p0: entry.id })} triggerText={tx('settings:copy.delete')} title={tx('settings:models-page.delete-provider-value-2', { p0: entry.id })} description={tx('settings:models-page.remove-this-source-s-provider-definition-model-definitions-are-k')} confirm={tx('settings:models-page.delete-provider-value', { p0: entry.id })} tone="destructive" settle={settle} disabled={!unit.admitted || unit.busy || unit.reviewNeeded} onConfirm={() => unit.submit(true)}/>}</div>
    </div>
    {unit.outcome.kind === 'conflict' && <small role="alert">{tx('settings:models-page.source-changed-open-details-to-review-the-preserved-revision')}</small>}
    {unit.outcome.kind === 'uncertain' && <small role="alert">{tx('settings:models-page.outcome-uncertain-open-details-and-reread-no-replay')}</small>}
    {unit.outcome.kind === 'rejected' && <small role="alert">{unit.outcome.detail}</small>}
    {unit.awaitingObservation && <small role="status">{tx('settings:models-page.saved-awaiting-observation')}</small>}
  </>;
}

function modelRow(tx: Translate, entry: CatalogEntry<Model>, scope: SourceScope): ResourceRow {
  const model = entry.authored ?? entry.effective;
  return {
    id: entry.id, name: entry.id,
    facts: <>
      <Badge>{model?.protocol}</Badge>
      {scope === 'workspace' && <Badge>{provenanceLabel(tx, entry.origin)}</Badge>}
      {scope === 'workspace' && !entry.authored && <Badge>{tx('settings:models-page.no-override-in-this-workspace')}</Badge>}
    </>,
    detail: <p className={css.hint}>{tx('settings:models-page.provider')}{' '}{model?.provider}{' '}{tx('settings:models-page.wire-identity')}{' '}{model?.id} · {model?.context_window}{' '}{tx('settings:models-page.context')}{' '}{model?.max_output_tokens}{' '}{tx('settings:models-page.output')}</p>,
  };
}

function NewModel({ exists, open, provider }: { exists: readonly string[]; open: (id: string) => void; provider?: string }) {
  const tx = useTranslation();
  const [identity, setIdentity] = useState('');
  return <div className={css.actions}>
    <TextField label={tx('settings:models-page.new-model-identity')} value={identity} change={setIdentity} />
    <Button disabled={!identity || exists.includes(identity)} onClick={() => { open(identity); setIdentity(''); }}>
      {provider ? tx('settings:models-page.add-model-to-value', { p0: provider }) : tx('settings:models-page.add-model')}
    </Button>
  </div>;
}

/** One Provider: its connection fields, its credential, and its Models. */
function ProviderDetail({ source, scope, revision, id, onFocus }: {
  source: SourceSettings; scope: SourceScope; revision: string; id: string; onFocus: (focus?: PageFocus['models']) => void;
}) {
  const tx = useTranslation();
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
  return <section aria-label={tx('settings:models-page.provider-value', { p0: id })} className={workflow.detail}>
    <div className={workflow.breadcrumb}><Button size="sm" onClick={() => onFocus(undefined)}>{tx('settings:models-page.models-2')}</Button><span>{tx('settings:models-page.provider')}{' '}{id}</span></div>
    <h3>{tx('settings:models-page.provider')}{' '}{id}</h3>
    <p>{tx('settings:models-page.replace-this-scope-s-complete-provider-definition-rust-validates')}</p>
    {inherited && <p>{tx('settings:models-page.native-effective-provider')}{' '}{id}: {inherited.base_url} · {credentialLabel(tx, inherited)}{tx('settings:models-page.an-override-authors-a-complete-new-definition-here-the-inherited')}</p>}
    <TypedUnitForm<ProviderWrite> key={`provider:${id}`} title={tx('settings:models-page.provider-value', { p0: id })}
      authored={authoredProvider && { base_url: authoredProvider.base_url, credential: { kind: 'retain' } }}
      blank={{ base_url: '', credential: { kind: 'environment', variable: '' } }}
      inherited={() => undefined} revision={revision}
      removalNotice={<p>{tx('settings:models-page.models-that-name-this-provider-identity-are-not-changed-and-no-s')}</p>}
      mutation={value => ({ kind: 'config', mutation: { unit: 'provider', id, authored: value } })}>
      {form => <ProviderFields form={form} authored={!!authoredProvider} />}
    </TypedUnitForm>
    <h4>{tx('settings:models-page.models-served-by')}{' '}{id}</h4>
    <ResourceList label={tx('settings:models-page.models-of-provider-value', { p0: id })} rows={owned.map(entry => modelRow(tx, entry, scope))}
      onOpen={modelId => onFocus({ kind: 'model', id: modelId, provider: id })}
      empty={tx('settings:copy.no-model-in-this-source-names-provider-value', { p0: id })} />
    <NewModel provider={id} exists={catalog.map(entry => entry.id)} open={modelId => onFocus({ kind: 'model', id: modelId, provider: id })} />
  </section>;
}

function ProviderFields({ form, authored }: { form: TypedForm<ProviderWrite>; authored: boolean }) {
  const tx = useTranslation();
  const Subscribe = form.Subscribe as unknown as (props: { selector: (state: { values: ProviderWrite }) => string; children: (kind: string) => React.ReactNode }) => React.ReactNode;
  return <>
    <Text form={form} name="base_url" label={tx('settings:models-page.endpoint')} required url />
    <Enum form={form} name="credential.kind" label={tx('settings:models-page.credential-source')}
      options={[...(authored ? [['retain', tx('settings:copy.keep-the-credential-this-scope-already-authored')] as const] : []), ['environment', tx('settings:copy.read-it-from-an-environment-variable')] as const, ['literal', tx('settings:copy.enter-a-literal-secret')] as const]} />
    <Subscribe selector={state => state.values.credential.kind}>{kind => <>
      {kind === 'environment' && <Text form={form} name="credential.variable" label={tx('settings:models-page.environment-variable')} required />}
      {kind === 'literal' && <LiteralCredential form={form} />}
    </>}</Subscribe>
    <p className={css.hint}>{tx('settings:models-page.credentials-are-never-inherited-from-a-shadowed-provider-or-read')}</p>
  </>;
}
function LiteralCredential({ form }: { form: TypedForm<ProviderWrite> }) {
  const tx = useTranslation();
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <Field name="credential.value">{field => <label>{tx('settings:models-page.new-literal-credential')}<input type="password" autoComplete="new-password" spellCheck={false} required value={(field.state.value as string | undefined) ?? ''}
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
  const tx = useTranslation();
  const authored = sourceView(source, scope)?.authored?.models?.[id];
  return <section aria-label={tx('settings:models-page.model-value', { p0: id })} className={workflow.detail}>
    <div className={workflow.breadcrumb}>
      <Button size="sm" onClick={() => onFocus(provider ? { kind: 'provider', id: provider } : undefined)}>← {provider ? tx('settings:models-page.provider-value', { p0: provider }) : tx('settings:models-page.models')}</Button>
      <span>{tx('settings:models-page.model')}{' '}{id}</span>
    </div>
    <h3>{tx('settings:models-page.model')}{' '}{id}</h3>
    <p>{tx('settings:models-page.replace-this-scope-s-complete-model-definition-unspecified-field')}</p>
    <TypedUnitForm<Model> key={`model:${id}`} title={tx('settings:models-page.model-value', { p0: id })} authored={authored ?? undefined}
      blank={emptyModel(provider)} revision={revision}
      removalNotice={<p>{tx('settings:models-page.any-source-that-names-this-model-identity-keeps-naming-it-native')}</p>}
      mutation={value => ({ kind: 'config', mutation: { unit: 'model', id, authored: value } })}>
      {form => <ModelFields form={form} />}
    </TypedUnitForm>
  </section>;
}

const modalities: readonly Modality[] = ['text', 'image', 'file'];
function ModelFields({ form }: { form: TypedForm<Model> }) {
  const tx = useTranslation();
  return <>
    <div className={css.grid}>
      <Text form={form} name="id" label={tx('settings:models-page.wire-model-identity')} required />
      <Text form={form} name="provider" label={tx('settings:models-page.provider-identity')} required />
      <Enum form={form} name="protocol" label={tx('settings:models-page.protocol')}
        options={[['openai_chat_completions', 'openai_chat_completions'], ['openai_responses', 'openai_responses'], ['anthropic_messages', 'anthropic_messages']]} />
      <NumericText form={form} name="context_window" label={tx('settings:models-page.context-window')} />
      <Numeric form={form} name="max_output_tokens" label={tx('settings:models-page.maximum-output-tokens')} />
    </div>
    <fieldset><legend>{tx('settings:models-page.explicit-capabilities')}</legend>
      <Bool form={form} name="capabilities.tool_calls" label={tx('settings:models-page.tool-calls')} />
      <Bool form={form} name="capabilities.reasoning" label={tx('settings:models-page.reasoning')} />
      <ModalitySet form={form} name="capabilities.input_modalities" label={tx('settings:models-page.input-modalities')} />
      <ModalitySet form={form} name="capabilities.output_modalities" label={tx('settings:models-page.output-modalities')} />
    </fieldset>
    <Advanced title={tx('settings:models-page.reasoning-profiles')}><ReasoningProfiles form={form} /></Advanced>
    <Advanced title={tx('settings:models-page.request-defaults-and-protocol-compatibility')}>
      <RequestParameters form={form} name="request_params" />
      <Enum form={form} name="compat.chat_reasoning_replay" label={tx('settings:models-page.chat-reasoning-replay')} empty={tx('settings:copy.unspecified')}
        options={[['omit', 'omit'], ['reasoning_content', 'reasoning_content'], ['reasoning', 'reasoning']]} />
      <Enum form={form} name="compat.chat_max_tokens_field" label={tx('settings:models-page.chat-output-field')} empty={tx('settings:copy.unspecified')}
        options={[['max_tokens', 'max_tokens'], ['max_completion_tokens', 'max_completion_tokens']]} />
      <Enum form={form} name="compat.chat_stream_usage" label={tx('settings:models-page.chat-stream-usage')} empty={tx('settings:copy.unspecified')}
        options={[['supported', 'supported'], ['unsupported', 'unsupported']]} />
      <Enum form={form} name="compat.chat_tool_protocol" label={tx('settings:models-page.chat-tool-protocol')} empty={tx('settings:copy.unspecified')}
        options={[['native', 'native'], ['qwen_xml', 'qwen_xml']]} />
      <Enum form={form} name="compat.responses_storage" label={tx('settings:models-page.responses-storage')} empty={tx('settings:copy.unspecified')}
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
  const tx = useTranslation();
  const [identity, setIdentity] = useState('');
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <Field name="reasoning">{field => {
    const reasoning = field.state.value as { default_profile: string; profiles: Record<string, Profile> } | null | undefined;
    const profiles = reasoning?.profiles ?? {};
    const set = (next: { default_profile: string; profiles: Record<string, Profile> } | null) => field.handleChange(next as never);
    return <>
      <label>{tx('settings:models-page.default-profile')}<input value={reasoning?.default_profile ?? ''}
        onChange={event => set({ default_profile: event.target.value, profiles })} /></label>
      {Object.entries(profiles).map(([profileId, profile]) => <div key={profileId}>
        <label><input type="checkbox" checked={profile.enabled}
          onChange={event => set({ default_profile: reasoning!.default_profile, profiles: { ...profiles, [profileId]: { ...profile, enabled: event.target.checked } } })} />{profileId}</label>
        <RequestParameterRows value={profile.request_params ?? {}}
          change={request_params => set({ default_profile: reasoning!.default_profile, profiles: { ...profiles, [profileId]: { ...profile, request_params } } })} />
        <Button onClick={() => { const next = { ...profiles }; delete next[profileId]; set({ default_profile: reasoning!.default_profile, profiles: next }); }}>{tx('settings:models-page.delete-profile')}{' '}{profileId}</Button>
      </div>)}
      <div className={css.actions}>
        <label>{tx('settings:models-page.new-reasoning-profile')}<input value={identity} onChange={event => setIdentity(event.target.value)} /></label>
        <Button disabled={!identity || identity in profiles} onClick={() => {
          set({ default_profile: reasoning?.default_profile ?? identity, profiles: { ...profiles, [identity]: { enabled: true, request_params: {} } } });
          setIdentity('');
        }}>{tx('settings:models-page.add-profile')}</Button>
      </div>
      {reasoning && <Button onClick={() => set(null)}>{tx('settings:models-page.remove-reasoning-profiles')}</Button>}
    </>;
  }}</Field>;
}

/** The model-selection fields shared by the default model and a named Agent's
 * explicit child model. Both author a `ModelLayer`, so both reach exactly the
 * same controls and the same identity-discovery rule. */
export function ModelSelection({ value, change, models }: { value: ModelLayer; change: (next: ModelLayer) => void; models: string[] }) {
  const tx = useTranslation();
  const summary = value.summary_model;
  const identities = (extra?: string) => [...new Set([...models, ...(extra ? [extra] : [])])];
  return <>
    <Choice label={tx('settings:models-page.model')} value={value.model ?? ''} options={[['', tx('settings:copy.select-model')], ...identities(value.model ?? undefined).map(id => [id, id] as const)]}
      onChange={model => change({ ...value, model: model || null })} />
    <ModelRequestFields value={value} change={change} />
    <Choice label={tx('settings:models-page.summary-model')} value={summary?.mode === 'explicit' ? summary.model : ''}
      options={[['', tx('settings:copy.follow-selected-model')], ...identities(summary?.mode === 'explicit' ? summary.model : undefined).map(id => [id, id] as const)]}
      onChange={model => change({ ...value, summary_model: model ? { ...(summary?.mode === 'explicit' ? summary : {}), mode: 'explicit', model } : { mode: 'session' } })} />
    {summary?.mode === 'explicit' && <fieldset><legend>{tx('settings:models-page.explicit-summary-model-settings')}</legend>
      <ModelRequestFields value={summary} suffix=" (Summary)" change={summary_model => change({ ...value, summary_model })} />
    </fieldset>}
  </>;
}
type RequestFields = Pick<ModelLayer, 'reasoning_profile' | 'max_output_tokens' | 'request_params'>;
/** The labels are suffixed rather than prefixed so that no control's name is a
 * suffix of another's: the Summary model's fields and the outer model's fields
 * stay unambiguously distinguishable, for a reader and for a test alike. */
function ModelRequestFields<T extends RequestFields>({ value, change, suffix = '' }: { value: T; change: (next: T) => void; suffix?: string }) {
  const tx = useTranslation();
  return <>
    <Choice label={tx('settings:models-page.reasoning-profilevalue', { p0: suffix })} value={value.reasoning_profile?.mode ?? ''}
      options={[['', tx('settings:copy.domain-default')], ['catalog_default', tx('settings:copy.catalog-default')], ['profile', tx('settings:copy.named-profile')]]}
      onChange={mode => change({ ...value, reasoning_profile: mode === 'profile' ? { mode: 'profile', name: '' } : mode === 'catalog_default' ? { mode: 'catalog_default' } : null })} />
    {value.reasoning_profile?.mode === 'profile' && <TextField label={tx('settings:models-page.profile-identityvalue', { p0: suffix })} required value={value.reasoning_profile.name}
      change={name => change({ ...value, reasoning_profile: { mode: 'profile', name } })} />}
    <label>{tx('settings:models-page.output-limit')}{suffix}<input type="number" min="1" value={value.max_output_tokens?.mode === 'limit' ? value.max_output_tokens.tokens : ''}
      onChange={event => change({ ...value, max_output_tokens: event.target.value ? { mode: 'limit', tokens: Number(event.target.value) } : { mode: 'catalog_default' } })} /></label>
    <RequestParameterRows value={value.request_params ?? {}} change={request_params => change({ ...value, request_params })} />
  </>;
}
