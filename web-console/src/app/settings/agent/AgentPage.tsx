import { useTranslation } from '../../../locale/react';
import type { AgentProjectInstructionsDocument, RuntimeLayer } from '../../../../../protocol/app-server/v77';
import { Switch } from '../../../presentation/primitives/Switch';
import settings from '../../../presentation/settings/SettingsContent.module.css';
import { UnitForm } from '../forms/bridge';
import { Names } from '../forms/controls';
import { Advanced } from '../primitives/aria';
import css from './AgentPage.module.css';

export function AgentPage({ document, revision }: { document: RuntimeLayer; revision: string }) {
  const tx = useTranslation();
  const instructions = tx('settings:agent-page.instructions');
  return <section aria-label={tx('settings:agent-page.agent')}>
    <h3>{tx('settings:agent-page.agent')}</h3>

    <UnitForm<string> title={instructions} authored={document.agent?.instructions ?? undefined} blank="" revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'instructions', authored } })}>
      {(value, change) => <textarea aria-label={instructions} value={value} onChange={event => change(event.target.value)} />}
    </UnitForm>

    <UnitForm<AgentProjectInstructionsDocument> title={tx('settings:agent-page.project-guidance')} authored={document.agent?.agents_md ?? undefined}
      blank={{}} revision={revision} mutation={authored => ({ kind: 'config', mutation: { unit: 'project_guidance', authored } })}>
      {(value, change) => <>
        <div className={css.guidanceRow}>
          <p className={settings.hint}>{tx('settings:agent-page.automatic-guidance-description')}</p>
          <Switch label={tx('settings:agent-page.automatic-guidance')} checked={value.inherit ?? true}
            onChange={inherit => change({ ...value, inherit })} />
        </div>
        <Advanced title={tx('settings:agent-page.advanced-options')}>
          <Names label={tx('settings:agent-page.guidance-files')} value={value.files ?? []} change={files => change({ ...value, files })} />
          <p className={settings.hint}>{tx('settings:agent-page.guidance-files-description')}</p>
        </Advanced>
      </>}
    </UnitForm>
  </section>;
}
