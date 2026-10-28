/* Copyright (c) 2026 DeepSeek. MIT. Adapted typed request controls; see PROVENANCE.md. */
import { useContext, useId, useRef, useState, type RefObject } from 'react';
import type { Model, Modality } from '../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { UnitReset, type TypedUnitForm as TypedForm } from '../forms/bridge';
import { Bool, JsonObjectEditor, Numeric, NumericText, RequestParameters, Text } from '../forms/fields';
import { Advanced, Choice } from '../primitives/aria';
import css from '../../../presentation/settings/SettingsContent.module.css';
import cards from './ModelsPage.module.css';

const inputModalities: readonly Modality[] = ['text', 'image'];
export function ModelFields({ form }: { form: TypedForm<Model> }) {
  const tx = useTranslation();
  const resets = useContext(UnitReset);
  const modelParameters = useRef<HTMLDivElement>(null);
  return <>
    <div className={cards.basicFields}>
      <Text form={form} name="id" label={tx('settings:models-page.wire-model-identity')} required />
      <Protocol form={form} />
      <NumericText form={form} name="context_window" label={tx('settings:models-page.context-window')} />
      <Numeric form={form} name="max_output_tokens" label={tx('settings:models-page.maximum-output-tokens')} />
    </div>
    <Advanced title={tx('settings:models.advanced')}>
    <Text form={form} name="provider" label={tx('settings:models-page.provider-identity')} required />
    <InputModalities form={form} />
    <Bool form={form} name="capabilities.json_schema_output" label={tx('settings:models-page.json-schema-output')} />
    <Profiles key={resets} form={form} modelParameters={modelParameters} />
    <div ref={modelParameters}><ModelRequestParameters form={form} /></div>
    </Advanced>
  </>;
}

/** Changing API format drops overrides owned by the old protocol. Editing
 * ordinary fields keeps expert overrides authored in source configuration. */
function Protocol({ form }: { form: TypedForm<Model> }) {
  const tx = useTranslation();
  const Bound = form.Field as unknown as Field;
  return <Bound name="compat">{compat => <Bound name="protocol">{protocol => <Choice
    label={tx('settings:models-page.protocol')} value={protocol.state.value as string}
    options={[
      ['openai_chat_completions', tx('settings:models.api-chat')],
      ['openai_responses', tx('settings:models.api-responses')],
      ['anthropic_messages', tx('settings:models.api-anthropic')],
    ]}
    onChange={next => {
      if (next !== protocol.state.value) {
        compat.handleChange(undefined as never);
        protocol.handleChange(next as never);
      }
    }} />}</Bound>}</Bound>;
}

function InputModalities({ form }: { form: TypedForm<Model> }) {
  const tx = useTranslation();
  const Bound = form.Field as unknown as Field;
  return <Bound name="capabilities.input_modalities">{field => {
    const value = (field.state.value as Modality[] | undefined) ?? [];
    return <div className={cards.modalityGroup}><span>{tx('settings:models-page.input-modalities')}</span><div className={cards.chips}>{inputModalities.map(modality => <label className={cards.chip} key={modality}>
      <input type="checkbox" checked={value.includes(modality)}
        onChange={event => field.handleChange((event.target.checked ? [...value, modality] : value.filter(item => item !== modality)) as never)} />{tx(`settings:model-option.${modality}`)}
    </label>)}</div></div>;
  }}</Bound>;
}

