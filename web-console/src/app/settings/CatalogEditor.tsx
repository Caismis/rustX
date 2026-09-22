/* Copyright (c) 2026 DeepSeek. MIT. Adapted model request controls; see PROVENANCE.md. */
import { useState } from 'react';
import type { Model, SourceScope, SourceSettings, ProviderWrite, ProviderView } from '../../../../protocol/app-server/v18';
import { Badge, SettingsCard } from '../../presentation/settings/SettingsContent';
import { RequestPolicy } from './RequestPolicy';
import { UnitForm, TextField, type SaveSource } from './controls';
import { Button } from '../../presentation/primitives/Button';
import { catalogEntries, provenanceLabel, sourceView, type CatalogEntry } from './projection';
import css from '../../presentation/settings/SettingsContent.module.css';

const emptyModel = (): Model => ({ provider: '', id: '', protocol: 'openai_responses', context_window: '128000', max_output_tokens: 8192, capabilities: { input_modalities: ['text'], output_modalities: ['text'], tool_calls: true, reasoning: false } });
/** The Providers & Models catalogs of exactly one authoring scope.
 *
 * Identity discovery is a native fact, not an authored one: the catalog
 * enumerates the identities native resolution produced, so a Workspace that
 * authors no override still reaches every Provider and Model it inherits. What
 * this scope *authors* stays a separate fact on each entry, and provenance is
 * reported from native `provenance`. The browser never merges two documents to
 * manufacture an identity, a value or an owner. */
export function CatalogEditor({ source, scope, revision, save }: { source: SourceSettings; scope: SourceScope; revision: string; save: SaveSource }) {
  const [providerId, setProviderId] = useState(''), [modelId, setModelId] = useState('');
  const [newProvider, setNewProvider] = useState(''), [newModel, setNewModel] = useState('');
  const authored = sourceView(source, scope)?.authored ?? {};
  const providers = catalogEntries<ProviderView>(source, scope, 'providers'), models = catalogEntries<Model>(source, scope, 'models');
  // Only a Workspace can inherit an identity, so only there is native ownership
  // a distinct fact worth reporting per catalog entry.
  const ownership = (entry: CatalogEntry<unknown>) => scope === 'workspace' ? <><Badge>{provenanceLabel(entry.origin)}</Badge></> : null;
  const overrideNote = (entry: CatalogEntry<unknown>) => scope === 'workspace' && !entry.authored ? ' · no override in this Workspace' : '';
  // A Provider this scope authors is reconstructed from its own redacted view;
  // a credential is never inherited from a shadowed definition, so the native
  // effective Provider is deliberately not projected into this editor. The
  // inherited definition is still reported, as the redacted native fact it is.
  const authoredProvider = authored.providers?.[providerId];
  const inheritedProvider = !authoredProvider ? providers.find(entry => entry.id === providerId)?.effective : undefined;
  if (providerId) return <section aria-label="Provider editor"><Button onClick={() => setProviderId('')}>Back to catalog</Button><p>Replace this scope's complete Provider definition. Rust validates and commits the authored unit.</p>{inheritedProvider && <p>Native effective Provider {providerId}: {inheritedProvider.base_url} · {credentialLabel(inheritedProvider)}. An override authors a complete new definition here; the inherited credential is never copied or read back.</p>}<UnitForm<ProviderWrite> key={`provider:${providerId}`} title={`Provider ${providerId}`} authored={authoredProvider && { base_url: authoredProvider.base_url, credential: { kind: 'retain' } }} blank={{ base_url: '', credential: { kind: 'environment', variable: '' } }} inherited={() => undefined} revision={revision} save={save} mutation={authored => ({ kind: 'config', mutation: { unit: 'provider', id: providerId, authored } })}>{(value, change) => <><TextField label="Endpoint" required value={value.base_url} change={base_url => change({ ...value, base_url })} /><label>Credential source<select value={value.credential.kind} onChange={e => change({ ...value, credential: e.target.value === 'retain' ? { kind: 'retain' } : e.target.value === 'environment' ? { kind: 'environment', variable: '' } : { kind: 'literal', value: '' } })}>{authoredProvider && <option value="retain">Retain this scope's credential</option>}<option value="environment">Environment variable</option><option value="literal">Literal secret</option></select></label>{value.credential.kind === 'environment' && <TextField label="Environment variable" required value={value.credential.variable} change={variable => change({ ...value, credential: { kind: 'environment', variable } })} />}{value.credential.kind === 'literal' && <TextField label="New literal credential" secret required value={value.credential.value} change={secret => change({ ...value, credential: { kind: 'literal', value: secret } })} />}<p>Credentials are never inherited from a shadowed Provider or read back as resolved values.</p></>}</UnitForm></section>;
  if (modelId) return <section aria-label="Model editor"><Button onClick={() => setModelId('')}>Back to catalog</Button><p>Replace this scope's complete Model definition. Unspecified fields use native defaults.</p><UnitForm<Model> key={`model:${modelId}`} title={`Model ${modelId}`} authored={authored.models?.[modelId] ?? undefined} blank={emptyModel()} revision={revision} save={save} mutation={authored => ({ kind: 'config', mutation: { unit: 'model', id: modelId, authored } })}>{(model, change) => <ModelEditor model={model} change={change} />}</UnitForm></section>;
  return <section aria-label="Providers & Models"><h3>Providers & Models</h3>
    <p>Each same-name Provider or Model is replaced as a complete object. Omitted fields use native defaults within that object.</p>
    {scope === 'workspace' && <p>Identities this Workspace does not override are listed from the native effective projection. Opening one authors nothing.</p>}
    <h4>Providers</h4><div className={css.rows}>{providers.map(entry => <SettingsCard key={entry.id} title={entry.id} meta={<><Badge>{credentialLabel(entry.authored ?? entry.effective)}</Badge>{ownership(entry)}</>} actions={<Button onClick={() => setProviderId(entry.id)}>{ownershipAction(entry, 'Provider')}</Button>}><p className={css.hint}>{(entry.authored ?? entry.effective)?.base_url}{overrideNote(entry)}</p></SettingsCard>)}</div>
    <div className={css.actions}><TextField label="New Provider identity" value={newProvider} change={setNewProvider} /><Button disabled={!newProvider || providers.some(entry => entry.id === newProvider)} onClick={() => { setProviderId(newProvider); setNewProvider(''); }}>Add Provider</Button></div>

    <h4>Models</h4><div className={css.rows}>{models.map(entry => { const model = entry.authored ?? entry.effective; return <SettingsCard key={entry.id} title={entry.id} meta={<><Badge>{model?.protocol}</Badge>{ownership(entry)}</>} actions={<Button onClick={() => setModelId(entry.id)}>{ownershipAction(entry, 'Model')}</Button>}><p className={css.hint}>{model?.provider} · {model?.id} · {model?.context_window} context · {model?.max_output_tokens} output{overrideNote(entry)}</p></SettingsCard>; })}</div>
    <div className={css.actions}><TextField label="New Model identity" value={newModel} change={setNewModel} /><Button disabled={!newModel || models.some(entry => entry.id === newModel)} onClick={() => { setModelId(newModel); setNewModel(''); }}>Add Model</Button></div>

  </section>;
}
/** The action truthfully names what opening this identity does in this scope:
 * editing an override this scope authors, or opening an inherited identity that
 * this scope may author an override for. */
