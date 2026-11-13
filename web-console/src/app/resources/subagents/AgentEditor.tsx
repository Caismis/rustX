/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import { useEffect, useId, useRef } from 'react';
import type { NamedAgentProfileDocument, SourceSettings } from '../../../../../protocol/app-server/v79';
import type { AppServerClient } from '../../../client/app-server';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { IconChevronRightOutline14 } from '../../../presentation/primitives/icons';
import { Switch } from '../../../presentation/primitives/Switch';
import settings from '../../../presentation/settings/SettingsContent.module.css';
import { DefinitionNotice, UnitOutcomeNotice, UnitReset, useUnitEditing } from '../../settings/forms/bridge';
import { Choice, ConfirmAction } from '../../settings/primitives/aria';
import { AgentModelOptions, ModelProfileChoice, type ModelProfiles } from '../../settings/models/ModelsPage';
import { SettingsNotice } from '../../settings/SettingsFeedback';
import { SettingsScopeMenu } from '../../settings/SettingsScopeMenu';
import type { SettingsTarget } from '../../settings/projection';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { extensionEntries } from '../shared/inventory';
import css from './Subagents.module.css';
import { AgentToolFields, ManagedToolSelections, type LoadMcpTools } from './AgentToolFields';
import { AgentSkillFields } from './AgentSkillFields';

export type AgentDraft = NamedAgentProfileDocument & { name?: string };

interface Props {
  source: SourceSettings; target: SettingsTarget; host?: ProductHostWorkspaces; name?: string;
  models: string[]; profiles: ModelProfiles; initialDraft?: AgentDraft;
  client?: AppServerClient; discover?: boolean; editable?: boolean;
  loadMcpTools?: LoadMcpTools;
  onScopeChange?: (target: SettingsTarget, draft: AgentDraft) => void;
  onClose: () => void;
}

