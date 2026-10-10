import type { Translate, TranslationKey } from '../../../locale/translation';
import { useTranslation } from '../../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted model request controls and settings cards; see PROVENANCE.md. */
import { useRef, useState } from 'react';
import { GridList, GridListItem, Button as AriaButton } from 'react-aria-components';
import type {
  Model, ModelLayer, Modality, ProviderView, ProviderWrite, SourceScope, SourceSettings,
} from '../../../../../protocol/app-server/v39';
import { Badge } from '../../../presentation/settings/SettingsContent';
import { Button } from '../../../presentation/primitives/Button';
import { TypedUnitForm, UnitForm, useUnitEditing, type TypedUnitForm as TypedForm } from '../forms/bridge';
import { Bool, Enum, Numeric, NumericText, RequestParameters, RequestParamsEditor, Text } from '../forms/fields';
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
  /** The Model Profiles each reachable model declares, for the selectors. */
  profiles: ModelProfiles;
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
export function ModelsPage({ source, scope, revision, models, profiles, focus, onFocus }: ModelsPageProps) {
  if (focus?.kind === 'provider') return <ProviderDetail source={source} scope={scope} revision={revision} id={focus.id} onFocus={onFocus} />;
  if (focus?.kind === 'model') return <ModelDetail source={source} scope={scope} revision={revision} id={focus.id} provider={focus.provider} onFocus={onFocus} />;
  return <ProviderList source={source} scope={scope} revision={revision} models={models} profiles={profiles} onFocus={onFocus} />;
}

function ProviderList({ source, scope, revision, models, profiles, onFocus }: Omit<ModelsPageProps, 'focus'>) {
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
        <ModelSelection value={value} change={change} models={models} profiles={profiles} />
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
 * complete object. Editing the context window therefore cannot drop Model
 * Profiles, request parameters, compatibility settings or capabilities: they
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
        options={[['openai_chat_completions', /* i18n-raw: exact protocol or wire-field identifier */ 'openai_chat_completions'], ['openai_responses', /* i18n-raw: exact protocol or wire-field identifier */ 'openai_responses'], ['anthropic_messages', /* i18n-raw: exact protocol or wire-field identifier */ 'anthropic_messages']]} />
      <NumericText form={form} name="context_window" label={tx('settings:models-page.context-window')} />
      <Numeric form={form} name="max_output_tokens" label={tx('settings:models-page.maximum-output-tokens')} />
    </div>
    <fieldset><legend>{tx('settings:models-page.explicit-capabilities')}</legend>
      <Bool form={form} name="capabilities.tool_calls" label={tx('settings:models-page.tool-calls')} />
      <Bool form={form} name="capabilities.reasoning" label={tx('settings:models-page.reasoning')} />
      <ModalitySet form={form} name="capabilities.input_modalities" label={tx('settings:models-page.input-modalities')} />
      <ModalitySet form={form} name="capabilities.output_modalities" label={tx('settings:models-page.output-modalities')} />
    </fieldset>
    <Advanced title={tx('settings:models-page.model-profiles')}><Profiles form={form} /></Advanced>
    <Advanced title={tx('settings:models-page.request-defaults-and-protocol-compatibility')}>
      <ModelRequestParameters form={form} />
      <Enum form={form} name="compat.chat_reasoning_replay" label={tx('settings:models-page.chat-reasoning-replay')} empty={tx('settings:copy.unspecified')}
        options={[['omit', tx('settings:model-option.omit')], ['reasoning_content', /* i18n-raw: exact protocol or wire-field identifier */ 'reasoning_content'], ['reasoning', /* i18n-raw: exact protocol or wire-field identifier */ 'reasoning']]} />
      <Enum form={form} name="compat.chat_max_tokens_field" label={tx('settings:models-page.chat-output-field')} empty={tx('settings:copy.unspecified')}
        options={[['max_tokens', /* i18n-raw: exact protocol or wire-field identifier */ 'max_tokens'], ['max_completion_tokens', /* i18n-raw: exact protocol or wire-field identifier */ 'max_completion_tokens']]} />
      <Enum form={form} name="compat.chat_stream_usage" label={tx('settings:models-page.chat-stream-usage')} empty={tx('settings:copy.unspecified')}
        options={[['supported', tx('settings:model-option.supported')], ['unsupported', tx('settings:model-option.unsupported')]]} />
      <Enum form={form} name="compat.chat_tool_protocol" label={tx('settings:models-page.chat-tool-protocol')} empty={tx('settings:copy.unspecified')}
        options={[['native', tx('settings:model-option.native')], ['qwen_xml', /* i18n-raw: exact protocol or wire-field identifier */ 'qwen_xml']]} />
      <Enum form={form} name="compat.responses_storage" label={tx('settings:models-page.responses-storage')} empty={tx('settings:copy.unspecified')}
        options={[['stateless', tx('settings:model-option.stateless')], ['stored', tx('settings:model-option.stored')]]} />
    </Advanced>
  </>;
}

