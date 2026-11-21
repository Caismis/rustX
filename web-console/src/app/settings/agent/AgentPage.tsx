import type { ReactNode } from 'react';
import { useTranslation } from '../../../locale/react';
import type { AgentProjectInstructionsDocument, RuntimeLayer, SourceSettings } from '../../../../../protocol/app-server/v80';
import { Switch } from '../../../presentation/primitives/Switch';
import { UnitForm } from '../forms/bridge';
import { SettingsNotice } from '../SettingsFeedback';
import { SettingsPageShell, SettingsGroup } from '../../../presentation/settings/SettingsPageShell';
import { PageTransactionFooter } from '../forms/bridge';
import { PageScopeToolbar, PageRefreshButton } from '../ScopeToolbar';
import settings from '../../../presentation/settings/SettingsContent.module.css';
import css from './AgentPage.module.css';

/** The Agent product page: the AGENTS.md instruction document and the
 * project-guidance inherit switch. Both stage into the shared page
 * transaction; the footer is the only commit. */
export function AgentPage({ config, globalGuidance, closed }: {
  config?: { document: RuntimeLayer; revision: string } | undefined; globalGuidance?: SourceSettings['global_guidance'];
  /** What stands in for the closed semantic units while `rustx.toml` cannot
   * be authored (malformed) — the notice and the repair form. */
  closed?: ReactNode;
}) {
  const tx = useTranslation();
  const instructions = tx('settings:agent-page.global-instructions');
  return <SettingsPageShell className={`${settings.settings} ${settings.page}`} title={tx('settings:agent-page.agent')}
    toolbar={<><PageScopeToolbar /><PageRefreshButton /></>} footer={<PageTransactionFooter />}>
    {globalGuidance && <SettingsGroup title={instructions} description={<code>{globalGuidance.path}</code>}>
      <UnitForm<string> title={instructions}
        authored={globalGuidance.authored ?? undefined} blank="" revision={globalGuidance.revision} removable={false}
        mutation={value => ({ kind: 'global_guidance', document: value ?? '' })}>
        {(value, change) => <div className={css.instructions}>
          <textarea aria-label={instructions} value={value} disabled={!!globalGuidance.diagnostic} onChange={event => change(event.target.value)} />
          <SettingsNotice text={globalGuidance.diagnostic ? tx('settings:agent-page.instructions-unreadable') : undefined} />
        </div>}
      </UnitForm>
    </SettingsGroup>}

    {!config && closed}

    {config && <SettingsGroup title={tx('settings:agent-page.project-guidance')} description={tx('settings:agent-page.automatic-guidance-description')}>
      <UnitForm<AgentProjectInstructionsDocument> title={tx('settings:agent-page.project-guidance')} legendHidden authored={config.document.agent?.agents_md ?? undefined}
        blank={{}} revision={config.revision} allowUnchangedDraft={false} mutation={authored => ({ kind: 'config', mutation: { unit: 'project_guidance', authored } })}>
        {(value, change) => <Switch label={tx('settings:agent-page.automatic-guidance')} checked={value.inherit ?? true}
          onChange={inherit => change({ inherit })} />}
      </UnitForm>
    </SettingsGroup>}
  </SettingsPageShell>;
}
