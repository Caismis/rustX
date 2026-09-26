import type { AgentProjectInstructionsDocument, RuntimeLayer, SourceScope } from '../../../../../protocol/app-server/v25';
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
  return <section aria-label="Agent">
    <h3>Agent</h3>
    <p>The root Agent is the Agent a new Session starts as. These settings say who it is and how it is guided.</p>
    {scope === 'workspace' && <p className={css.hint}>Each of these is an independent Workspace override. Leaving one alone keeps inheriting the global value.</p>}

    <UnitForm<string> title="Root identity" authored={document.agent_id ?? undefined} blank="" revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'agent_identity', authored } })}>
      {(value, change) => <TextField label="Agent identity" value={value} change={change} />}
    </UnitForm>

    <UnitForm<string> title="Root description" authored={document.agent?.description ?? undefined} blank="" revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'description', authored } })}>
      {(value, change) => <TextField label="Root description" value={value} change={change} />}
    </UnitForm>

    <UnitForm<string> title="Root instructions" authored={document.agent?.instructions ?? undefined} blank="" revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'instructions', authored } })}>
      {(value, change) => <label>Instructions<textarea value={value} onChange={event => change(event.target.value)} /></label>}
    </UnitForm>

    <UnitForm<AgentProjectInstructionsDocument> title="Project guidance" authored={document.agent?.agents_md ?? undefined}
      blank={{}} revision={revision} mutation={authored => ({ kind: 'config', mutation: { unit: 'project_guidance', authored } })}>
      {(value, change) => <>
        <p className={css.hint}>Project guidance files are read from the Workspace at prompt time. This setting selects whether they are included and which files count as guidance.</p>
        <label><input type="checkbox" checked={value.inherit ?? true} onChange={event => change({ ...value, inherit: event.target.checked })} />Include project guidance</label>
        <Names label="Guidance files" value={value.files ?? []} change={files => change({ ...value, files })} />
      </>}
    </UnitForm>

    <p className={css.hint}>Named Agents are separate resources with their own definitions; manage them under Extensions. Which of them the root Agent may delegate to is a separate decision on Tools &amp; Permissions.</p>
  </section>;
}
