import { SettingsNotice } from '../SettingsFeedback';
import type { TranslationKey } from '../../../locale/translation';
import { useTranslation } from '../../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted settings cards and request controls; see PROVENANCE.md. */
import { useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { shallowEqual, useSelector } from '@xstate/react';
import { Button as AriaButton, GridList, GridListItem } from 'react-aria-components';
import type { Model, ModelLayer, ProviderView, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v79';
import { Badge } from '../../../presentation/settings/SettingsContent';
import { Switch } from '../../../presentation/primitives/Switch';
import { Button } from '../../../presentation/primitives/Button';
import { DialogSurface } from '../../../presentation/primitives/DialogSurface';
import { IconApiOutline14, IconCloseOutline16, IconEditOutline16, IconLinkOutline14, IconRefreshOutline16, IconTrashOutline16 } from '../../../presentation/primitives/icons';
import { TypedUnitForm, UnitForm, UnitOutcomeNotice, useUnitEditing, type UnitOptions } from '../forms/bridge';
import { JsonObjectEditor } from '../forms/fields';
import { Advanced, Choice, ConfirmAction, Search } from '../primitives/aria';
import { catalogEntries, sourceView, type CatalogEntry, type SettingsTarget, observedResult, observedResultLabel, unitApplication } from '../projection';
import type { PageFocus } from '../machines/navigation';
import { useSettingsActor } from '../machines/react';
import { ModelFields } from './ModelFields';
import { ProviderIcon } from './ProviderIcon';
import type { AppServerClient } from '../../../client/app-server';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { useModelProbe } from './useModelProbe';
import { ProviderConnection } from './ProviderConnection';
import { ProviderNameDialog } from './ProviderTemplates';
import css from '../../../presentation/settings/SettingsContent.module.css';
import cards from './ModelsPage.module.css';

export interface ModelsPageProps {
  client: AppServerClient; host?: ProductHostWorkspaces; target: SettingsTarget;
  source: SourceSettings; scope: SourceScope; revision: string;
  models: string[];
  profiles: ModelProfiles;
  focus?: PageFocus['models'];
  onFocus: (focus?: PageFocus['models']) => void;
  onRefresh?: () => void;
  refreshing?: boolean;
}

const compactContext = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 });

const emptyModel = (provider = '', id = ''): Model => ({
  provider, id, context_window: '128000', max_output_tokens: 8192,
  capabilities: { input_modalities: ['text'], output_modalities: ['text'], json_schema_output: false },
});

/** Provider and model settings backed by rustX's native semantic units.
 * Each model inherits its provider's connection protocol. */
