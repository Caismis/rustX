// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { ModelsPage } from '../src/app/settings/models/ModelsPage';
import { AgentPage } from '../src/app/settings/agent/AgentPage';
import { ToolsPage } from '../src/app/settings/tools/ToolsPage';
import { ExtensionDetail } from '../src/app/settings/extensions/ExtensionDetail';
import { cfg3Effective, cfg3Source } from './cfg3-data';
import type { ModelLayer, RuntimeLayer, SourceScope, SourceSettings } from '../../protocol/app-server/v25';
import { chooseOption, confirmAction, renderEditor } from './settings-harness';
afterEach(cleanup);

const noop = () => {};

/** An editor bound to a real Workspace source projection, so the unit forms see
 * the same authored/effective/provenance facts they see inside Settings. */
function workspaceSource(resolved: Record<string, unknown> = {}) {
  const source = cfg3Source();
  source.target = { kind: 'workspace', directory: '/workspace/A' };
  source.resolved = resolved as never;
  return source;
}
/** One scope's authored catalog document, bound to a source projection that
 * resolves to exactly the same document, as a single-scope catalog does. */
function catalogSource(scope: SourceScope, document: Record<string, unknown>) {
  const source = cfg3Source();
  source.target = scope === 'user' ? { kind: 'user' } : { kind: 'workspace', directory: '/workspace/A' };
  source[scope === 'user' ? 'user' : 'workspace']!.authored = document as never;
  source.resolved = document as never;
  return source;
}
/** The Models page focused on one Model's detail, which is where a Model's
 * complete typed contract is authored. */
const modelDetail = (source: SourceSettings, scope: SourceScope, revision: string, id: string) =>
  <ModelsPage source={source} scope={scope} revision={revision} models={[]} focus={{ kind: 'model', id }} onFocus={noop} />;
/** The Tools & Permissions page, bound to one authored `rustx.toml`. */
const toolsPage = (source: SourceSettings, document: RuntimeLayer, scope: SourceScope, revision: string) =>
  <ToolsPage source={source} document={document} scope={scope} revision={revision} />;

it('preserves complete Model replacement, profile params and all compatibility fields', async () => {
  const model = { ...cfg3Effective().document.models!.main, request_params: { temperature: .3, nested: { budget: 32 } }, reasoning: { default_profile: 'deep', profiles: { deep: { enabled: true, request_params: { reasoning: { effort: 'high' } } } } }, compat: { chat_max_tokens_field: 'max_completion_tokens' as const, chat_stream_usage: 'supported' as const, chat_reasoning_replay: 'omit' as const, chat_tool_protocol: 'native' as const, responses_storage: 'stateless' as const } };
  const source = catalogSource('workspace', { models: { main: model } });
  const { writes: write } = await renderEditor(modelDetail(source, 'workspace', 'exact', 'main'), { source, context: source });
  // One unrelated field changes; everything else must survive the complete
  // object replacement untouched.
  fireEvent.change(screen.getByLabelText('Wire model identity'), { target: { value: 'new-wire' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...model, id: 'new-wire' } } }, 'exact'));
  await confirmAction('Use global default Model main');
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: null } }, 'exact'));
});