function ownershipAction<T>(entry: CatalogEntry<T>, family: string) { return `${entry.authored ? 'Edit' : 'Override'} ${family} ${entry.id}`; }
function credentialLabel(provider: ProviderView | undefined) { return !provider ? 'Definition not resolved' : provider.credential.type === 'environment' ? `Environment: ${provider.credential.variable}` : 'Literal secret (redacted)'; }
export function ModelEditor({ model, change }: { model: Model; change: (value: Model) => void }) {
  return <details className={css.model} open><summary>{model.id || 'New model'}</summary><div className={css.grid}>
    <label>Wire model identity<input value={model.id} onChange={e => change({ ...model, id: e.target.value })} /></label>
    <label>Provider identity<input required value={model.provider} onChange={e => change({ ...model, provider: e.target.value })} /></label>
    <label>Protocol<select value={model.protocol} onChange={e => change({ ...model, protocol: e.target.value as Model['protocol'] })}>{(['openai_chat_completions', 'openai_responses', 'anthropic_messages'] as const).map(protocol => <option key={protocol}>{protocol}</option>)}</select></label>
    <label>Context window<input type="number" min="1" value={model.context_window} onChange={e => change({ ...model, context_window: e.target.value })} /></label>
    <label>Maximum output tokens<input type="number" min="1" value={model.max_output_tokens} onChange={e => change({ ...model, max_output_tokens: Number(e.target.value) })} /></label>
  </div><fieldset><legend>Explicit capabilities</legend>{(['tool_calls', 'reasoning'] as const).map(key => <label key={key}><input type="checkbox" checked={model.capabilities[key]} onChange={e => change({ ...model, capabilities: { ...model.capabilities, [key]: e.target.checked } })} />{key}</label>)}
    {(['input_modalities', 'output_modalities'] as const).map(key => <div key={key}>{key}{(['text', 'image', 'file'] as const).map(modality => <label key={modality}><input type="checkbox" checked={model.capabilities[key].includes(modality)} onChange={e => change({ ...model, capabilities: { ...model.capabilities, [key]: e.target.checked ? [...model.capabilities[key], modality] : model.capabilities[key].filter(value => value !== modality) } })} />{modality}</label>)}</div>)}
    </fieldset>
    <details><summary>Reasoning profiles</summary><label>Default profile<input value={model.reasoning?.default_profile ?? ''} onChange={e => change({ ...model, reasoning: { default_profile: e.target.value, profiles: model.reasoning?.profiles ?? {} } })} /></label>
      {Object.entries(model.reasoning?.profiles ?? {}).map(([id, profile]) => <div key={id}><label><input type="checkbox" checked={profile.enabled} onChange={e => change({ ...model, reasoning: { ...model.reasoning!, profiles: { ...model.reasoning!.profiles, [id]: { ...profile, enabled: e.target.checked } } } })} />{id}</label><RequestPolicy value={profile.request_params ?? {}} change={request_params => change({ ...model, reasoning: { ...model.reasoning!, profiles: { ...model.reasoning!.profiles, [id]: { ...profile, request_params } } } })} /><Button onClick={() => { const profiles = { ...model.reasoning!.profiles }; delete profiles[id]; change({ ...model, reasoning: { ...model.reasoning!, profiles } }); }}>Delete profile {id}</Button></div>)}
      <ProfileAdder add={id => change({ ...model, reasoning: { default_profile: model.reasoning?.default_profile ?? id, profiles: { ...model.reasoning?.profiles, [id]: { enabled: true, request_params: {} } } } })} /><Button onClick={() => change({ ...model, reasoning: null })}>Remove reasoning profiles</Button>
    </details><details><summary>Request defaults and protocol compatibility</summary><RequestPolicy value={model.request_params ?? {}} change={request_params => change({ ...model, request_params })} />
    <label>Chat reasoning replay<select value={model.compat?.chat_reasoning_replay ?? ''} onChange={e => change({ ...model, compat: { ...model.compat, chat_reasoning_replay: (e.target.value || null) as NonNullable<Model['compat']>['chat_reasoning_replay'] } })}><option value="">Unspecified</option><option value="omit">omit</option><option value="reasoning_content">reasoning_content</option><option value="reasoning">reasoning</option></select></label>
    <label>Chat output field<select value={model.compat?.chat_max_tokens_field ?? ''} onChange={e => change({ ...model, compat: { ...model.compat, chat_max_tokens_field: (e.target.value || null) as NonNullable<Model['compat']>['chat_max_tokens_field'] } })}><option value="">Unspecified</option><option value="max_tokens">max_tokens</option><option value="max_completion_tokens">max_completion_tokens</option></select></label>
    <label>Chat stream usage<select value={model.compat?.chat_stream_usage ?? ''} onChange={e => change({ ...model, compat: { ...model.compat, chat_stream_usage: (e.target.value || null) as NonNullable<Model['compat']>['chat_stream_usage'] } })}><option value="">Unspecified</option><option value="supported">supported</option><option value="unsupported">unsupported</option></select></label>
    <label>Chat tool protocol<select value={model.compat?.chat_tool_protocol ?? ''} onChange={e => change({ ...model, compat: { ...model.compat, chat_tool_protocol: (e.target.value || null) as NonNullable<Model['compat']>['chat_tool_protocol'] } })}><option value="">Unspecified</option><option value="native">native</option><option value="qwen_xml">qwen_xml</option></select></label>
    <label>Responses storage<select value={model.compat?.responses_storage ?? ''} onChange={e => change({ ...model, compat: { ...model.compat, responses_storage: (e.target.value || null) as NonNullable<Model['compat']>['responses_storage'] } })}><option value="">Unspecified</option><option value="stateless">stateless</option><option value="stored">stored</option></select></label>
    </details></details>;
}
function ProfileAdder({ add }: { add: (id: string) => void }) { const [id, setId] = useState(''); return <div className={css.actions}><input aria-label="New reasoning profile" value={id} onChange={e => setId(e.target.value)} /><Button disabled={!id} onClick={() => { add(id); setId(''); }}>Add profile</Button></div>; }