export function ModelsPage({ client, host, target, source, scope, revision, models, profiles, focus, onFocus, onRefresh, refreshing = false }: ModelsPageProps) {
  const tx = useTranslation();
  const landing = useRef<HTMLElement>(null);
  const previousProvider = useRef<string>(undefined);
  const probe = useModelProbe(client, host, target, source);
  const [query, setQuery] = useState('');
  const [adding, setAdding] = useState<'provider' | 'model'>();
  const actor = useSettingsActor();
  const draftedProviders = useSelector(actor, state => Object.values(state.context.units).flatMap(ref => {
    const { selector } = ref.getSnapshot().context;
    return selector.kind === 'config' && selector.unit === 'provider' && selector.entry ? [selector.entry] : [];
  }), shallowEqual);
  const providers = catalogEntries<ProviderView>(source, scope, 'providers');
  const catalog = catalogEntries<Model>(source, scope, 'models');
  // Keep models editable while their provider is missing or its source is invalid.
  const unconfigured = new Set(draftedProviders);
  for (const entry of catalog) {
    const model = entry.authored ?? entry.effective;
    if (model) unconfigured.add(model.provider);
  }
  for (const entry of providers) unconfigured.delete(entry.id);
  const navigationProviders: CatalogEntry<ProviderView>[] = [
    ...providers,
    ...[...unconfigured].map(id => ({ id, origin: { state: 'unavailable' as const } })),
  ];
  const requested = focus?.kind === 'provider' ? focus.id : focus?.kind === 'model' ? focus.provider : undefined;
  const missing = !!requested && !navigationProviders.some(entry => entry.id === requested);
  const selected = missing ? navigationProviders[0]?.id : requested ?? navigationProviders[0]?.id;
  useLayoutEffect(() => {
    const previous = previousProvider.current;
    previousProvider.current = selected;
    const removed = previous !== undefined && !navigationProviders.some(entry => entry.id === previous);
    if (removed || missing && focus?.kind === 'provider') {
      onFocus(undefined);
      landing.current?.focus();
    }
  }, [navigationProviders, selected, missing, focus?.kind, onFocus]);
  const search = query.trim().toLocaleLowerCase();
  const navigation = navigationProviders.filter(({ id, authored, effective }) => {
    const name = (authored ?? effective)?.display_name;
    return id.toLocaleLowerCase().includes(search) || name?.toLocaleLowerCase().includes(search);
  });
  const providerEntry = navigationProviders.find(entry => entry.id === selected);
  const selectedProvider = providerEntry?.authored ?? providerEntry?.effective;
  const selectedModels = catalog.filter(entry => (entry.authored ?? entry.effective)?.provider === selected);
  const visibleModels = models.filter(id => {
    const entry = catalog.find(entry => entry.id === id);
    const model = entry?.authored ?? entry?.effective;
    const owner = providers.find(entry => entry.id === model?.provider);
    const provider = owner?.authored ?? owner?.effective;
    return !!model && !!provider && !model.hidden && !provider.hidden;
  });
  const application = observedResult(unitApplication(source.application, 'provider'));
  const closeModel = () => onFocus(selected ? { kind: 'provider', id: selected } : undefined);
  return <section ref={landing} tabIndex={-1} aria-label={tx('settings:models-page.models')} className={cards.section}>
    <div className={cards.toolbar}>
      <div><h3>{tx('settings:models-page.models')}</h3>
        <p className={cards.description}>{tx('settings:models.description')}</p></div>
      <div className={cards.headerActions}>
        {onRefresh && <Button size="sm" variant="outline" aria-label={tx('settings:settings.reload-configuration')} title={tx('settings:settings.reload-configuration')} disabled={refreshing} onClick={onRefresh}><IconRefreshOutline16/></Button>}
        <Button size="sm" variant="primary" onClick={() => setAdding('provider')}>{tx('settings:models.add-provider')}</Button>
      </div>
    </div>
    <SettingsNotice text={application.state === 'failed' ? `${observedResultLabel(tx, application)} · ${application.diagnostic}` : undefined}/>
    <div className={cards.layout}>
      {!!navigationProviders.length && <div className={cards.navigation}>
        <h4>{tx('settings:models.custom-providers')}</h4>
        <Search label={tx('settings:models-page.find-a-provider')} value={query} onChange={setQuery} placeholder={tx('settings:models.search')} />
        <GridList className={cards.providers} aria-label={tx('settings:models-page.providers')} selectionMode="single" disallowEmptySelection selectedKeys={selected ? [selected] : []}
          onSelectionChange={keys => {
            if (keys === 'all') return;
            const id = keys.values().next().value;
            if (id !== undefined) onFocus({ kind: 'provider', id: String(id) });
          }}>
          {navigation.map(entry => {
            const provider = entry.authored ?? entry.effective;
            const name = provider?.display_name ?? entry.id;
            const hasModels = catalog.some(item => {
              const model = item.authored ?? item.effective;
              return model?.provider === entry.id && !model.hidden;
            });
            return <GridListItem id={entry.id} key={entry.id} textValue={name} aria-label={name} className={cards.provider}>
              <ProviderIcon icon={provider?.icon} size={16}/>
              <span className={cards.providerName} title={name}>{name}</span>
              <span className={cards.providerDot} data-state={provider?.hidden ? 'off' : provider && hasModels ? 'ready' : 'empty'} aria-hidden="true"/>
            </GridListItem>;
          })}
        </GridList>
        {!navigation.length && <p className={cards.emptyNavigation}>{query ? tx('settings:models-page.no-provider-identity-matches-value', { p0: query }) : tx('settings:models-page.no-provider-is-defined-for-this-source-yet')}</p>}
      </div>}
      {selected ? <div className={cards.detailPane}>
          <ProviderConnection key={selected} scope={scope} revision={revision} id={selected} authored={providerEntry?.authored}
            inherited={scope === 'workspace' ? (providerEntry?.authored ? source.user.authored?.providers?.[selected] : providerEntry?.effective) : undefined}
            hasModels={selectedModels.length > 0}/>
          <div className={cards.modelsHeading}><h4>{tx('settings:models.list')}</h4>
            <Button size="sm" variant="toolbar" title={!selectedProvider ? tx('settings:models.save-provider-first') : undefined}
              disabled={!selectedProvider} onClick={() => setAdding('model')}>{tx('settings:models.add-model')}</Button>
          </div>
          {selectedModels.length ? <GridList aria-label={tx('settings:models-page.models-of-provider-value', { p0: selectedProvider?.display_name ?? selected })} className={cards.modelList}>
            {selectedModels.map(entry => <ModelRow key={entry.id} entry={entry} revision={revision} providerHidden={!selectedProvider || !!selectedProvider.hidden} canProbe={!!selectedProvider} probe={probe}
              scope={scope} restoresGlobal={scope === 'workspace' && !!source.user.authored?.models?.[entry.id]} settle={landing}
              onOpen={() => onFocus({ kind: 'model', id: entry.id, provider: selected })}/>)}
          </GridList> : <p className={cards.emptyModels}>{tx('settings:models.empty-models')}</p>}
      </div> : <div className={cards.unselected}><IconApiOutline14 size={24}/><p>{tx('settings:models.empty-providers')}</p></div>}
    </div>
    <div className={cards.defaultModel}>
      <Advanced title={tx('settings:models-page.default-model-for-new-sessions')}>
        <UnitForm<ModelLayer> title={tx('settings:models-page.default-model')} authored={sourceView(source, scope)?.authored?.agent?.model ?? undefined}
          blank={{}} revision={revision} mutation={authored => ({ kind: 'config', mutation: { unit: 'root_model', authored } })}
          removalNotice={<p>{tx('settings:models-page.new-sessions-fall-back-to-the-native-default-model-once-no-sourc')}</p>}>
          {(value, change) => <><p className={css.hint}>{tx('settings:models-page.this-is-the-model-new-sessions-start-from-it-is-not-the-model-an')}</p>
            <DefaultModelSelection value={value} change={change} models={visibleModels} profiles={profiles} /></>}
        </UnitForm>
      </Advanced>
    </div>
    {adding === 'provider' && <ProviderNameDialog revision={revision} exists={[...providers.map(entry => entry.id), ...draftedProviders]} onClose={() => setAdding(undefined)}
      onCreate={id => { setAdding(undefined); onFocus({ kind: 'provider', id }); }} />}
    {adding === 'model' && selected && <ModelDialog source={source} scope={scope} revision={revision} provider={selected} suggestDeepSeek={selectedProvider?.base_url.replace(/\/$/, '') === 'https://api.deepseek.com/anthropic'} onClose={() => setAdding(undefined)} />}
    {focus?.kind === 'model' && <ModelDialog source={source} scope={scope} revision={revision} id={focus.id} provider={focus.provider} onClose={closeModel} />}
  </section>;
}