export function AgentEditor({ source, target, host, name, models, profiles, initialDraft, client, discover = true, editable = true, loadMcpTools, onScopeChange, onClose }: Props) {
  const tx = useTranslation();
  const scope = target.kind;
  const creating = name === undefined;
  const current = source.agents.find(agent => agent.scope === scope && agent.name === name);
  const unit = useUnitEditing<AgentDraft>({
    onSaved: onClose,
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

  const close = () => { unit.discard(); onClose(); };
  const value = unit.displayed;
  const identity = name ?? value.name?.trim() ?? '';
  const validationId = useId();
  const promptId = useId();
  const sections = useId();
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
  const canSubmit = editable && unit.admitted && !busy && !unit.reviewNeeded;
  const save = canSubmit && !invalid && !duplicate && unit.writable && (creating || unit.draft);
  const model = value.model ?? undefined;
  const modelChoices = [...new Set([...models, ...(model?.model ? [model.model] : [])])];
  const update = (changes: Partial<AgentDraft>) => unit.edit({ ...value, ...changes });
  const guidance = value.agents_md ?? {};
  const resourceScope = unit.definition === 'inherited' ? 'user' : scope;

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
          if (unit.draft) unit.submit();
          else unit.apply(value);
        }}>
        <fieldset className={css.body} disabled={!editable || busy}>
          {(unit.definition === 'inherited' || unit.definition === 'overriding' || unit.unparsed) && <DefinitionNotice unit={unit}/>}
          <SettingsNotice text={current?.source.diagnostic ?? undefined} />
          <fieldset className={css.fields} disabled={!unit.writable}>
            <section className={css.section} aria-labelledby={`${sections}-profile`}>
              <div className={css.sectionHead}>
                <h2 id={`${sections}-profile`}>{tx('resources:subagents.profile')}</h2>
                <div className={css.scope}>
                  <span>{tx('settings:catalog.scope')}</span>
                  {creating && onScopeChange ? <SettingsScopeMenu target={target} host={host} userLabel={tx('resources:scope.global')}
                    onSelect={next => { unit.discard(); onScopeChange(next, value); }} />
                    : <span className={css.scopeValue}>{unit.definition === 'inherited' || target.kind === 'user' ? tx('resources:scope.global') : target.displayName}</span>}
                </div>
              </div>
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
              <div className={css.prompt}>
                <label htmlFor={promptId}>{tx('resources:subagents.prompt')}</label>
                <textarea id={promptId} rows={4} value={value.instructions ?? ''} onChange={event => update({ instructions: event.target.value })} />
              </div>
              <div className={css.settingRow}>
                <span>{tx('resources:subagents.inject-guidance')}</span>
                <Switch label={tx('resources:subagents.inject-guidance')} checked={guidance.inherit ?? true}
                  onChange={inherit => update({ agents_md: { ...guidance, inherit } })} />
              </div>
            </section>
            <AgentToolFields source={source} scope={resourceScope}
              value={value} change={update} loadMcpTools={discover && unit.writable ? loadMcpTools : undefined} />
            <section className={css.section} aria-labelledby={`${sections}-advanced`}>
              <h2 id={`${sections}-advanced`}>{tx('settings:agent-page.advanced-options')}</h2>
              <AgentSkillFields client={client} source={source} scope={resourceScope} active={discover}
                value={value.skills ?? []} change={skills => update({ skills })} />
              {model && <AgentModelOptions value={model} change={next => update({ model: next })} models={models} profiles={profiles} />}
              <ManagedToolSelections tools={value.tools} change={tools => update({ tools })} />
              <label className={css.timeout}>{tx('settings:extension-detail.child-timeout-ms')}
                <input type="number" min="1" value={value.timeout_ms ?? ''} onChange={event => update({ timeout_ms: event.target.value || null })} />
              </label>
              <fieldset className={css.featureGroup}>
                <legend>{tx('settings:extension-detail.extensions-default-off')}</legend>
                {(['todo', 'agent_status'] as const).map(id => {
                  const label = tx(`resources:subagents.feature-${id}`);
                  return <div className={css.settingRow} key={id}>
                    <span>{label}</span>
                    <Switch label={label} checked={value.plugins?.[id]?.enabled ?? false}
                      onChange={enabled => update({ plugins: { ...value.plugins, [id]: { ...value.plugins?.[id], enabled } } })} />
                  </div>;
                })}
              </fieldset>
            </section>
          </fieldset>
        </fieldset>
        <UnitOutcomeNotice title={formName} outcome={unit.outcome} />
        <SettingsNotice text={unit.reviewNeeded && unit.outcome.kind !== 'conflict' ? tx(unit.committed && !unit.draft ? 'settings:forms.committed-changed-elsewhere' : 'settings:forms.changed-elsewhere') : undefined}/>
        <footer className={css.actions}>
          {unit.authoredPresent && <span data-removal={restoring ? 'override-removal' : 'authored-removal'}><ConfirmAction
            label={tx(restoring ? 'settings:bridge.use-global-default-value' : 'settings:bridge.remove-value', { p0: formName })}
            title={tx(restoring ? 'settings:bridge.use-the-global-default-for-value' : 'settings:bridge.remove-value-from-value-configuration', {
              p0: formName, p1: tx(scope === 'workspace' ? 'settings:extension-detail.workspace' : 'settings:extension-detail.user'),
            })}
            triggerText={tx(restoring ? 'settings:models.restore' : 'settings:copy.delete')}
            confirm={tx(restoring ? 'settings:bridge.use-global-default-value' : 'settings:bridge.remove-value', { p0: formName })}
            description={tx(restoring ? 'settings:forms.restore-help' : 'resources:subagents.delete-help')}
            settle={form} tone={restoring ? 'restore' : 'destructive'} disabled={!canSubmit}
            onConfirm={() => { if (canSubmit) unit.submit(true); }} /></span>}
          <div className={css.saveActions}>
            {unit.reviewNeeded && <Button size="sm" disabled={!editable || busy} onClick={unit.review}>{tx('settings:bridge.use-reviewed-revision')}</Button>}
            <Button variant="outline" size="sm" disabled={busy} onClick={close}>{tx('resources:cancel')}</Button>
            {unit.override ? <Button type="button" variant="primary" size="sm"
              aria-label={`${tx('settings:bridge.override')} ${formName} ${tx('settings:bridge.in-this-workspace')}`}
              disabled={!editable || busy} onClick={unit.override}>{tx('settings:forms.customize')}</Button>
              : <Button type="submit" variant="primary" size="sm" aria-label={`${tx('settings:bridge.save')} ${formName}`} disabled={!save}>
                {tx(busy ? 'settings:catalog.saving' : 'settings:bridge.save')}
              </Button>}
          </div>
        </footer>
      </form>
    </UnitReset.Provider>
  </div>;
}
