import { useTranslation } from '../../../locale/react';
import type { AgentProjectInstructionsDocument, RuntimeLayer, SourceSettings } from '../../../../../protocol/app-server/v77';
import { Switch } from '../../../presentation/primitives/Switch';
import settings from '../../../presentation/settings/SettingsContent.module.css';
import { UnitForm } from '../forms/bridge';
import { SettingsNotice } from '../SettingsFeedback';
import css from './AgentPage.module.css';

export function AgentPage({ config, globalGuidance }: {
  config?: { document: RuntimeLayer; revision: string } | undefined; globalGuidance?: SourceSettings['global_guidance'];
}) {
  const tx = useTranslation();
  const instructions = tx('settings:agent-page.global-instructions');
  return <section aria-label={tx('settings:agent-page.agent')}>
    <h3>{tx('settings:agent-page.agent')}</h3>

    {globalGuidance && <UnitForm<string> title={instructions} saveLabel={tx('settings:agent-page.save-instructions')}
      authored={globalGuidance.authored ?? undefined} blank="" revision={globalGuidance.revision} removable={false}
      mutation={value => ({ kind: 'global_guidance', document: value ?? '' })}>
      {(value, change) => <div className={css.instructions}>
        <code>{globalGuidance.path}</code>
        <textarea aria-label={instructions} value={value} disabled={!!globalGuidance.diagnostic} onChange={event => change(event.target.value)} />
        <SettingsNotice text={globalGuidance.diagnostic ? tx('settings:agent-page.instructions-unreadable') : undefined}/>
      </div>}
    </UnitForm>}

    {config && <UnitForm<AgentProjectInstructionsDocument> title={tx('settings:agent-page.project-guidance')} authored={config.document.agent?.agents_md ?? undefined}
      blank={{}} revision={config.revision} allowUnchangedDraft={false} mutation={authored => ({ kind: 'config', mutation: { unit: 'project_guidance', authored } })}>
      {(value, change) => <div className={css.guidanceRow}>
        <p className={settings.hint}>{tx('settings:agent-page.automatic-guidance-description')}</p>
        <Switch label={tx('settings:agent-page.automatic-guidance')} checked={value.inherit ?? true}
          onChange={inherit => change({ inherit })} />
      </div>}
    </UnitForm>}
  </section>;
}