function ModelRow({ entry, revision, providerHidden, canProbe, probe, scope, restoresGlobal, settle, onOpen }: {
  entry: CatalogEntry<Model>; revision: string; providerHidden: boolean; canProbe: boolean; probe: ReturnType<typeof useModelProbe>;
  scope: SourceScope; restoresGlobal: boolean; settle: RefObject<HTMLElement | null>; onOpen: () => void;
}) {
  const tx = useTranslation();
  const model = entry.authored ?? entry.effective;
  const unit = useUnitEditing({
    authored: entry.authored, blank: emptyModel(), revision,
    mutation: authored => ({ kind: 'config', mutation: { unit: 'model', id: entry.id, authored } }),
  });
  if (!model) return null;
  const context = Number(model.context_window);
  const status = probe.states[entry.id];
  const title = tx('settings:models-page.model-value', { p0: entry.id });
  const removeLabel = tx(restoresGlobal ? 'settings:bridge.use-global-default-value' : 'settings:bridge.remove-value', { p0: title });
  const removeDisabled = !unit.admitted || unit.busy || unit.awaitingObservation;
  return <GridListItem id={entry.id} textValue={model.id} aria-label={entry.id} onAction={onOpen} className={cards.modelRow} data-hidden={!!model.hidden || providerHidden}>
    <div className={cards.modelIdentity}>
      <span className={cards.modelName} title={model.id}>{model.id}</span>
      <div className={cards.modelBadges}>
        <span title={tx('settings:models.context-tokens', { p0: model.context_window })}><Badge>{compactContext.format(context)}</Badge></span>
        {model.capabilities.input_modalities.includes('image') && <Badge>{tx('settings:models.vision')}</Badge>}
      </div>
    </div>
    <div className={cards.modelActions}
      onPointerDown={event => event.stopPropagation()} onClick={event => event.stopPropagation()}
      onKeyDown={event => { if (event.key === 'Enter' || event.key === ' ') event.stopPropagation(); }}>
      <Button size="sm" className={cards.iconButton} aria-label={tx('settings:models.test-model', { p0: model.id })}
        title={tx('settings:models.test-model', { p0: model.id })} aria-busy={status === 'checking'} data-probe={status}
        disabled={!canProbe || !unit.admitted || !probe.available || status === 'checking'} onClick={() => { void probe.test(entry.id); }}><IconLinkOutline14/></Button>
      <Button size="sm" className={cards.iconButton} aria-label={tx('settings:models.edit-model')} title={tx('settings:models.edit-model')} onClick={onOpen}><IconEditOutline16 size={14}/></Button>
      {entry.authored && <ConfirmAction label={removeLabel} confirm={removeLabel}
        tone={restoresGlobal ? 'restore' : 'destructive'} settle={settle} disabled={removeDisabled} onConfirm={() => unit.submit(true)}
        title={restoresGlobal ? tx('settings:bridge.use-the-global-default-for-value', { p0: title }) : tx('settings:bridge.remove-value-from-value-configuration', { p0: title, p1: tx(scope === 'workspace' ? 'settings:extension-detail.workspace' : 'settings:extension-detail.user') })}
        description={<p>{tx(restoresGlobal ? 'settings:forms.restore-help' : 'settings:models-page.any-source-that-names-this-model-identity-keeps-naming-it-native')}</p>}
        renderTrigger={(open, ref) => <AriaButton ref={ref} className={cards.iconButton} aria-label={removeLabel} isDisabled={removeDisabled} onPress={open}>
          {restoresGlobal ? <IconRefreshOutline16 size={14}/> : <IconTrashOutline16 size={14}/>}
        </AriaButton>}/>}
      <Switch checked={!model.hidden && !providerHidden} label={tx('settings:models.show-model', { p0: model.id })}
        disabled={!entry.authored || providerHidden || !unit.admitted || unit.draft || unit.awaitingObservation}
        onChange={checked => unit.apply({ ...model, hidden: !checked })}/>
    </div>
    <UnitOutcomeNotice title={entry.id} outcome={unit.outcome} />
  </GridListItem>;
}

