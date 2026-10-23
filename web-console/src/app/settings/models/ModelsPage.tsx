import type { Translate, TranslationKey } from '../../../locale/translation';
import { useTranslation } from '../../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted settings cards and request controls; see PROVENANCE.md. */
import { useRef, useState } from 'react';
import { GridList, GridListItem } from 'react-aria-components';
import type { Model, ModelLayer, ProviderView, ProviderWrite, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v53';
import { Badge } from '../../../presentation/settings/SettingsContent';
import { Button } from '../../../presentation/primitives/Button';
import { DialogSurface } from '../../../presentation/primitives/DialogSurface';
import { IconApiOutline14, IconCloseOutline16, IconRefreshOutline16 } from '../../../presentation/primitives/icons';
import { TypedUnitForm, UnitForm, type TypedUnitForm as TypedForm } from '../forms/bridge';
import { Enum, JsonObjectEditor, Text } from '../forms/fields';
import { Advanced, Choice, ResourceList, Search, type ResourceRow } from '../primitives/aria';
import { catalogEntries, provenanceLabel, sourceView, type CatalogEntry, observedResult, observedResultLabel, unitApplication } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { ModelFields } from './ModelFields';
import css from '../../../presentation/settings/SettingsContent.module.css';
import cards from './ModelsPage.module.css';

export interface ModelsPageProps {
  source: SourceSettings; scope: SourceScope; revision: string;
  models: string[];
  profiles: ModelProfiles;
  focus?: PageFocus['models'];
  onFocus: (focus?: PageFocus['models']) => void;
  onRefresh: () => void;
}

const emptyModel = (provider = '', id = ''): Model => ({
  provider, id, protocol: 'openai_responses', context_window: '128000', max_output_tokens: 8192,
  capabilities: { input_modalities: ['text'], output_modalities: ['text'] },
});

function credentialLabel(tx: Translate, provider: ProviderView | undefined): string {
  return !provider ? tx('settings:copy.definition-not-resolved')
    : provider.credential.type === 'environment' ? tx('settings:copy.environment-value', { p0: provider.credential.variable }) : tx('settings:copy.literal-secret-redacted');
}

/** ZCode's provider navigation/detail split and model dialog, bound to rustX's
 * native semantic units. Provider availability and API format are not invented:
 * native owns availability, and the protocol belongs to each Model. */
export function ModelsPage({ source, scope, revision, models, profiles, focus, onFocus, onRefresh }: ModelsPageProps) {
  const tx = useTranslation();
  const landing = useRef<HTMLElement>(null);
  const [query, setQuery] = useState('');
  const [adding, setAdding] = useState<'provider' | 'model'>();
  const providers = catalogEntries<ProviderView>(source, scope, 'providers');
  const catalog = catalogEntries<Model>(source, scope, 'models');
  const selected = focus?.kind === 'provider' ? focus.id : focus?.kind === 'model' ? focus.provider : undefined;
  const matches = providers.filter(entry => entry.id.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
  const navigation = matches;
  const application = observedResult(unitApplication(source.application, 'provider'));
  const closeModel = () => onFocus(selected ? { kind: 'provider', id: selected } : undefined);
  return <section ref={landing} tabIndex={-1} aria-label={tx('settings:models-page.models')} className={cards.section}>
    <h3 className={cards.title}>{tx('settings:models-page.models')}</h3>
    <div className={cards.toolbar}>
      <p>{tx('settings:models.description')}</p>
      <div className={cards.headerActions}>
        <Button size="sm" aria-label={tx('settings:models.refresh')} title={tx('settings:models.refresh')} onClick={onRefresh}><IconRefreshOutline16/></Button>
        <Button size="sm" variant="primary" onClick={() => setAdding('provider')}>{tx('settings:models.add-provider')}</Button>
      </div>
    </div>
    {application.state === 'failed' && <p className={css.error} role="alert">{observedResultLabel(tx, application)} — {application.diagnostic}</p>}
    <div className={cards.splitPanel} data-selected={!!selected}>
      <div className={cards.navigation}>
        <Search label={tx('settings:models-page.find-a-provider')} value={query} onChange={setQuery} placeholder={tx('settings:models.search')} />
        <h4>{tx('settings:models-page.providers')}</h4>
        <GridList className={cards.providers} aria-label={tx('settings:models-page.providers')} selectionMode="single" selectedKeys={selected ? [selected] : []}
          onAction={id => onFocus({ kind: 'provider', id: String(id) })}>
          {navigation.map(entry => <GridListItem id={entry.id} key={entry.id} textValue={entry.id} aria-label={entry.id} className={cards.provider}>
            <span aria-hidden="true" className={cards.providerIcon}><IconApiOutline14 size={18}/></span>
            <span className={cards.providerLabel}><span className={cards.providerName} title={entry.id}>{entry.id}</span>
              {scope === 'workspace' && entry.origin && <small>{provenanceLabel(tx, entry.origin)}</small>}
            </span>
            <span className={cards.providerDot} data-authored={!!entry.authored} title={entry.authored ? tx('settings:models-page.configured') : entry.origin ? provenanceLabel(tx, entry.origin) : tx('settings:models-page.new-provider')}/>
          </GridListItem>)}
        </GridList>
        {!navigation.length && <p className={cards.emptyNavigation}>{query ? tx('settings:models-page.no-provider-identity-matches-value', { p0: query }) : tx('settings:models-page.no-provider-is-defined-for-this-source-yet')}</p>}
      </div>
      <div className={cards.detailPane}>
        {selected ? <ProviderDetail key={selected} source={source} scope={scope} revision={revision} id={selected} onBack={() => onFocus(undefined)}
          onModel={id => onFocus({ kind: 'model', id, provider: selected })} onAddModel={() => setAdding('model')}/>
          : <div className={cards.unselected}><span aria-hidden="true"><IconApiOutline14 size={32}/></span><p>{tx('settings:models.select-provider')}</p></div>}
      </div>
    </div>
    <Advanced title={tx('settings:models-page.all-models-value', { p0: catalog.length })}>
      <ResourceList label={tx('settings:models-page.all-models')} rows={catalog.map(entry => modelRow(tx, entry, scope))}
        onOpen={id => onFocus({ kind: 'model', id, provider: (catalog.find(entry => entry.id === id)?.authored ?? catalog.find(entry => entry.id === id)?.effective)?.provider })}
        empty={tx('settings:copy.no-model-is-defined-for-this-source-yet')} />
      <Button size="sm" onClick={() => setAdding('model')}>{tx('settings:models-page.add-model')}</Button>
    </Advanced>
    <Advanced title={tx('settings:models-page.default-model-for-new-sessions')}>
      <UnitForm<ModelLayer> title={tx('settings:models-page.default-model')} authored={sourceView(source, scope)?.authored?.agent?.model ?? undefined}
        blank={{}} revision={revision} mutation={authored => ({ kind: 'config', mutation: { unit: 'root_model', authored } })}
        removalNotice={<p>{tx('settings:models-page.new-sessions-fall-back-to-the-native-default-model-once-no-sourc')}</p>}>
        {(value, change) => <><p className={css.hint}>{tx('settings:models-page.this-is-the-model-new-sessions-start-from-it-is-not-the-model-an')}</p>
          <ModelSelection value={value} change={change} models={models} profiles={profiles} /></>}
      </UnitForm>
    </Advanced>
    {adding && <ResourceNameDialog kind={adding} exists={(adding === 'provider' ? providers : catalog).map(entry => entry.id)} onClose={() => setAdding(undefined)}
      onCreate={id => { setAdding(undefined); onFocus(adding === 'provider' ? { kind: 'provider', id } : { kind: 'model', id, provider: selected }); }} />}
    {focus?.kind === 'model' && <ModelDialog source={source} scope={scope} revision={revision} id={focus.id} provider={focus.provider} onClose={closeModel} />}
  </section>;
}

/** Native configuration identities are independent of wire model IDs. Choose
 * the stable unit identity before its transaction starts; no temporary native
 * resource is created merely by opening Add. */
function ResourceNameDialog({ kind, exists, onCreate, onClose }: { kind: 'provider' | 'model'; exists: readonly string[]; onCreate: (id: string) => void; onClose: () => void }) {
  const tx = useTranslation();
  const [identity, setIdentity] = useState('');
  const input = useRef<HTMLInputElement>(null);
  const duplicate = exists.includes(identity.trim());
  const title = kind === 'provider' ? tx('settings:models-page.add-provider') : tx('settings:models-page.add-model');
  return <DialogSurface open onClose={onClose} title={title} overlayClassName={cards.overlay} className={`${css.settings} ${cards.nameDialog}`} initialFocus={input}>
    <div className={cards.dialogHeader}><h2>{title}</h2><Button aria-label={tx('settings:models.close')} onClick={onClose}><IconCloseOutline16/></Button></div>
    <form onSubmit={event => { event.preventDefault(); if (identity.trim() && !duplicate) onCreate(identity.trim()); }}>
      <label>{kind === 'provider' ? tx('settings:models-page.new-provider-identity') : tx('settings:models-page.new-model-identity')}
        <input ref={input} required value={identity} aria-invalid={duplicate || undefined} onChange={event => setIdentity(event.target.value)} />
      </label>
      <p className={css.hint}>{kind === 'model' ? tx('settings:models.identity-help') : tx('settings:models.provider-identity-help')}</p>
      {duplicate && <p role="alert" className={css.error}>{tx('settings:models.duplicate')}</p>}
      <div className={cards.nameActions}><Button onClick={onClose}>{tx('settings:aria.cancel')}</Button><Button type="submit" variant="primary" disabled={!identity.trim() || duplicate}>{tx('settings:models.continue')}</Button></div>
    </form>
  </DialogSurface>;
}

function modelRow(tx: Translate, entry: CatalogEntry<Model>, scope: SourceScope): ResourceRow {
  const model = entry.authored ?? entry.effective;
  return { id: entry.id, name: entry.id,
    facts: <><Badge>{model?.protocol}</Badge>{scope === 'workspace' && <Badge>{provenanceLabel(tx, entry.origin)}</Badge>}</>,
    detail: <p className={css.hint}>{model?.id !== entry.id && <>{model?.id} · </>}{model?.context_window}{' '}{tx('settings:models-page.context')}{' '}{model?.max_output_tokens}{' '}{tx('settings:models-page.output')}</p>,
  };
}

function ProviderDetail({ source, scope, revision, id, onBack, onModel, onAddModel }: {
  source: SourceSettings; scope: SourceScope; revision: string; id: string;
  onBack: () => void; onModel: (id: string) => void; onAddModel: () => void;
}) {
  const tx = useTranslation();
  const authored = sourceView(source, scope)?.authored?.providers?.[id];
  const inherited = !authored ? catalogEntries<ProviderView>(source, scope, 'providers').find(entry => entry.id === id)?.effective : undefined;
  const owned = catalogEntries<Model>(source, scope, 'models').filter(entry => (entry.authored ?? entry.effective)?.provider === id);
  return <section aria-label={tx('settings:models-page.provider-value', { p0: id })} className={cards.detail}>
    <Button size="sm" className={cards.back} onClick={onBack}>{tx('settings:models-page.models-2')}</Button>
    <h3 title={id}>{tx('settings:models-page.provider')}{' '}{id}</h3>
    {inherited && <p className={css.hint}>{tx('settings:models-page.native-effective-provider')}{' '}{id}: {inherited.base_url} · {credentialLabel(tx, inherited)}{tx('settings:models-page.an-override-authors-a-complete-new-definition-here-the-inherited')}</p>}
    <TypedUnitForm<ProviderWrite> key={`provider:${id}`} title={tx('settings:models-page.provider-value', { p0: id })}
      authored={authored && { base_url: authored.base_url, credential: { kind: 'retain' } }} blank={{ base_url: '', credential: { kind: 'environment', variable: '' } }}
      inherited={() => undefined} revision={revision} presentation="resource"
      removalNotice={<p>{tx('settings:models-page.models-that-name-this-provider-identity-are-not-changed-and-no-s')}</p>}
      mutation={value => ({ kind: 'config', mutation: { unit: 'provider', id, authored: value } })}>
      {form => <ProviderFields form={form} authored={!!authored} />}
    </TypedUnitForm>
    <div className={cards.modelsHeading}><h4>{tx('settings:models.list')}</h4><Button size="sm" onClick={onAddModel}>{tx('settings:models.add-model')}</Button></div>
    <ResourceList label={tx('settings:models-page.models-of-provider-value', { p0: id })} rows={owned.map(entry => modelRow(tx, entry, scope))}
      onOpen={onModel} empty={tx('settings:copy.no-model-in-this-source-names-provider-value', { p0: id })} />
  </section>;
}

function ProviderFields({ form, authored }: { form: TypedForm<ProviderWrite>; authored: boolean }) {
  const tx = useTranslation();
  const Subscribe = form.Subscribe as unknown as (props: { selector: (state: { values: ProviderWrite }) => string; children: (kind: string) => React.ReactNode }) => React.ReactNode;
  return <>
    <Text form={form} name="base_url" label={tx('settings:models-page.endpoint')} required url placeholder={/* i18n-raw: exact example API endpoint */ 'https://api.example.com/v1'} />
    <Enum form={form} name="credential.kind" label={tx('settings:models-page.credential-source')}
      options={[...(authored ? [['retain', tx('settings:copy.keep-the-credential-this-scope-already-authored')] as const] : []), ['environment', tx('settings:copy.read-it-from-an-environment-variable')] as const, ['literal', tx('settings:copy.enter-a-literal-secret')] as const]} />
    <Subscribe selector={state => state.values.credential.kind}>{kind => <>
      {kind === 'environment' && <Text form={form} name="credential.variable" label={tx('settings:models-page.environment-variable')} required />}
      {kind === 'literal' && <LiteralCredential form={form} />}
    </>}</Subscribe>
  </>;
}
function LiteralCredential({ form }: { form: TypedForm<ProviderWrite> }) {
  const tx = useTranslation();
  const [visible, setVisible] = useState(false);
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <Field name="credential.value">{field => <div className={cards.secret}>
    <label>{tx('settings:models-page.new-literal-credential')}<input type={visible ? 'text' : 'password'} autoComplete="new-password" spellCheck={false} required value={(field.state.value as string | undefined) ?? ''}
      onChange={event => field.handleChange(event.target.value as never)} /></label>
    <Button size="sm" aria-pressed={visible} onClick={() => setVisible(!visible)}>{visible ? tx('settings:models.hide-key') : tx('settings:models.show-key')}</Button>
  </div>}</Field>;
}

function ModelDialog({ source, scope, revision, id, provider, onClose }: { source: SourceSettings; scope: SourceScope; revision: string; id: string; provider?: string; onClose: () => void }) {
  const tx = useTranslation();
  const dialog = useRef<HTMLElement>(null);
  const authored = sourceView(source, scope)?.authored?.models?.[id];
  const title = tx('settings:models-page.model-value', { p0: id });
  return <DialogSurface open title={title} onClose={onClose} overlayClassName={cards.overlay} className={`${css.settings} ${cards.modelDialog}`} contentRef={dialog}
    initialFocus={() => {
      const input = dialog.current?.querySelector<HTMLInputElement>(authored ? 'input[type="number"]' : 'input');
      if (input && !input.matches(':disabled')) { input.focus(); input.select(); return false; }
      return dialog.current ?? false;
    }}>
    <div className={cards.dialogHeader}><h2>{authored ? tx('settings:models.edit-model') : tx('settings:models-page.add-model')}</h2><Button aria-label={tx('settings:models.close')} onClick={onClose}><IconCloseOutline16/></Button></div>
    <TypedUnitForm<Model> key={`model:${id}`} title={title} authored={authored ?? undefined} blank={emptyModel(provider)} revision={revision}
      presentation="dialog" onCancel={onClose}
      removalNotice={<p>{tx('settings:models-page.any-source-that-names-this-model-identity-keeps-naming-it-native')}</p>}
      mutation={value => ({ kind: 'config', mutation: { unit: 'model', id, authored: value } })}>
      {form => <ModelFields form={form} />}
    </TypedUnitForm>
  </DialogSurface>;
}

/** The Model Profiles each model identity this scope reaches declares. */
export type ModelProfiles = Readonly<Record<string, readonly string[]>>;

/** The model-selection fields shared by the default model and a named Agent's
 * explicit child model. Both author a `ModelLayer`, so both reach exactly the
 * same controls and the same identity-discovery rule. */
export function ModelSelection({ value, change, models, profiles }: { value: ModelLayer; change: (next: ModelLayer) => void; models: string[]; profiles: ModelProfiles }) {
  const tx = useTranslation();
  const summary = value.summary_model;
  const title = value.title_model;
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
    <Choice label={tx('settings:models-page.title-model')} value={title?.mode === 'explicit' ? title.model : ''}
      options={[['', tx('settings:copy.follow-selected-model')], ...identities(title?.mode === 'explicit' ? title.model : undefined).map(id => [id, id] as const)]}
      onChange={model => change({ ...value, title_model: model ? { ...(title?.mode === 'explicit' ? title : {}), mode: 'explicit', model } : { mode: 'session' } })} />
    {title?.mode === 'explicit' && <fieldset><legend>{tx('settings:models-page.explicit-title-model-settings')}</legend>
      <ModelRequestFields value={title} variant="title" change={title_model => change({ ...value, title_model })} profiles={profiles[title.model] ?? []} />
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
  title: { profile: 'settings:models-page.profile-title', limit: 'settings:models-page.output-limit-title', params: 'settings:models-page.request-parameter-overrides-title' },
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
    <JsonObjectEditor label={tx(labels.params)} value={value.request_params ?? undefined} change={request_params => change({ ...value, request_params })} />
  </>;
}
