import { useTranslation } from '../../../locale/react';
import type { AgentProjectInstructionsDocument, RuntimeLayer, SourceScope } from '../../../../../protocol/app-server/v23';
import { UnitForm } from '../forms/bridge';
import { Names, TextField } from '../forms/controls';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The Agent product page.
 *
 * It answers exactly one user question — *who is the root Agent, and how is it
 * guided?* — and nothing else. What the root Agent may **use** is the Tools &
 * Permissions page; the definitions of **named** Agents are resources managed
 * under Extensions.
 *
 * Grouping them here would be a presentation convenience, not a native truth:
 * a named Agent profile is an independent resource document, and the root
 * Agent's delegation allowlist is a separate `rustx.toml` semantic unit. They
 * stay separate mutations with separate settlements, so they stay on separate
 * pages. */
export function AgentPage({ document, scope, revision }: { document: RuntimeLayer; scope: SourceScope; revision: string }) {
  const tx = useTranslation();
  return <section aria-label={tx('settings:agent-page.agent')}>
    <h3>{tx('settings:agent-page.agent')}</h3>
    <p>{tx('settings:agent-page.the-root-agent-is-the-agent-a-new-session-starts-as-these-settin')}</p>
    {scope === 'workspace' && <p className={css.hint}>{tx('settings:agent-page.each-of-these-is-an-independent-workspace-override-leaving-one-a')}</p>}

    <UnitForm<string> title={tx('settings:agent-page.root-identity')} authored={document.agent_id ?? undefined} blank="" revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'agent_identity', authored } })}>
      {(value, change) => <TextField label={tx('settings:agent-page.agent-identity')} value={value} change={change} />}
    </UnitForm>

    <UnitForm<string> title={tx('settings:agent-page.root-description')} authored={document.agent?.description ?? undefined} blank="" revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'description', authored } })}>
      {(value, change) => <TextField label={tx('settings:agent-page.root-description')} value={value} change={change} />}
    </UnitForm>

    <UnitForm<string> title={tx('settings:agent-page.root-instructions')} authored={document.agent?.instructions ?? undefined} blank="" revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'instructions', authored } })}>
      {(value, change) => <label>{tx('settings:agent-page.instructions')}<textarea value={value} onChange={event => change(event.target.value)} /></label>}
    </UnitForm>

    <UnitForm<AgentProjectInstructionsDocument> title={tx('settings:agent-page.project-guidance')} authored={document.agent?.agents_md ?? undefined}
      blank={{}} revision={revision} mutation={authored => ({ kind: 'config', mutation: { unit: 'project_guidance', authored } })}>
      {(value, change) => <>
        <p className={css.hint}>{tx('settings:agent-page.project-guidance-files-are-read-from-the-workspace-at-prompt-tim')}</p>
        <label><input type="checkbox" checked={value.inherit ?? true} onChange={event => change({ ...value, inherit: event.target.checked })} />{tx('settings:agent-page.include-project-guidance')}</label>
        <Names label={tx('settings:agent-page.guidance-files')} value={value.files ?? []} change={files => change({ ...value, files })} />
      </>}
    </UnitForm>

    <p className={css.hint}>{tx('settings:agent-page.named-agents-are-separate-resources-with-their-own-definitions-m')}</p>
  </section>;
}