function ModelDialog({ source, scope, revision, id, provider, suggestDeepSeek, onClose }: { source: SourceSettings; scope: SourceScope; revision: string; id?: string; provider?: string; suggestDeepSeek?: boolean; onClose: () => void }) {
  const tx = useTranslation();
  const dialog = useRef<HTMLElement>(null);
  const authored = id ? sourceView(source, scope)?.authored?.models?.[id] : undefined;
  const catalog = catalogEntries<Model>(source, scope, 'models');
  const inherited = id ? catalog.find(entry => entry.id === id)?.effective : undefined;
  const creating = id === undefined;
  const title = creating ? tx('settings:models-page.add-model') : tx('settings:models-page.model-value', { p0: id });
  const options: UnitOptions<Model> = {
    authored: authored ?? undefined, blank: inherited ?? emptyModel(provider), revision,
    draftKey: creating ? JSON.stringify({ create: 'model', provider: provider ?? null }) : undefined,
    mutation: value => ({ kind: 'config', mutation: { unit: 'model', id: id ?? (value ? `${value.provider}/${value.id.trim()}` : ''), authored: value && { ...value, id: value.id.trim() } } }),
  };
  const unit = useUnitEditing(options);
  const close = () => { if (unit.busy) return; if (creating) unit.discard(); onClose(); };
  const duplicate = (owner: string, wireId: string) => catalog.some(entry => {
    const model = entry.authored ?? entry.effective;
    return entry.id === `${owner}/${wireId}` || model?.provider === owner && model.id === wireId;
  });
  // Model metadata from the matching DeepSeek template in ZCode's catalog.
  const suggestions = creating && suggestDeepSeek ? ['deepseek-flash', 'deepseek-v4-pro'].filter(id => !duplicate(provider ?? '', id)) : [];
  return <DialogSurface open title={title} onClose={close} overlayClassName={cards.overlay} className={`${css.settings} ${cards.modelDialog}`} contentRef={dialog}
    initialFocus={() => {
      const input = dialog.current?.querySelector<HTMLInputElement>(authored ? 'input[type="number"]' : 'input');
      if (input && !input.matches(':disabled')) { input.focus(); input.select(); return false; }
      return dialog.current ?? false;
    }}>
    <div className={cards.dialogHeader}><h2>{creating ? tx('settings:models-page.add-model') : tx('settings:models.edit-model')}</h2><Button aria-label={tx('settings:models.close')} disabled={unit.busy} onClick={close}><IconCloseOutline16/></Button></div>
    {!!suggestions.length && <div role="group" className={cards.templates} aria-label={tx('settings:models.model-suggestions')}>
      {suggestions.map(modelId => <Button key={modelId} size="sm" variant="outline" disabled={unit.busy || !unit.writable}
        aria-pressed={unit.displayed.id === modelId} onClick={() => unit.edit({
          ...unit.displayed, id: modelId, context_window: '1000000',
          capabilities: { ...unit.displayed.capabilities, input_modalities: modelId === 'deepseek-flash' ? ['text', 'image'] : ['text'] },
        })}>{modelId}</Button>)}
    </div>}
    <TypedUnitForm<Model> {...options} title={title}
      presentation="dialog" removable={false} onCancel={close} onSaved={creating ? onClose : undefined}>
      {form => <ModelFields form={form} duplicate={creating ? duplicate : undefined} />}
    </TypedUnitForm>
  </DialogSurface>;
}