it('edits reasoning profiles and native request values without interpreting semantics', async () => {
  const source = catalogSource('user', { models: { main: cfg3Effective().document.models!.main } });
  const { writes: write } = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Reasoning profiles' }));
  fireEvent.change(screen.getByLabelText('New reasoning profile'), { target: { value: 'deep' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  const profile = screen.getByLabelText('deep').parentElement!.parentElement!;
  fireEvent.change(within(profile).getByLabelText('Parameter name'), { target: { value: 'budget' } });
  fireEvent.click(within(profile).getByRole('button', { name: 'Add parameter' }));
  fireEvent.change(within(profile).getByLabelText('budget'), { target: { value: '1024' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write.mock.calls[0][0]).toMatchObject({ mutation: { authored: { reasoning: { default_profile: 'deep', profiles: { deep: { enabled: true, request_params: { budget: 1024 } } } } } } }));
});

it.each(['all', 'none', 'exact'] as const)('writes exact native %s Skill selection', async mode => {
  const source = workspaceSource();
  const { writes: write } = await renderEditor(toolsPage(source, {}, 'workspace', 's1'), { source, context: source });
  const form = within(screen.getByRole('form', { name: 'Skill visibility' }));
  // An explicitly authored empty selection is a value, not the absence of one,
  // so it is authored by the explicit Override gesture rather than produced by
  // re-picking the mode the control already displays.
  if (mode === 'none') fireEvent.click(form.getByRole('button', { name: 'Override Skill visibility' }));
  else await chooseOption('Selection', mode === 'all' ? 'All' : 'Exact identities', form);
  if (mode === 'exact') fireEvent.change(form.getByLabelText('Visible Skills identities 1'), { target: { value: 'review' } });
  fireEvent.click(form.getByRole('button', { name: 'Save Skill visibility' }));
  await waitFor(() => expect(write).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'skills', authored: mode === 'all' ? 'all' : mode === 'none' ? [] : ['review'] } }, 's1'));
});

it('writes Native and MCP invocation policies independently, and an empty policy only when explicitly authored', async () => {
  const context = workspaceSource();
  const { writes: write } = await renderEditor(toolsPage(context, {}, 'workspace', 'p1'), { source: context, context });
  // Per-Tool invocation policy is progressively disclosed rather than given a
  // page of its own.
  fireEvent.click(screen.getByRole('button', { name: 'Advanced Tool policies' }));
  const form = within(screen.getByRole('form', { name: 'bash policy' }));
  await chooseOption('execution', 'model_selectable', form);
  await chooseOption('concurrency', 'parallel', form);
  await chooseOption('approval', 'always', form);
  fireEvent.click(form.getByRole('button', { name: 'Save bash policy' }));
  await waitFor(() => expect(write.mock.calls[0]).toEqual([{ kind: 'config', mutation: { unit: 'native_policy', id: 'bash', authored: { execution: 'model_selectable', concurrency: 'parallel', approval: 'always' } } }, 'p1']));
  fireEvent.change(screen.getByLabelText('MCP policy identity'), { target: { value: 'search' } });
  fireEvent.click(screen.getByRole('button', { name: 'Edit MCP policy' }));
  const mcp = within(screen.getByRole('form', { name: 'MCP policy search' }));
  // This Workspace authors no `search` policy; opening its editor authors none
  // either, so there is nothing to save until the user says so.
  expect((mcp.getByRole('button', { name: 'Save MCP policy search' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(mcp.getByRole('button', { name: 'Override MCP policy search' }));
  fireEvent.click(mcp.getByRole('button', { name: 'Save MCP policy search' }));
  // An explicitly authored empty policy object stays `{}` and is never null.
  await waitFor(() => expect(write.mock.calls[1][0]).toEqual({ kind: 'config', mutation: { unit: 'mcp_policy', id: 'search', authored: {} } }));
});

it('round-trips an independent Agent profile with delegation, guidance, model and worktree', async () => {
  const source = cfg3Source();
  const authored = { description: 'Review', instructions: 'Inspect', agents: ['helper'], workflows: ['audit'], model: { model: 'main', reasoning_profile: { mode: 'catalog_default' as const }, max_output_tokens: { mode: 'catalog_default' as const }, summary_model: { mode: 'session' as const }, request_params: { temperature: .5 } }, tools: { builtin: ['read'], sources: { 'python:analysis': 'all' as const, search: [] } }, skills: 'all' as const, agents_md: { inherit: false, files: ['REVIEW.md'] }, timeout_ms: '30000', worktree: { enabled: true, require_clean_parent: false }, plugins: { todo: { enabled: true }, goal: { enabled: false }, agent_status: { enabled: true, time: { enabled: false }, background: { enabled: true } } } };
  source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/workspace/.agents/agents/reviewer.toml', revision: 'agent-r1', authored } }];
  const { writes: write } = await renderEditor(<ExtensionDetail source={source} scope="workspace" revision="r1" models={['main']} family="agent" name="reviewer" onFocus={noop} />, { source, context: source });
  // Opening an authored profile authors nothing new, so a no-op Save is
  // unavailable; an edit back to the same text still round-trips every field.
  expect((screen.getByRole('button', { name: 'Save Agent reviewer' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(screen.getByLabelText('Description'), { target: { value: 'Review draft' } });
  fireEvent.change(screen.getByLabelText('Description'), { target: { value: 'Review' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Agent reviewer' }));
  await waitFor(() => expect(write).toHaveBeenCalledWith({ kind: 'agent', name: 'reviewer', authored }, 'agent-r1'));
});

it.each(['search', 'python:analysis'])('keeps all, none and exact Tool selection distinct for %s', async id => {
  const source = workspaceSource();
  source.target = { kind: 'user' };
  const document: RuntimeLayer = { agent: { tools: { sources: { [id]: [] } } } };
  const { writes: write } = await renderEditor(toolsPage(source, document, 'user', 'tools-r1'), { source, context: source });
  const form = within(screen.getByRole('form', { name: `Source ${id}` }));
  for (const mode of ['all', 'none', 'exact'] as const) {
    await chooseOption('Selection', mode === 'all' ? 'All' : mode === 'none' ? 'None' : 'Exact identities', form);
    if (mode === 'exact') fireEvent.change(form.getByRole('textbox', { name: `${id} identities 1` }), { target: { value: 'inspect' } });
    fireEvent.click(form.getByRole('button', { name: `Save Source ${id}` }));
    await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'source_tools', id, authored: mode === 'all' ? 'all' : mode === 'none' ? [] : ['inspect'] } }, 'tools-r1'));
  }
});

it('replaces default model intent and project guidance as independent native units on their own pages', async () => {
  // The default model belongs to the Models page and project guidance to the
  // Agent page: two user tasks, and two independent native semantic units.
  const source = catalogSource('workspace', { agent: { model: { model: 'main', request_params: { temperature: .4 } } } });
  const { writes: write, rerender } = await renderEditor(
    <ModelsPage source={source} scope="workspace" revision="root-r1" models={['main', 'summary']} onFocus={noop} />,
    { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Default model for new Sessions' }));
  const model = within(screen.getByRole('form', { name: 'Default model' }));
  await chooseOption('Reasoning profile', 'Named profile', model);
  fireEvent.change(model.getByLabelText('Profile identity'), { target: { value: 'deep' } });
  fireEvent.change(model.getByLabelText('Output limit'), { target: { value: '4096' } });
  await chooseOption('Summary model', 'summary', model);
  fireEvent.click(model.getByRole('button', { name: 'Save Default model' }));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'root_model', authored: { model: 'main', request_params: { temperature: .4 }, reasoning_profile: { mode: 'profile', name: 'deep' }, max_output_tokens: { mode: 'limit', tokens: 4096 }, summary_model: { mode: 'explicit', model: 'summary' } } } }, 'root-r1'));
  await chooseOption('Reasoning profile', 'Catalog default', model);
  fireEvent.change(model.getByLabelText('Output limit'), { target: { value: '' } });
  fireEvent.click(model.getByRole('button', { name: 'Save Default model' }));
  await waitFor(() => expect(write.mock.calls.at(-1)?.[0]).toMatchObject({ mutation: { authored: { reasoning_profile: { mode: 'catalog_default' }, max_output_tokens: { mode: 'catalog_default' } } } }));

  rerender(<AgentPage document={{}} scope="workspace" revision="root-r1" />);
  const guidance = within(screen.getByRole('form', { name: 'Project guidance' }));
  fireEvent.click(guidance.getByLabelText('Include project guidance'));
  fireEvent.click(guidance.getByRole('button', { name: 'Save Project guidance' }));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'project_guidance', authored: { inherit: false } } }, 'root-r1'));
  fireEvent.click(guidance.getByRole('button', { name: 'Add Guidance files' }));
  fireEvent.change(guidance.getByRole('textbox', { name: 'Guidance files 1' }), { target: { value: 'REVIEW.md' } });
  fireEvent.click(guidance.getByRole('button', { name: 'Save Project guidance' }));
  await waitFor(() => expect(write.mock.calls.at(-1)?.[0]).toMatchObject({ mutation: { authored: { inherit: false, files: ['REVIEW.md'] } } }));
});

it.each(['default', 'named Agent'] as const)('%s preserves and edits the complete explicit Summary Model independently', async owner => {
  const summary = { mode: 'explicit' as const, model: 'summary-a', reasoning_profile: { mode: 'profile' as const, name: 'deep' }, max_output_tokens: { mode: 'limit' as const, tokens: 2048 }, request_params: { temperature: .2 } };
  const model = { model: 'main', reasoning_profile: { mode: 'catalog_default' as const }, request_params: { temperature: .7 }, summary_model: summary };
  const models = ['main', 'summary-a', 'summary-b'];
  let write!: Awaited<ReturnType<typeof renderEditor>>['writes'];
  if (owner === 'default') {
    const source = catalogSource('workspace', { agent: { model } });
    ({ writes: write } = await renderEditor(<ModelsPage source={source} scope="workspace" revision="r1" models={models} onFocus={noop} />, { source, context: source }));
  } else {
    const source = cfg3Source();
    source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/agent.toml', revision: 'r1', authored: { model } } }];
    ({ writes: write } = await renderEditor(<ExtensionDetail source={source} scope="workspace" revision="r1" models={models} family="agent" name="reviewer" onFocus={noop} />, { source, context: source }));
  }
  if (owner === 'default') fireEvent.click(screen.getByRole('button', { name: 'Default model for new Sessions' }));
  const formName = owner === 'default' ? 'Default model' : 'Agent reviewer';
  const saveName = owner === 'default' ? 'Save Default model' : 'Save Agent reviewer';
  const commit = async (expected: ModelLayer) => {
    const before = write.mock.calls.length;
    const form = screen.getByRole('form', { name: formName }) as HTMLFormElement;
    expect(form.checkValidity()).toBe(true);
    fireEvent.click(within(form).getByRole('button', { name: saveName }));
    await waitFor(() => expect(write).toHaveBeenCalledTimes(before + 1));
    expect(write.mock.calls.at(-1)).toEqual([owner === 'default'
      ? { kind: 'config', mutation: { unit: 'root_model', authored: expected } }
      : { kind: 'agent', name: 'reviewer', authored: { model: expected } }, 'r1']);
  };
  // Changing identity must not discard any nested authored intent.
  await chooseOption('Summary model', 'summary-b');
  await commit({ ...model, summary_model: { ...summary, model: 'summary-b' } });
  const nested = within(screen.getByRole('group', { name: 'Explicit Summary Model settings' }));
  fireEvent.change(nested.getByLabelText('Profile identity (Summary)'), { target: { value: 'quick' } });
  fireEvent.change(nested.getByLabelText('Output limit (Summary)'), { target: { value: '1024' } });
  fireEvent.change(nested.getByLabelText('temperature'), { target: { value: '0.4' } });
  await commit({ ...model, summary_model: { ...summary, model: 'summary-b', reasoning_profile: { mode: 'profile', name: 'quick' }, max_output_tokens: { mode: 'limit', tokens: 1024 }, request_params: { temperature: .4 } } });
  await chooseOption('Reasoning profile (Summary)', 'Catalog default', nested);
  fireEvent.change(nested.getByLabelText('Output limit (Summary)'), { target: { value: '' } });
  await commit({ ...model, summary_model: { ...summary, model: 'summary-b', reasoning_profile: { mode: 'catalog_default' }, max_output_tokens: { mode: 'catalog_default' }, request_params: { temperature: .4 } } });
  await chooseOption('Summary model', 'Follow selected model');
  await commit({ ...model, summary_model: { mode: 'session' } });
  expect(screen.queryByRole('group', { name: 'Explicit Summary Model settings' })).toBeNull();
  await chooseOption('Summary model', 'summary-a');
  await commit({ ...model, summary_model: { mode: 'explicit', model: 'summary-a' } });
});

it.each([{}, { description: 'Review' }, { instructions: 'Inspect' }])('saves an Agent with optional profile text omitted: %j', async authored => {
  const source = cfg3Source();
  source.agents = [{ name: 'optional', scope: 'workspace', source: { path: '/agent.toml', revision: 'optional-r1', authored } }];
  const { writes: write } = await renderEditor(<ExtensionDetail source={source} scope="workspace" revision="r1" models={[]} family="agent" name="optional" onFocus={noop} />, { source, context: source });
  fireEvent.click(screen.getByLabelText('read'));
  expect((screen.getByRole('form', { name: 'Agent optional' }) as HTMLFormElement).checkValidity()).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Save Agent optional' }));
  await waitFor(() => expect(write).toHaveBeenCalledWith({ kind: 'agent', name: 'optional', authored: { ...authored, tools: { builtin: ['read'] } } }, 'optional-r1'));
});

// ── A request parameter's JSON text is a buffer, never a second draft ───────

/** A User Model with two explicit request parameters, opened on its detail
 * with the request-parameter editor expanded. */
async function requestParameters() {
  const main = { ...cfg3Effective().document.models!.main, request_params: { temperature: 1, top_p: 0.5 } };
  const source = catalogSource('user', { models: { main } });
  const editor = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Request defaults and protocol compatibility' }));
  return { ...editor, main, input: screen.getByLabelText('temperature') as HTMLInputElement };
}
const incomplete = 'Enter a complete JSON value before saving.';

it('an actor-owned reset replaces a mounted JSON buffer, its error and its validity, and a later edit starts from it', async () => {
  const { writes: write, main, input } = await requestParameters();
  // A complete value becomes the actor's draft; incomplete text after it
  // stays in the input with its error and its invalid state.
  fireEvent.change(input, { target: { value: '2' } });
  fireEvent.change(input, { target: { value: '{' } });
  expect(input.value).toBe('{');
  expect(screen.getByText(incomplete)).toBeTruthy();
  expect(input.validity.valid).toBe(false);
  // Discarding the draft is the transaction owner's reset. The parameter row
  // keeps its key, so the same input stays mounted across it.
  fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Discard draft' })).toBeNull());
  expect(screen.getByLabelText('temperature')).toBe(input);
  expect(input.value).toBe('1');
  expect(screen.queryByText(incomplete)).toBeNull();
  expect(input.validity.valid).toBe(true);
  expect(input.validationMessage).toBe('');
  // Following the owner wrote nothing back and began no draft.
  expect((screen.getByRole('button', { name: 'Save Model main' }) as HTMLButtonElement).disabled).toBe(true);
  // The next keystroke extends what the owner holds, not the stale buffer.
  fireEvent.change(input, { target: { value: `${input.value}5` } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...main, request_params: { temperature: 15, top_p: 0.5 } } } });
});

it('incomplete JSON stays visible and diagnosed locally and never reaches the draft', async () => {
  const { writes: write, main, input } = await requestParameters();
  fireEvent.change(input, { target: { value: '{"budget":' } });
  expect(input.value).toBe('{"budget":');
  expect(screen.getByText(incomplete)).toBeTruthy();
  expect(input.validity.valid).toBe(false);
  // No draft exists: the incomplete text was never offered to the actor.
  expect((screen.getByRole('button', { name: 'Save Model main' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: 'Discard draft' })).toBeNull();
  // Completing it is an ordinary edit; the buffer is not reset by its own echo.
  fireEvent.change(input, { target: { value: '{"budget": 32}' } });
  expect(input.value).toBe('{"budget": 32}');
  expect(screen.queryByText(incomplete)).toBeNull();
  expect(input.validity.valid).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...main, request_params: { temperature: { budget: 32 }, top_p: 0.5 } } } });
});
