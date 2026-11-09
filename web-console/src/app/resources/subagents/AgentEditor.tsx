/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import { useEffect, useId, useRef } from 'react';
import type { AgentProfileDocument, SourceSettings } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { IconChevronRightOutline14 } from '../../../presentation/primitives/icons';
import { Switch } from '../../../presentation/primitives/Switch';
import settings from '../../../presentation/settings/SettingsContent.module.css';
import { DefinitionNotice, UnitOutcomeNotice, UnitReset, useUnitEditing } from '../../settings/forms/bridge';
import { Names, Selection } from '../../settings/forms/controls';
import { Advanced, Choice, ConfirmAction } from '../../settings/primitives/aria';
import { AgentModelOptions, ModelProfileChoice, type ModelProfiles } from '../../settings/models/ModelsPage';
import { SettingsNotice } from '../../settings/SettingsFeedback';
import { SettingsScopeMenu } from '../../settings/SettingsScopeMenu';
import type { SettingsTarget } from '../../settings/projection';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { extensionEntries } from '../shared/inventory';
import css from './Subagents.module.css';
import { AgentToolFields } from './AgentToolFields';

export type AgentDraft = AgentProfileDocument & { name?: string };

interface Props {
  source: SourceSettings; target: SettingsTarget; host?: ProductHostWorkspaces; name?: string;
  models: string[]; profiles: ModelProfiles; initialDraft?: AgentDraft;
  onScopeChange?: (target: SettingsTarget, draft: AgentDraft) => void;
  onClose: () => void;
}