/** The Model Profiles each model identity this scope reaches declares. */
export type ModelProfiles = Readonly<Record<string, readonly string[]>>;

type ModelSelectionProps = { value: ModelLayer; change: (next: ModelLayer) => void; models: string[]; profiles: ModelProfiles };

function DefaultModelSelection({ value, change, models, profiles }: ModelSelectionProps) {
  const tx = useTranslation();
  const choices = new Set(models);
  const model = value.model && choices.has(value.model) ? value.model : null;
  // A new primary selection uses its own preset's parameters and output
  // budget. The auxiliary models are configured independently in the source.
  const { profile, request_params: _params, max_output_tokens: _limit, ...selection } = value;
  return <>
    <div className={cards.defaultChoices}>
      <Choice label={tx('settings:models-page.model')} value={model} placeholder={tx('settings:copy.select-model')} disabled={choices.size === 0}
        options={[...choices].map(id => [id, id])} onChange={model => change({ ...selection, model })}/>
      {model && <ModelProfileChoice label={tx('settings:models-page.profile')} profile={profile} profiles={profiles[model] ?? []}
        onChange={profile => change(profile ? { ...selection, profile } : selection)}/>}
    </div>
    {value.model && <Advanced title={tx('settings:models-page.auxiliary-models')}>
      <AuxiliaryModelSelections value={value} change={change} models={models} profiles={profiles}/>
    </Advanced>}
  </>;
}

