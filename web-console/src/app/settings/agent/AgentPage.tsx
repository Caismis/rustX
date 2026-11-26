import type { ReactNode } from 'react';
import { useTranslation } from '../../../locale/react';
import type { AgentProjectInstructionsDocument, RuntimeLayer, SourceMutation, SourceScope, SourceSettings } from '../../../../../protocol/app-server/v80';
import { Switch } from '../../../presentation/primitives/Switch';
import { UnitForm } from '../forms/bridge';
import { SettingsNotice } from '../SettingsFeedback';
import { SettingsPageShell, SettingsGroup } from '../../../presentation/settings/SettingsPageShell';
import { PageScopeToolbar, PageRefreshButton } from '../ScopeToolbar';
import { WorkspaceFollowGlobal } from '../WorkspaceFollowGlobal';
import settings from '../../../presentation/settings/SettingsContent.module.css';
import { Textarea } from '@/components/ui/textarea';

/** The Agent product page: the AGENTS.md instruction document and the
 * project-guidance inherit switch. Both stage into the shared page
 * transaction; the footer is the only commit. */
export function AgentPage({ scope = 'user', config, globalGuidance, closed, bodyDisabled = false }: {
  scope?: SourceScope;
  config?: { document: RuntimeLayer; revision: string } | undefined;
  globalGuidance?: SourceSettings['global_guidance'];
  /** What stands in for the closed semantic units while `rustx.toml` cannot
   * be authored (malformed) — the notice and the repair form. */
  closed?: ReactNode;
  bodyDisabled?: boolean;
}) {
  const tx = useTranslation();
  const instructions = tx('settings:agent-page.global-instructions');
  const hasWorkspaceOverrides = scope === 'workspace' && config?.document.agent?.agents_md != null;
  const workspaceRemovals: SourceMutation[] = hasWorkspaceOverrides
    ? [{ kind: 'config', mutation: { unit: 'project_guidance', authored: null } }]
    : [];

  return (
    <SettingsPageShell
      className={`${settings.settings} ${settings.page}`}
      title={tx('settings:agent-page.agent')}
      bodyDisabled={bodyDisabled}
      toolbar={<><PageScopeToolbar /><PageRefreshButton /></>}
    >
      {globalGuidance && (
        <SettingsGroup title={instructions} description={<code>{globalGuidance.path}</code>}>
          <UnitForm<string> immediate
            title={instructions}
            legendHidden
            authored={globalGuidance.authored ?? undefined}
            blank=""
            revision={globalGuidance.revision}
            removable={false}
            mutation={value => ({ kind: 'global_guidance', document: value ?? '' })}
          >
            {(value, change) => (
              <div className="grid gap-[8px] [&_code]:[color:var(--dsw-alias-label-secondary)] [&_code]:text-[12px] [&_code]:wrap-anywhere [&_textarea]:min-h-[180px]">
                <Textarea
                  aria-label={instructions}
                  value={value}
                  disabled={!!globalGuidance.diagnostic}
                  onChange={event => change(event.target.value)}
                />
                <SettingsNotice text={globalGuidance.diagnostic ? tx('settings:agent-page.instructions-unreadable') : undefined} />
              </div>
            )}
          </UnitForm>
        </SettingsGroup>
      )}

      {!config && closed}

      {config && (
        <WorkspaceFollowGlobal scope={scope} hasOverrides={hasWorkspaceOverrides} authoredRemovals={workspaceRemovals} units={['project_guidance']}>
          <SettingsGroup
            title={tx('settings:agent-page.project-guidance')}
            description={tx('settings:agent-page.automatic-guidance-description')}
          >
            <UnitForm<AgentProjectInstructionsDocument> immediate
              title={tx('settings:agent-page.project-guidance')}
              legendHidden
              authored={config.document.agent?.agents_md ?? undefined}
              blank={{}}
              revision={config.revision}
              mutation={authored => ({ kind: 'config', mutation: { unit: 'project_guidance', authored } })}
            >
              {(value, change) => (
                <Switch
                  label={tx('settings:agent-page.automatic-guidance')}
                  checked={value.inherit ?? true}
                  onChange={inherit => change({ inherit })}
                />
              )}
            </UnitForm>
          </SettingsGroup>
        </WorkspaceFollowGlobal>
      )}
    </SettingsPageShell>
  );
}
