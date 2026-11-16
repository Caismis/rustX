import { SettingsNotice } from '../SettingsFeedback';
import { useTranslation } from '../../../locale/react';
import { useState } from 'react';
import type {
  ExecutionPermissionMode as ApprovalMode,
  RuntimeLayer, SourceScope, SourceSettings, SourceToolSelection,
} from '../../../../../protocol/app-server/v78';
import { Button } from '../../../presentation/primitives/Button';
import { UnitForm } from '../forms/bridge';
import { CheckboxList, Selection, TextField, nativeTools } from '../forms/controls';
import { Choice } from '../primitives/aria';
import { reachableIdentities, sourceView } from '../projection';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The Tools & Permissions product page.
 *
 * It answers exactly one user question — *what may the root Agent use, and
 * under what permission mode?* Mode selection and built-in Tool availability
 * remain here. MCP three-mode resource overrides live on the MCP definition;
 * Skills and named Agents own their availability controls in their modules.
 *
 * Every control keeps its exact native identity and its exact native
 * vocabulary. `all`, an explicit list of identities, an explicit empty list and
 * an absent unit are four different authored values and are never collapsed
 * into one browser state.
 *
 * There is no Session permission overlay here and no shortcut that writes User
 * or Workspace policy from a Session surface. Execution-time approval and
 * ask-user interactions remain a separate runtime concern and are untouched. */
export function ToolsPage({ source, document, scope, revision }: {
  source: SourceSettings; document: RuntimeLayer; scope: SourceScope; revision: string;
}) {
  const tx = useTranslation();
  const resolved = source.resolved;
  const agent = document.agent;
  const [source_, setSource] = useState('');
  const [added, setAdded] = useState<string[]>([]);
  const sources = [...new Set([...reachableIdentities(scope, agent?.tools?.sources, resolved?.agent?.tools?.sources), ...added])].filter(id => id.startsWith('python:'));
  return <section aria-label={tx('settings:tools-page.tools-permissions')}>
    <h3>{tx('settings:tools-page.tools-amp-permissions')}</h3>

    <UnitForm<ApprovalMode> title={tx('settings:tools-page.approval-mode')} authored={document.approval_mode ?? undefined} blank="plan" revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'approval', authored } })}>
      {(value, change) => <Choice label={tx('settings:tools-page.approval-mode')} value={value}
        options={[['plan', tx('settings:copy.plan')], ['automatic', tx('settings:copy.automatic')], ['full_access', tx('settings:copy.full-access')]]} onChange={change} />}
    </UnitForm>

    <UnitForm<string[]> title={tx('settings:extension-detail.native-tools')} authored={agent?.tools?.builtin ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'native_tools', authored } })}>
      {(value, change) => <>
        <CheckboxList label={tx('settings:tools-page.explicit-whitelist')} values={nativeTools} selected={value} change={change} />
      </>}
    </UnitForm>

    <h4>{tx('settings:tools-page.tool-sources')}</h4>
    {sources.map(id => <UnitForm<SourceToolSelection> key={id} title={tx('settings:extension-detail.source-value', { p0: id })}
      authored={agent?.tools?.sources?.[id] ?? undefined} blank={[]} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'source_tools', id, authored } })}>
      {(value, change) => <Selection label={id} value={value} change={change} />}
    </UnitForm>)}
    <div className={css.actions}>
      <TextField label={tx('settings:tools-page.source-identity')} value={source_} change={setSource} />
      <Button disabled={!source_} onClick={() => { setAdded(current => [...current, `python:${source_}`]); setSource(''); }}>{tx('settings:tools-page.add-source-selection')}</Button>
    </div>

    <SettingsNotice text={sourceView(source, scope) === undefined ? tx('settings:tools-page.this-scope-has-no-source-view') : undefined}/>
  </section>;
}