/** The remaining invocation settings after a named Agent chooses its model and preset. */
export function AgentModelOptions({ value, change, models, profiles }: ModelSelectionProps) {
  return <>
    <ModelRequestFields value={value} change={change} profiles={[]} showProfile={false} />
    <AuxiliaryModelSelections value={value} change={change} models={models} profiles={profiles}/>
  </>;
}

type AuxiliaryAuthoring = NonNullable<ModelLayer['auxiliary']>;
type AuxiliaryProfileAuthoring = AuxiliaryAuthoring['profile'];

function auxiliaryPolicy(value: ModelLayer): AuxiliaryAuthoring {
  return value.auxiliary ?? { mode: 'session', profile: { mode: 'lowest' } };
}

function AuxiliaryModelSelections({ value, change, models, profiles }: ModelSelectionProps) {
  const tx = useTranslation();
  const auxiliary = auxiliaryPolicy(value);
  const identities = (extra?: string) => [...new Set([...models, ...(extra ? [extra] : [])])];
  const modelId = auxiliary.mode === 'explicit' ? auxiliary.model : value.model;
  const profileMode = auxiliary.profile?.mode ?? 'lowest';
  const setAuxiliary = (next: AuxiliaryAuthoring) => change({ ...value, auxiliary: next });
  const setProfile = (profile: AuxiliaryProfileAuthoring) => setAuxiliary({ ...auxiliary, profile });
  return <>
    <Choice label={tx('settings:models-page.auxiliary-model')} value={auxiliary.mode === 'explicit' ? auxiliary.model : ''}
      options={[['', tx('settings:copy.follow-selected-model')], ...identities(auxiliary.mode === 'explicit' ? auxiliary.model : undefined).map(id => [id, id] as const)]}
      onChange={model => setAuxiliary(model
        ? { ...(auxiliary.mode === 'explicit' ? auxiliary : { profile: auxiliary.profile, request_params: auxiliary.request_params, max_output_tokens: auxiliary.max_output_tokens }), mode: 'explicit', model }
        : { mode: 'session', profile: { mode: 'lowest' } })} />
    <Choice label={tx('settings:models-page.auxiliary-profile-strategy')} value={profileMode}
      options={[
        ['lowest', tx('settings:models-page.auxiliary-profile-lowest')],
        ['model_default', tx('settings:models-page.auxiliary-profile-model-default')],
        ['explicit', tx('settings:models-page.auxiliary-profile-explicit')],
      ]}
      onChange={mode => {
        if (mode === 'lowest') setProfile({ mode: 'lowest' });
        else if (mode === 'model_default') setProfile({ mode: 'model_default' });
        else {
          const existing = auxiliary.profile?.mode === 'explicit' ? auxiliary.profile.id : undefined;
          const first = profiles[modelId ?? '']?.[0];
          const id = existing && existing.length > 0 ? existing : first;
          // Explicit requires a concrete Profile id; without one, keep model_default.
          if (id) setProfile({ mode: 'explicit', id });
          else setProfile({ mode: 'model_default' });
        }
      }} />
    <p className={css.hint}>{tx('settings:models-page.auxiliary-profile-lowest-help')}</p>
    <fieldset><legend>{tx('settings:models-page.auxiliary-model-settings')}</legend>
      {profileMode === 'explicit' && modelId && <ModelProfileChoice label={tx('settings:models-page.auxiliary-profile')} profile={auxiliary.profile?.mode === 'explicit' ? auxiliary.profile.id : undefined}
        profiles={profiles[modelId] ?? []}
        onChange={id => {
          if (!id) setProfile({ mode: 'model_default' });
          else setProfile({ mode: 'explicit', id });
        }} />}
      <label>{tx('settings:models-page.output-limit-auxiliary')}<input type="number" min="1" value={auxiliary.max_output_tokens?.mode === 'limit' ? auxiliary.max_output_tokens.tokens : ''}
        onChange={event => setAuxiliary({ ...auxiliary, max_output_tokens: event.target.value ? { mode: 'limit', tokens: Number(event.target.value) } : { mode: 'catalog_default' } })} /></label>
      <JsonObjectEditor label={tx('settings:models-page.request-parameter-overrides-auxiliary')} value={auxiliary.request_params ?? undefined}
        change={request_params => setAuxiliary({ ...auxiliary, request_params })} />
    </fieldset>
  </>;
}
type RequestFields = Pick<ModelLayer, 'profile' | 'max_output_tokens' | 'request_params'>;
const REQUEST_FIELD_LABELS = {
  default: { profile: 'settings:models-page.profile', limit: 'settings:models-page.output-limit', params: 'settings:models-page.request-parameter-overrides' },
} as const satisfies Record<string, Record<string, TranslationKey>>;
/** A selection's Profile, output budget and explicit overrides. An omitted
 * Profile selects the Model's own default; an override may not repeat a key
 * the selected Profile declares, which native reports. */
