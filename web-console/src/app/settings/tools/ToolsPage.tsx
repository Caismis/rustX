import { SettingsNotice } from '../SettingsFeedback';
import { useTranslation } from '../../../locale/react';
import { useState } from 'react';
import type {
  ExecutionPermissionMode as ApprovalMode,
  RuntimeLayer, SourceMutation, SourceScope, SourceSettings, SourceToolSelection,
} from '../../../../../protocol/app-server/v80';
import { Button } from '../../../presentation/primitives/Button';
import { Switch } from '../../../presentation/primitives/Switch';
import { UnitForm } from '../forms/bridge';
import { Selection, TextField, nativeTools } from '../forms/controls';
import { Choice } from '../primitives/aria';
import { reachableIdentities, sourceView } from '../projection';
import { SettingsPageShell, SettingsGroup } from '../../../presentation/settings/SettingsPageShell';
import { PageScopeToolbar, PageRefreshButton } from '../ScopeToolbar';
import { WorkspaceFollowGlobal } from '../WorkspaceFollowGlobal';
import css from '../../../presentation/settings/SettingsContent.module.css';
import toolsCss from './ToolsPage.module.css';

/** The Tools & Permissions product page. */
export function ToolsPage({ source, document, scope, revision, bodyDisabled = false }: {
  source: SourceSettings; document: RuntimeLayer; scope: SourceScope; revision: string;
  bodyDisabled?: boolean;
}) {
  const tx = useTranslation();
  const resolved = source.resolved;
  const agent = document.agent;
  const [source_, setSource] = useState('');
  const [added, setAdded] = useState<string[]>([]);
  const authoredSources = agent?.tools?.sources ?? {};
  const sources = [...new Set([...reachableIdentities(scope, authoredSources, resolved?.agent?.tools?.sources), ...added])].filter(id => id.startsWith('python:'));
  // Only the sources this page owns count (python:*, same as sources and
  // workspaceRemovals); other grant kinds do not block Follow global.
  const hasWorkspaceOverrides = scope === 'workspace' && (
    document.approval_mode != null || agent?.tools?.builtin != null || Object.keys(authoredSources).some(id => id.startsWith('python:'))
  );
  const workspaceRemovals: SourceMutation[] = scope === 'workspace' ? [
    ...(document.approval_mode != null ? [{ kind: 'config', mutation: { unit: 'approval', authored: null } } as const] : []),
    ...(agent?.tools?.builtin != null ? [{ kind: 'config', mutation: { unit: 'native_tools', authored: null } } as const] : []),
    // Only the sources this page owns (python:*); other grant kinds keep
    // their authored selections when this page follows global again.
    ...Object.keys(authoredSources).filter(id => id.startsWith('python:')).map(id => ({ kind: 'config', mutation: { unit: 'source_tools', id, authored: null } } as const)),
  ] : [];
  const scopeNotice = sourceView(source, scope) === undefined ? tx('settings:tools-page.this-scope-has-no-source-view') : undefined;

  return (
    <SettingsPageShell
      className={`${css.settings} ${css.page}`}
      title={tx('settings:tools-page.tools-amp-permissions')}
      bodyDisabled={bodyDisabled}
      toolbar={<><PageScopeToolbar /><PageRefreshButton /></>}
    >
      <WorkspaceFollowGlobal scope={scope} hasOverrides={hasWorkspaceOverrides} authoredRemovals={workspaceRemovals} units={['approval', 'native_tools', 'source_tools']}>
        <SettingsGroup title={tx('settings:tools-page.approval-mode')}>
          <UnitForm<ApprovalMode> immediate
            title={tx('settings:tools-page.approval-mode')}
            legendHidden
            authored={document.approval_mode ?? undefined}
            blank="plan"
            revision={revision}
            mutation={authored => ({ kind: 'config', mutation: { unit: 'approval', authored } })}
          >
            {(value, change) => (
              <Choice
                hideLabel
                label={tx('settings:tools-page.approval-mode')}
                value={value}
                options={[['plan', tx('settings:copy.plan')], ['automatic', tx('settings:copy.automatic')], ['full_access', tx('settings:copy.full-access')]]}
                onChange={change}
              />
            )}
          </UnitForm>
        </SettingsGroup>

        <SettingsGroup title={tx('settings:extension-detail.native-tools')}>
          <UnitForm<string[]> immediate
            title={tx('settings:extension-detail.native-tools')}
            legendHidden
            authored={agent?.tools?.builtin ?? undefined}
            blank={[]}
            revision={revision}
            mutation={authored => ({ kind: 'config', mutation: { unit: 'native_tools', authored } })}
          >
            {(value, change) => (
              <div className={toolsCss.toolsGrid}>
                {nativeTools.map(tool => (
                  <div key={tool} className={toolsCss.toolTile}>
                    <code className={toolsCss.toolName}>{tool}</code>
                    <Switch
                      label={tool}
                      checked={value.includes(tool)}
                      onChange={on => change(on ? [...value, tool] : value.filter(entry => entry !== tool))}
                    />
                  </div>
                ))}
              </div>
            )}
          </UnitForm>
        </SettingsGroup>

        <SettingsGroup title={tx('settings:tools-page.tool-sources')}>
          {sources.map(id => (
            <UnitForm<SourceToolSelection> immediate
              key={id}
              title={tx('settings:extension-detail.source-value', { p0: id })}
              authored={agent?.tools?.sources?.[id] ?? undefined}
              blank={[]}
              revision={revision}
              mutation={authored => ({ kind: 'config', mutation: { unit: 'source_tools', id, authored } })}
            >
              {(value, change) => <Selection label={id} value={value} change={change} />}
            </UnitForm>
          ))}
          <div className={toolsCss.sourceAdder}>
            <TextField label={tx('settings:tools-page.source-identity')} value={source_} change={setSource} />
            <Button size="sm" variant="outline" disabled={!source_} onClick={() => { setAdded(current => [...current, `python:${source_}`]); setSource(''); }}>
              {tx('settings:tools-page.add-source-selection')}
            </Button>
          </div>
          {scopeNotice && <div className={toolsCss.sourceNotice}><SettingsNotice text={scopeNotice} /></div>}
        </SettingsGroup>
      </WorkspaceFollowGlobal>
    </SettingsPageShell>
  );
}
