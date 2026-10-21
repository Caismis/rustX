/* Copyright (c) 2026 DeepSeek. MIT. Adapted typed request controls; see PROVENANCE.md. */
import { useState } from 'react';
import type { Model, Modality } from '../../../../../protocol/app-server/v51';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import type { TypedUnitForm as TypedForm } from '../forms/bridge';
import { TextField } from '../forms/controls';
import { Enum, Numeric, NumericText, RequestParameters, RequestParamsEditor, Text } from '../forms/fields';
import { Advanced, Choice } from '../primitives/aria';
import css from '../../../presentation/settings/SettingsContent.module.css';
import cards from './ModelsPage.module.css';

const modalities: readonly Modality[] = ['text', 'image', 'file'];
export function ModelFields({ form }: { form: TypedForm<Model> }) {
  const tx = useTranslation();
  return <>
    <div className={cards.basicFields}>
      <Text form={form} name="id" label={tx('settings:models-page.wire-model-identity')} required />
      <Enum form={form} name="protocol" label={tx('settings:models-page.protocol')}
        options={[['openai_chat_completions', tx('settings:models.api-chat')], ['openai_responses', tx('settings:models.api-responses')], ['anthropic_messages', tx('settings:models.api-anthropic')]]} />
      <NumericText form={form} name="context_window" label={tx('settings:models-page.context-window')} />
      <Numeric form={form} name="max_output_tokens" label={tx('settings:models-page.maximum-output-tokens')} />
    </div>
    <Advanced title={tx('settings:models.advanced')}>
    <Text form={form} name="provider" label={tx('settings:models-page.provider-identity')} required />
    <fieldset className={cards.capabilities}><legend>{tx('settings:models-page.explicit-capabilities')}</legend>
      <div className={cards.chips}>
        <CapabilityChip form={form} name="tool_calls" label={tx('settings:models-page.tool-calls')} />
        <CapabilityChip form={form} name="reasoning" label={tx('settings:models-page.reasoning')} />
      </div>
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
    </Advanced>
  </>;
}

function CapabilityChip({ form, name, label }: { form: TypedForm<Model>; name: 'tool_calls' | 'reasoning'; label: string }) {
  const Bound = form.Field as unknown as Field;
  return <Bound name={`capabilities.${name}`}>{field => <label className={cards.chip}>
    <input type="checkbox" checked={!!field.state.value} onChange={event => field.handleChange(event.target.checked as never)} />{label}
  </label>}</Bound>;
}

function ModalitySet({ form, name, label }: { form: TypedForm<Model>; name: 'capabilities.input_modalities' | 'capabilities.output_modalities'; label: string }) {
  const tx = useTranslation();
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <Field name={name}>{field => {
    const value = (field.state.value as Modality[] | undefined) ?? [];
    return <div className={cards.modalityGroup}><span>{label}</span><div className={cards.chips}>{modalities.map(modality => <label className={cards.chip} key={modality}>
      <input type="checkbox" checked={value.includes(modality)}
        onChange={event => field.handleChange((event.target.checked ? [...value, modality] : value.filter(item => item !== modality)) as never)} />{tx(`settings:model-option.${modality}`)}
    </label>)}</div></div>;
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