function ModelRequestFields<T extends RequestFields>({ value, change, profiles, variant = 'default', showProfile = true }: { value: T; change: (next: T) => void; profiles: readonly string[]; variant?: keyof typeof REQUEST_FIELD_LABELS; showProfile?: boolean }) {
  const tx = useTranslation();
  const labels = REQUEST_FIELD_LABELS[variant];
  return <>
    {showProfile && <ModelProfileChoice label={tx(labels.profile)} profile={value.profile} profiles={profiles}
      onChange={profile => { const { profile: _, ...rest } = value; change((profile ? { ...rest, profile } : rest) as T); }}/>}
    <label>{tx(labels.limit)}<input type="number" min="1" value={value.max_output_tokens?.mode === 'limit' ? value.max_output_tokens.tokens : ''}
      onChange={event => change({ ...value, max_output_tokens: event.target.value ? { mode: 'limit', tokens: Number(event.target.value) } : { mode: 'catalog_default' } })} /></label>
    <JsonObjectEditor label={tx(labels.params)} value={value.request_params ?? undefined} change={request_params => change({ ...value, request_params })} />
  </>;
}

export function ModelProfileChoice({ label, profile, profiles, onChange }: {
  label: string; profile?: string | null; profiles: readonly string[]; onChange: (profile: string) => void;
}) {
  const tx = useTranslation();
  const choices = new Set(profiles);
  if (profile) choices.add(profile);
  // Empty id is not a valid Explicit Profile; the blank option clears back to
  // the model default through the empty-string sentinel handled by callers.
  return choices.size > 0 ? <Choice label={label} value={profile ?? ''}
    options={[['', tx('settings:copy.model-default-profile')], ...[...choices].map(id => [id, id] as const)]}
    onChange={onChange}/> : null;
}