export function AgentEditor({ source, target, host, name, models, profiles, initialDraft, onScopeChange, onClose }: Props) {
  const tx = useTranslation();
  const scope = target.kind;
  const creating = name === undefined;
  const current = source.agents.find(agent => agent.scope === scope && agent.name === name);
  const unit = useUnitEditing<AgentDraft>({
    draftKey: creating ? 'create-subagent' : undefined,
    authoredPresent: current !== undefined,
    authored: current?.source.authored ?? undefined,
    blank: initialDraft ?? {},
    revision: current?.source.revision ?? source.absent_resource_revision,
    mutation: value => {
      const { name: draftName, ...authored } = value ?? {};
      return { kind: 'agent', name: name ?? draftName?.trim() ?? '', authored: value === null ? null : authored };
    },
  });
  const form = useRef<HTMLFormElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus({ preventScroll: true }); }, []);
  const submitted = useRef(false);
  useEffect(() => {
    if (submitted.current && unit.committed && !unit.awaitingObservation) {
      submitted.current = false;
      onClose();
    }
  }, [unit.committed, unit.awaitingObservation, onClose]);

  const close = () => { unit.discard(); onClose(); };
  const value = unit.displayed;
  const identity = name ?? value.name?.trim() ?? '';
  const validationId = useId();
  const promptId = useId();
  const invalid = creating && !/^[a-z][a-z0-9_-]{0,63}$/.test(identity);
  const duplicate = creating && !!identity && (
    source.agents.some(agent => agent.name === identity) ||
    extensionEntries(source, scope, 'agent').some(entry => entry.name === identity)
  );
  const nameError = duplicate ? tx('resources:subagents.duplicate')
    : invalid && value.name ? tx('resources:subagents.name-help') : undefined;
  const busy = unit.busy || unit.awaitingObservation;
  const restoring = scope === 'workspace' && !!unit.shadowed;
  const title = tx(creating ? 'resources:subagents.new' : 'resources:subagents.edit');
  const formName = creating ? title : tx('settings:extension-detail.agent-value', { p0: name ?? '' });
  const save = !invalid && !duplicate && unit.admitted && unit.writable && !busy && (creating || unit.draft);
  const model = value.model ?? undefined;
  const modelChoices = [...new Set([...models, ...(model?.model ? [model.model] : [])])];
  const update = (changes: Partial<AgentDraft>) => unit.edit({ ...value, ...changes });
  const guidance = value.agents_md ?? {};

  return <div className={css.editor}>
    <nav className={css.breadcrumb} aria-label={tx('resources:subagents.navigation')}>
      <Button variant="ghost" size="sm" disabled={busy} onClick={close}>{tx('resources:subagents.title')}</Button>
      <IconChevronRightOutline14 aria-hidden="true" />
      <span aria-current="page">{name ?? title}</span>
    </nav>
    <header className={css.heading}>
      <h1 ref={heading} tabIndex={-1}>{title}</h1>
      <p>{tx(creating ? 'resources:subagents.create-help' : 'resources:subagents.edit-help')}</p>
    </header>
    <UnitReset.Provider value={unit.resets}>
      <form ref={form} tabIndex={-1} aria-label={formName} aria-busy={busy} className={`${settings.settings} ${css.form}`}
        data-definition={unit.definition} data-authored-value={unit.authoredPresent ? unit.unparsed ? 'unparsed' : 'parsed' : undefined}
        onSubmit={event => {
          event.preventDefault();
          if (!save) return;
          submitted.current = true;
          if (unit.draft) unit.submit();
          else unit.apply(value);
        }}>
        <fieldset className={css.body} disabled={busy}>
          <div className={css.scope}>
            <span>{tx('settings:catalog.scope')}</span>
            {creating && onScopeChange ? <SettingsScopeMenu target={target} host={host} userLabel={tx('resources:scope.global')}
              onSelect={next => { unit.discard(); onScopeChange(next, value); }} />
              : <span className={css.scopeValue}>{unit.definition === 'inherited' || target.kind === 'user' ? tx('resources:scope.global') : target.displayName}</span>}
          </div>
          {(unit.definition === 'inherited' || unit.definition === 'overriding' || unit.unparsed) && <DefinitionNotice unit={unit}/>}
          <SettingsNotice text={current?.source.diagnostic ?? undefined} />
          <fieldset className={css.fields} disabled={!unit.writable}>
            <div className={css.identity}>
              <div>
                <label>{tx('resources:subagents.name')}
                  <input value={name ?? value.name ?? ''} readOnly={!creating} maxLength={64} spellCheck={false}
                    aria-invalid={nameError ? true : undefined} aria-describedby={nameError ? validationId : undefined}
                    onChange={event => update({ name: event.target.value })} />
                </label>
                {nameError && <span id={validationId} className={css.validation}>{nameError}</span>}
              </div>
              <Choice label={tx('settings:models-page.model')} value={model?.model ?? ''}
                options={[['', tx('resources:subagents.inherit-model')], ...modelChoices.map(id => [id, id] as const)]}
                onChange={id => {
                  const { profile: _profile, ...rest } = model ?? {};
                  update({ model: id ? { ...rest, model: id } : null });
                }} />
              {model?.model && <ModelProfileChoice label={tx('settings:models-page.profile')} profile={model.profile} profiles={profiles[model.model] ?? []}
                onChange={profile => {
                  const { profile: _profile, ...rest } = model;
                  update({ model: profile ? { ...rest, profile } : rest });
                }} />}
            </div>
            <label>{tx('settings:extension-detail.description')}
              <input value={value.description ?? ''} onChange={event => update({ description: event.target.value })} />
            </label>
            <AgentToolFields key={unit.resets} value={value.tools} change={tools => update({ tools })} />
            <div className={css.prompt}>
              <label htmlFor={promptId}>{tx('resources:subagents.prompt')}</label>
              <textarea id={promptId} rows={3} value={value.instructions ?? ''} onChange={event => update({ instructions: event.target.value })} />
            </div>
            <div className={css.guidance}>
              <span>{tx('resources:subagents.inject-guidance')}</span>
              <Switch label={tx('resources:subagents.inject-guidance')} checked={guidance.inherit ?? true}
                onChange={inherit => update({ agents_md: { ...guidance, inherit } })} />
            </div>
            <Advanced title={tx('settings:agent-page.advanced-options')}>
              {model && <AgentModelOptions value={model} change={next => update({ model: next })} models={models} profiles={profiles} />}
              <Names label={tx('settings:extension-detail.delegated-agents')} value={value.agents ?? []} change={agents => update({ agents })} />
              <Names label={tx('settings:extension-detail.delegated-workflows')} value={value.workflows ?? []} change={workflows => update({ workflows })} />
              <Selection label={tx('settings:extension-detail.skill-prompt-visibility')} value={value.skills ?? []} change={skills => update({ skills })} />
              <Names label={tx('settings:extension-detail.project-guidance-files')} value={guidance.files ?? []}
                change={files => update({ agents_md: { ...guidance, files } })} />
              <label>{tx('settings:extension-detail.child-timeout-ms')}
                <input type="number" min="1" value={value.timeout_ms ?? ''} onChange={event => update({ timeout_ms: event.target.value || null })} />
              </label>
              <fieldset>
                <legend>{tx('settings:extension-detail.extensions-default-off')}</legend>
                {(['todo', 'goal', 'agent_status'] as const).map(id => <label key={id}>
                  <input type="checkbox" checked={value.plugins?.[id]?.enabled ?? false}
                    onChange={event => update({ plugins: { ...value.plugins, [id]: { ...value.plugins?.[id], enabled: event.target.checked } } })} />
                  {id === 'agent_status' ? tx('settings:extension-detail.agent-status') : id}
                </label>)}
              </fieldset>
            </Advanced>
          </fieldset>
          <UnitOutcomeNotice title={formName} outcome={unit.outcome} />
          <SettingsNotice text={unit.reviewNeeded && unit.outcome.kind !== 'conflict' ? tx('settings:forms.changed-elsewhere') : undefined}/>
          <footer className={css.actions}>
            {unit.authoredPresent && <span data-removal={restoring ? 'override-removal' : 'authored-removal'}><ConfirmAction
              label={tx(restoring ? 'settings:bridge.use-global-default-value' : 'settings:bridge.remove-value', { p0: formName })}
              title={tx(restoring ? 'settings:bridge.use-the-global-default-for-value' : 'settings:bridge.remove-value-from-value-configuration', {
                p0: formName, p1: tx(scope === 'workspace' ? 'settings:extension-detail.workspace' : 'settings:extension-detail.user'),
              })}
              triggerText={tx(restoring ? 'settings:models.restore' : 'settings:copy.delete')}
              confirm={tx(restoring ? 'settings:bridge.use-global-default-value' : 'settings:bridge.remove-value', { p0: formName })}
              description={tx(restoring ? 'settings:forms.restore-help' : 'resources:subagents.delete-help')}
              settle={form} tone={restoring ? 'restore' : 'destructive'} disabled={!unit.admitted}
              onConfirm={() => { submitted.current = true; unit.submit(true); }} /></span>}
            <div className={css.saveActions}>
              {unit.reviewNeeded && <Button type="button" size="sm" onClick={unit.review}>{tx('settings:bridge.use-reviewed-revision')}</Button>}
              {unit.override ? <Button type="button" variant="primary" size="sm"
                aria-label={`${tx('settings:bridge.override')} ${formName} ${tx('settings:bridge.in-this-workspace')}`}
                onClick={unit.override}>{tx('settings:forms.customize')}</Button>
                : <Button type="submit" variant="primary" size="sm" aria-label={`${tx('settings:bridge.save')} ${formName}`} disabled={!save}>
                  {tx(busy ? 'settings:catalog.saving' : 'settings:bridge.save')}
                </Button>}
              <Button type="button" size="sm" onClick={close}>{tx('resources:cancel')}</Button>
            </div>
          </footer>
        </fieldset>
      </form>
    </UnitReset.Provider>
  </div>;
}