type Field = (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
type ProfileMap = NonNullable<Model['profiles']>;

/** A Model's native request parameters, which exist only while the Model
 * declares no profiles: each profile then authors its own complete object. */
function ModelRequestParameters({ form }: { form: TypedForm<Model> }) {
  const tx = useTranslation();
  const Subscribe = form.Subscribe as unknown as (props: { selector: (state: { values: Model }) => boolean; children: (profiled: boolean) => React.ReactNode }) => React.ReactNode;
  return <Subscribe selector={state => Object.keys(state.values.profiles ?? {}).length > 0}>{profiled => profiled
    ? null
    : <RequestParameters form={form} name="request_params" label={tx('settings:models-page.model-request-parameters')} />}</Subscribe>;
}

/** General Model Profiles: named, complete and independent invocation
 * presets. A profile's name carries no meaning; its output default and native
 * parameters are exactly what it declares. Native Rust validates the declared
 * default, budgets within the hard maximum, and protected keys. */
function Profiles({ form, modelParameters }: { form: TypedForm<Model>; modelParameters: RefObject<HTMLDivElement | null> }) {
  const tx = useTranslation();
  const [adding, setAdding] = useState(false);
  const [identity, setIdentity] = useState('');
  const [selected, setSelected] = useState<string>();
  // Enter/Escape can focus the plus button before React removes the input;
  // its resulting blur must not commit the same entry again or undo cancel.
  const addingDraft = useRef(false);
  const panel = useRef<HTMLDivElement>(null);
  const chips = useRef<HTMLDivElement>(null);
  const errorId = useId();
  const Bound = form.Field as unknown as Field;
  return <Bound name="request_params">{params => <Bound name="default_profile">{fallback => <Bound name="profiles">{field => {
    const profiles = (field.state.value as ProfileMap | null | undefined) ?? {};
    const identities = Object.keys(profiles);
    const current = selected && Object.hasOwn(profiles, selected) ? selected : identities[0];
    const name = identity.trim();
    const duplicate = Object.hasOwn(profiles, name);
    const setProfiles = (next: ProfileMap) => field.handleChange((Object.keys(next).length ? next : undefined) as never);
    const update = (id: string, profile: ProfileMap[string]) => setProfiles({ ...profiles, [id]: profile });
    const focusAdd = () => chips.current?.querySelector<HTMLButtonElement>('[data-add-profile]')?.focus();
    // A JSON syntax error lives in the editor, not in the native form value.
    // Validate before replacing its panel so switching cannot discard that text
    // and silently save the last valid parameters instead.
    const canSwitch = () => {
      const container = identities.length ? panel.current : modelParameters.current;
      const invalid = container?.querySelector<HTMLInputElement>('input:invalid');
      return invalid ? invalid.reportValidity() : true;
    };
    const finishAdd = () => {
      if (!addingDraft.current) return;
      if (!name) { addingDraft.current = false; setAdding(false); return; }
      if (duplicate || !canSwitch()) return;
      addingDraft.current = false;
      // The first profile starts from the Model's own parameters, which a
      // Model with profiles no longer declares.
      const seed = identities.length ? {} : (params.state.value as ProfileMap[string]['request_params'] | null | undefined) ?? {};
      if (!identities.length) { params.handleChange(undefined as never); fallback.handleChange(name as never); }
      update(name, { request_params: seed });
      setSelected(name);
      setIdentity('');
      setAdding(false);
      focusAdd();
    };
    return <fieldset className={cards.profiles}><legend>{tx('settings:models-page.model-profiles')}</legend>
      <div className={cards.profileChips} ref={chips}>
        {identities.map(id => <div className={cards.profileChip} key={id} data-selected={id === current}>
          <Button size="sm" aria-label={tx('settings:models-page.profile-value', { p0: id })} aria-pressed={id === current}
            onClick={() => { if (id !== current && canSwitch()) setSelected(id); }}>{id}</Button>
          <Button size="sm" aria-label={`${tx('settings:models-page.delete-profile')} ${id}`} onClick={() => {
            const next = { ...profiles }; delete next[id];
            const remaining = Object.keys(next);
            if (fallback.state.value === id) fallback.handleChange((remaining[0] ?? undefined) as never);
            if (id === current) setSelected(remaining[0]);
            setProfiles(next);
            focusAdd();
          }}>×</Button>
        </div>)}
        {adding && <input autoFocus aria-label={tx('settings:models-page.new-profile-identity')} className={cards.profileName}
          value={identity} aria-invalid={duplicate || undefined} aria-describedby={duplicate ? errorId : undefined}
          onChange={event => setIdentity(event.target.value)} onBlur={finishAdd} onKeyDown={event => {
            if (event.key === 'Enter') { event.preventDefault(); finishAdd(); }
            if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); addingDraft.current = false; setAdding(false); setIdentity(''); focusAdd(); }
          }} />}
        <Button data-add-profile size="sm" variant="outline" className={cards.profileAdd} aria-label={tx('settings:models-page.add-profile')}
          onClick={() => { if (canSwitch()) { addingDraft.current = true; setIdentity(''); setAdding(true); } }}>+</Button>
      </div>
      {adding && duplicate && <p id={errorId} role="alert" className={css.error}>{name} {tx('settings:fields.is-already-listed')}</p>}
      <div ref={panel}>
        {current && <ProfileFields key={current} id={current} profile={profiles[current]} change={next => update(current, next)} />}
      </div>
      {identities.length > 0 && <Choice label={tx('settings:models-page.default-profile')} value={(fallback.state.value as string | null | undefined) ?? ''}
        options={[['', tx('settings:copy.unspecified')], ...identities.map(id => [id, id] as const)]}
        onChange={id => fallback.handleChange((id || undefined) as never)} />}
      {!identities.length && <p className={css.hint}>{tx('settings:models-page.no-profiles')}</p>}
    </fieldset>;
  }}</Bound>}</Bound>}</Bound>;
}

function ProfileFields({ id, profile, change }: { id: string; profile: ProfileMap[string]; change: (next: ProfileMap[string]) => void }) {
  const tx = useTranslation();
  return <div role="group" aria-label={tx('settings:models-page.profile-value', { p0: id })} data-profile={id}>
    <JsonObjectEditor label={tx('settings:models-page.profile-parameters')} value={{ ...profile }} optional={false}
      hint={tx('settings:models-page.profile-parameters-hint')}
      change={next => change((next ?? {}) as ProfileMap[string])} />
  </div>;
}