function ModalitySet({ form, name, label }: { form: TypedForm<Model>; name: 'capabilities.input_modalities' | 'capabilities.output_modalities'; label: string }) {
  const tx = useTranslation();
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <Field name={name}>{field => {
    const value = (field.state.value as Modality[] | undefined) ?? [];
    return <div>{label}{modalities.map(modality => <label key={modality}>
      <input type="checkbox" checked={value.includes(modality)}
        onChange={event => field.handleChange((event.target.checked ? [...value, modality] : value.filter(item => item !== modality)) as never)} />{tx(`settings:model-option.${modality}`)}
    </label>)}</div>;
  }}</Field>;
}

type Field = (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
type ProfileMap = NonNullable<Model['profiles']>;

/** A Model's native request parameters, which exist only while the Model
 * declares no profiles: each profile then authors its own complete object. */
function ModelRequestParameters({ form }: { form: TypedForm<Model> }) {
  const tx = useTranslation();
  const Subscribe = form.Subscribe as unknown as (props: { selector: (state: { values: Model }) => boolean; children: (profiled: boolean) => React.ReactNode }) => React.ReactNode;
  return <Subscribe selector={state => Object.keys(state.values.profiles ?? {}).length > 0}>{profiled => profiled
    ? <p className={css.hint}>{tx('settings:models-page.profiles-own-request-parameters')}</p>
    : <RequestParameters form={form} name="request_params" label={tx('settings:models-page.model-request-parameters')} />}</Subscribe>;
}

/** General Model Profiles: named, complete and independent invocation
 * presets. A profile's name carries no meaning; its reasoning state, output
 * default and native parameters are exactly what it declares. Native Rust
 * validates the contract — a declared default, explicit reasoning state on a
 * reasoning-capable Model, budgets within the hard maximum, protected keys. */
function Profiles({ form }: { form: TypedForm<Model> }) {
  const tx = useTranslation();
  const [identity, setIdentity] = useState('');
  const Bound = form.Field as unknown as Field;
  return <Bound name="request_params">{params => <Bound name="default_profile">{fallback => <Bound name="profiles">{field => {
    const profiles = (field.state.value as ProfileMap | null | undefined) ?? {};
    const identities = Object.keys(profiles);
    const setProfiles = (next: ProfileMap) => field.handleChange((Object.keys(next).length ? next : undefined) as never);
    const update = (id: string, profile: ProfileMap[string]) => setProfiles({ ...profiles, [id]: profile });
    return <>
      {identities.length > 0 && <Choice label={tx('settings:models-page.default-profile')} value={(fallback.state.value as string | null | undefined) ?? ''}
        options={[['', tx('settings:copy.unspecified')], ...identities.map(id => [id, id] as const)]}
        onChange={id => fallback.handleChange((id || undefined) as never)} />}
      {identities.map(id => <ProfileFields key={id} id={id} profile={profiles[id]} change={next => update(id, next)} remove={() => {
        const next = { ...profiles }; delete next[id];
        if (fallback.state.value === id) fallback.handleChange((Object.keys(next)[0] ?? undefined) as never);
        setProfiles(next);
      }} />)}
      <div className={css.actions}>
        <TextField label={tx('settings:models-page.new-profile-identity')} value={identity} change={setIdentity} />
        <Button disabled={!identity || identity in profiles} onClick={() => {
          // The first profile starts from the Model's own parameters, which a
          // Model with profiles no longer declares.
          const seed = identities.length ? {} : (params.state.value as ProfileMap[string]['request_params'] | null | undefined) ?? {};
          if (!identities.length) { params.handleChange(undefined as never); fallback.handleChange(identity as never); }
          update(identity, { request_params: seed });
          setIdentity('');
        }}>{tx('settings:models-page.add-profile')}</Button>
      </div>
      {!identities.length && <p className={css.hint}>{tx('settings:models-page.no-profiles')}</p>}
    </>;
  }}</Bound>}</Bound>}</Bound>;
}

function ProfileFields({ id, profile, change, remove }: { id: string; profile: ProfileMap[string]; change: (next: ProfileMap[string]) => void; remove: () => void }) {
  const tx = useTranslation();
  const reasoning = profile.reasoning_enabled;
  return <fieldset data-profile={id}><legend>{tx('settings:models-page.profile-value', { p0: id })}</legend>
    <Choice label={tx('settings:models-page.profile-reasoning')} value={reasoning === undefined || reasoning === null ? '' : String(reasoning)}
      options={[['', tx('settings:copy.unspecified')], ['true', tx('settings:copy.on')], ['false', tx('settings:copy.off')]]}
      onChange={next => change({ ...profile, reasoning_enabled: next === '' ? undefined : next === 'true' })} />
    <label>{tx('settings:models-page.profile-output-default')}<input type="number" min="1" value={profile.max_output_tokens ?? ''}
      onChange={event => change({ ...profile, max_output_tokens: event.target.value ? Number(event.target.value) : undefined })} /></label>
    <RequestParamsEditor label={tx('settings:models-page.profile-request-parameters')} value={profile.request_params ?? {}} optional={false}
      change={request_params => change({ ...profile, request_params: request_params ?? {} })} />
    <Button onClick={remove}>{tx('settings:models-page.delete-profile')}{' '}{id}</Button>
  </fieldset>;
}

/** The Model Profiles each model identity this scope reaches declares. */
export type ModelProfiles = Readonly<Record<string, readonly string[]>>;

/** The model-selection fields shared by the default model and a named Agent's
 * explicit child model. Both author a `ModelLayer`, so both reach exactly the
 * same controls and the same identity-discovery rule. */
export function ModelSelection({ value, change, models, profiles }: { value: ModelLayer; change: (next: ModelLayer) => void; models: string[]; profiles: ModelProfiles }) {
  const tx = useTranslation();
  const summary = value.summary_model;
  const identities = (extra?: string) => [...new Set([...models, ...(extra ? [extra] : [])])];
  return <>
    <Choice label={tx('settings:models-page.model')} value={value.model ?? ''} options={[['', tx('settings:copy.select-model')], ...identities(value.model ?? undefined).map(id => [id, id] as const)]}
      onChange={model => change({ ...value, model: model || null })} />
    <ModelRequestFields value={value} change={change} profiles={profiles[value.model ?? ''] ?? []} />
    <Choice label={tx('settings:models-page.summary-model')} value={summary?.mode === 'explicit' ? summary.model : ''}
      options={[['', tx('settings:copy.follow-selected-model')], ...identities(summary?.mode === 'explicit' ? summary.model : undefined).map(id => [id, id] as const)]}
      onChange={model => change({ ...value, summary_model: model ? { ...(summary?.mode === 'explicit' ? summary : {}), mode: 'explicit', model } : { mode: 'session' } })} />
    {summary?.mode === 'explicit' && <fieldset><legend>{tx('settings:models-page.explicit-summary-model-settings')}</legend>
      <ModelRequestFields value={summary} variant="summary" change={summary_model => change({ ...value, summary_model })} profiles={profiles[summary.model] ?? []} />
    </fieldset>}
  </>;
}
type RequestFields = Pick<ModelLayer, 'profile' | 'max_output_tokens' | 'request_params'>;
/** Each variant owns complete, distinct labels so the Summary model's fields
 * and the outer model's fields stay unambiguously distinguishable, for a reader
 * and for a test alike, in every locale. */
const REQUEST_FIELD_LABELS = {
  default: { profile: 'settings:models-page.profile', limit: 'settings:models-page.output-limit', params: 'settings:models-page.request-parameter-overrides' },
  summary: { profile: 'settings:models-page.profile-summary', limit: 'settings:models-page.output-limit-summary', params: 'settings:models-page.request-parameter-overrides-summary' },
} as const satisfies Record<string, Record<string, TranslationKey>>;
/** A selection's Profile, output budget and explicit overrides. An omitted
 * Profile selects the Model's own default; an override may not repeat a key
 * the selected Profile declares, which native reports. */
function ModelRequestFields<T extends RequestFields>({ value, change, profiles, variant = 'default' }: { value: T; change: (next: T) => void; profiles: readonly string[]; variant?: keyof typeof REQUEST_FIELD_LABELS }) {
  const tx = useTranslation();
  const labels = REQUEST_FIELD_LABELS[variant];
  const selected = value.profile ?? '';
  const choices = [...new Set([...profiles, ...(selected ? [selected] : [])])];
  return <>
    {choices.length > 0 && <Choice label={tx(labels.profile)} value={selected}
      options={[['', tx('settings:copy.model-default-profile')], ...choices.map(id => [id, id] as const)]}
      onChange={profile => { const { profile: _, ...rest } = value; change((profile ? { ...rest, profile } : rest) as T); }} />}
    <label>{tx(labels.limit)}<input type="number" min="1" value={value.max_output_tokens?.mode === 'limit' ? value.max_output_tokens.tokens : ''}
      onChange={event => change({ ...value, max_output_tokens: event.target.value ? { mode: 'limit', tokens: Number(event.target.value) } : { mode: 'catalog_default' } })} /></label>
    <RequestParamsEditor label={tx(labels.params)} value={value.request_params ?? undefined} change={request_params => change({ ...value, request_params })} />
  </>;
}
