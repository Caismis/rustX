// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { ModelsPage } from '../src/app/settings/models/ModelsPage';
import { AgentPage } from '../src/app/settings/agent/AgentPage';
import { ToolsPage } from '../src/app/settings/tools/ToolsPage';
import { ExtensionDetail } from '../src/app/settings/extensions/ExtensionDetail';
import { cfg3Effective, cfg3Source } from './cfg3-data';
import type { ModelLayer, RuntimeLayer, SourceScope, SourceSettings } from '../../protocol/app-server/v39';
import { chooseOption, confirmAction, renderEditor } from './settings-harness';
import { act } from '@testing-library/react';
import { localeController } from '../src/locale/controller';
import { translator } from '../src/locale/translation';
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
  <ModelsPage source={source} scope={scope} revision={revision} models={[]} profiles={{}} focus={{ kind: 'model', id }} onFocus={noop} />;
/** The Tools & Permissions page, bound to one authored `rustx.toml`. */
const toolsPage = (source: SourceSettings, document: RuntimeLayer, scope: SourceScope, revision: string) =>
  <ToolsPage source={source} document={document} scope={scope} revision={revision} />;

it('preserves complete Model replacement, profiles and all compatibility fields', async () => {
  const model = { ...cfg3Effective().document.models!.main, capabilities: { ...cfg3Effective().document.models!.main.capabilities, reasoning: true }, default_profile: 'deep', profiles: { deep: { reasoning_enabled: true, max_output_tokens: 2048, request_params: { reasoning: { effort: 'high' }, stop: [null] } }, quick: { reasoning_enabled: false, request_params: {} } }, compat: { chat_max_tokens_field: 'max_completion_tokens' as const, chat_stream_usage: 'supported' as const, chat_reasoning_replay: 'omit' as const, chat_tool_protocol: 'native' as const, responses_storage: 'stateless' as const } };
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

it('authors general Model Profiles as complete presets, moving model parameters into the first one', async () => {
  const main = { ...cfg3Effective().document.models!.main, request_params: { temperature: 0.2 } };
  const source = catalogSource('user', { models: { main } });
  const { writes: write } = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Model profiles' }));
  fireEvent.change(screen.getByLabelText('New profile identity'), { target: { value: 'precise' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  const precise = within(screen.getByRole('group', { name: 'Profile precise' }));
  // The Model's own parameters seed the first profile; the Model no longer
  // declares any, and the new profile is the default.
  expect((precise.getByLabelText('Profile request parameters') as HTMLTextAreaElement).value).toBe('{\n  "temperature": 0.2\n}');
  fireEvent.change(precise.getByLabelText('Profile request parameters'), { target: { value: '{"temperature": 0.1, "top_p": 0.5, "stop": ["\\n", null], "metadata": {"tier": null}}' } });
  fireEvent.change(precise.getByLabelText('Default output tokens'), { target: { value: '1024' } });
  fireEvent.change(screen.getByLabelText('New profile identity'), { target: { value: 'creative' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  const creative = within(screen.getByRole('group', { name: 'Profile creative' }));
  // A later profile starts empty: profiles never inherit from one another.
  expect((creative.getByLabelText('Profile request parameters') as HTMLTextAreaElement).value).toBe('{}');
  fireEvent.change(creative.getByLabelText('Profile request parameters'), { target: { value: '{"temperature": 1.3}' } });
  await chooseOption('Reasoning', 'Off', creative);
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  const { request_params: _, ...rest } = main;
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...rest, default_profile: 'precise', profiles: {
    precise: { request_params: { temperature: 0.1, top_p: 0.5, stop: ['\n', null], metadata: { tier: null } }, max_output_tokens: 1024 },
    creative: { request_params: { temperature: 1.3 }, reasoning_enabled: false },
  } } } });
  // Removing the default profile moves the default to a remaining one.
  fireEvent.click(precise.getByRole('button', { name: 'Delete profile precise' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(2));
  expect(write.mock.calls[1][0]).toMatchObject({ mutation: { authored: { default_profile: 'creative', profiles: { creative: { request_params: { temperature: 1.3 } } } } } });
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
  await chooseOption('execution', 'Model selectable', form);
  await chooseOption('concurrency', 'Parallel', form);
  await chooseOption('approval', 'Always', form);
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
  const authored = { description: 'Review', instructions: 'Inspect', agents: ['helper'], workflows: ['audit'], model: { model: 'main', profile: 'deep', max_output_tokens: { mode: 'catalog_default' as const }, summary_model: { mode: 'session' as const }, request_params: { temperature: .5, nested: [null] } }, tools: { builtin: ['read'], sources: { 'python:analysis': 'all' as const, search: [] } }, skills: 'all' as const, agents_md: { inherit: false, files: ['REVIEW.md'] }, timeout_ms: '30000', worktree: { enabled: true, require_clean_parent: false }, plugins: { todo: { enabled: true }, goal: { enabled: false }, agent_status: { enabled: true, time: { enabled: false }, background: { enabled: true } } } };
  source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/workspace/.agents/agents/reviewer.toml', revision: 'agent-r1', authored } }];
  const { writes: write } = await renderEditor(<ExtensionDetail source={source} scope="workspace" revision="r1" models={['main']} profiles={{}} family="agent" name="reviewer" onFocus={noop} />, { source, context: source });
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
    <ModelsPage source={source} scope="workspace" revision="root-r1" models={['main', 'summary']} profiles={{ main: ['deep', 'quick'] }} onFocus={noop} />,
    { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Default model for new Sessions' }));
  const model = within(screen.getByRole('form', { name: 'Default model' }));
  await chooseOption('Profile', 'deep', model);
  fireEvent.change(model.getByLabelText('Output limit'), { target: { value: '4096' } });
  await chooseOption('Summary model', 'summary', model);
  fireEvent.click(model.getByRole('button', { name: 'Save Default model' }));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'root_model', authored: { model: 'main', request_params: { temperature: .4 }, profile: 'deep', max_output_tokens: { mode: 'limit', tokens: 4096 }, summary_model: { mode: 'explicit', model: 'summary' } } } }, 'root-r1'));
  // The model default profile is an omission, never a sentinel.
  await chooseOption('Profile', 'Model default profile', model);
  fireEvent.change(model.getByLabelText('Output limit'), { target: { value: '' } });
  fireEvent.click(model.getByRole('button', { name: 'Save Default model' }));
  await waitFor(() => expect(write.mock.calls.at(-1)?.[0]).toEqual({ kind: 'config', mutation: { unit: 'root_model', authored: { model: 'main', request_params: { temperature: .4 }, max_output_tokens: { mode: 'catalog_default' }, summary_model: { mode: 'explicit', model: 'summary' } } } }));

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
  const summary = { mode: 'explicit' as const, model: 'summary-a', profile: 'deep', max_output_tokens: { mode: 'limit' as const, tokens: 2048 }, request_params: { temperature: .2 } };
  const model = { model: 'main', profile: 'fast', request_params: { temperature: .7 }, summary_model: summary };
  const models = ['main', 'summary-a', 'summary-b'];
  const profiles = { main: ['fast'], 'summary-a': ['deep'], 'summary-b': ['deep', 'quick'] };
  let write!: Awaited<ReturnType<typeof renderEditor>>['writes'];
  if (owner === 'default') {
    const source = catalogSource('workspace', { agent: { model } });
    ({ writes: write } = await renderEditor(<ModelsPage source={source} scope="workspace" revision="r1" models={models} profiles={profiles} onFocus={noop} />, { source, context: source }));
  } else {
    const source = cfg3Source();
    source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/agent.toml', revision: 'r1', authored: { model } } }];
    ({ writes: write } = await renderEditor(<ExtensionDetail source={source} scope="workspace" revision="r1" models={models} profiles={profiles} family="agent" name="reviewer" onFocus={noop} />, { source, context: source }));
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
  await chooseOption('Profile (Summary)', 'quick', nested);
  fireEvent.change(nested.getByLabelText('Output limit (Summary)'), { target: { value: '1024' } });
  fireEvent.change(nested.getByLabelText('Request parameter overrides (Summary)'), { target: { value: '{"temperature": 0.4, "seed": null}' } });
  await commit({ ...model, summary_model: { ...summary, model: 'summary-b', profile: 'quick', max_output_tokens: { mode: 'limit', tokens: 1024 }, request_params: { temperature: .4, seed: null } } });
  await chooseOption('Profile (Summary)', 'Model default profile', nested);
  fireEvent.change(nested.getByLabelText('Output limit (Summary)'), { target: { value: '' } });
  const { profile: _, ...withoutProfile } = summary;
  await commit({ ...model, summary_model: { ...withoutProfile, model: 'summary-b', max_output_tokens: { mode: 'catalog_default' }, request_params: { temperature: .4, seed: null } } });
  await chooseOption('Summary model', 'Follow selected model');
  await commit({ ...model, summary_model: { mode: 'session' } });
  expect(screen.queryByRole('group', { name: 'Explicit Summary Model settings' })).toBeNull();
  await chooseOption('Summary model', 'summary-a');
  await commit({ ...model, summary_model: { mode: 'explicit', model: 'summary-a' } });
});

it('the explicit Summary Model variant owns complete labels in each locale without changing native payloads', async () => {
  const summary = { mode: 'explicit' as const, model: 'summary-a', profile: 'deep', max_output_tokens: { mode: 'limit' as const, tokens: 2048 }, request_params: { temperature: .2 } };
  const model = { model: 'main', profile: 'main-profile', max_output_tokens: { mode: 'limit' as const, tokens: 4096 }, request_params: { temperature: .7 }, summary_model: summary };
  const source = catalogSource('workspace', { agent: { model } });
  const { writes: write, context } = await renderEditor(<ModelsPage source={source} scope="workspace" revision="r1" models={['main', 'summary-a']} profiles={{ main: ['main-profile'], 'summary-a': ['deep', 'quick'] }} onFocus={noop} />, { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Default model for new Sessions' }));
  const labels = (locale: 'en' | 'zh') => {
    const tx = translator(locale);
    const group = screen.getByRole('group', { name: tx('settings:models-page.explicit-summary-model-settings') });
    const nested = within(group);
    return {
      group,
      profile: nested.getByRole('button', { name: (name: string) => name.endsWith(tx('settings:models-page.profile-summary')) }),
      limit: nested.getByLabelText(tx('settings:models-page.output-limit-summary')) as HTMLInputElement,
      params: nested.getByLabelText(tx('settings:models-page.request-parameter-overrides-summary')) as HTMLTextAreaElement,
      outer: screen.getByLabelText(tx('settings:models-page.request-parameter-overrides')) as HTMLTextAreaElement,
    };
  };
  const english = labels('en');
  expect([english.limit.value, english.params.value, english.outer.value]).toEqual(['2048', '{\n  "temperature": 0.2\n}', '{\n  "temperature": 0.7\n}']);
  expect(english.profile.textContent).toContain('deep');
  expect(screen.getByLabelText('Output limit (Summary)')).toBe(english.limit);
  const snapshot = context();

  act(() => localeController.setLocale('zh'));
  const chinese = labels('zh');
  expect([chinese.limit, chinese.params, chinese.outer]).toEqual([english.limit, english.params, english.outer]);
  expect(screen.getByLabelText('输出上限（摘要）')).toBe(english.limit);
  expect(screen.getByLabelText('请求参数覆盖（摘要）')).toBe(english.params);
  expect(within(chinese.group).getByRole('button', { name: /预设（摘要）$/ })).toBe(chinese.profile);
  expect(within(chinese.group).queryAllByRole('button', { name: /Summary/ })).toEqual([]);
  expect(chinese.group.textContent).not.toMatch(/Summary|Profile|Output limit|Request parameter/);
  expect(write).not.toHaveBeenCalled();
  expect(context()).toBe(snapshot);

  fireEvent.change(chinese.limit, { target: { value: '1024' } });
  const zh = translator('zh');
  const form = screen.getByRole('form', { name: zh('settings:models-page.default-model') });
  fireEvent.click(within(form).getByRole('button', { name: `${zh('settings:bridge.save')} ${zh('settings:models-page.default-model')}` }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0]).toEqual([{ kind: 'config', mutation: { unit: 'root_model', authored: {
    ...model, summary_model: { ...summary, max_output_tokens: { mode: 'limit', tokens: 1024 } },
  } } }, 'r1']);
});

it.each([{}, { description: 'Review' }, { instructions: 'Inspect' }])('saves an Agent with optional profile text omitted: %j', async authored => {
  const source = cfg3Source();
  source.agents = [{ name: 'optional', scope: 'workspace', source: { path: '/agent.toml', revision: 'optional-r1', authored } }];
  const { writes: write } = await renderEditor(<ExtensionDetail source={source} scope="workspace" revision="r1" models={[]} profiles={{}} family="agent" name="optional" onFocus={noop} />, { source, context: source });
  fireEvent.click(screen.getByLabelText('read'));
  expect((screen.getByRole('form', { name: 'Agent optional' }) as HTMLFormElement).checkValidity()).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Save Agent optional' }));
  await waitFor(() => expect(write).toHaveBeenCalledWith({ kind: 'agent', name: 'optional', authored: { ...authored, tools: { builtin: ['read'] } } }, 'optional-r1'));
});

// ── A request-parameter object's JSON text is a buffer, never a second draft ──

/** A User Model with native request parameters, opened on its detail with the
 * request-parameter editor expanded. */
async function requestParameters() {
  const main = { ...cfg3Effective().document.models!.main, request_params: { temperature: 1, top_p: 0.5 } };
  const source = catalogSource('user', { models: { main } });
  const editor = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Request defaults and protocol compatibility' }));
  return { ...editor, main, input: screen.getByLabelText('Model request parameters') as HTMLTextAreaElement };
}
const invalidJson = 'Not valid JSON. Nothing is saved until the text is one complete JSON object.';
const save = () => screen.getByRole('button', { name: 'Save Model main' }) as HTMLButtonElement;

it('a pasted object with nested values, arrays and null becomes the structured draft', async () => {
  const { writes: write, main, input } = await requestParameters();
  expect(input.value).toBe('{\n  "temperature": 1,\n  "top_p": 0.5\n}');
  fireEvent.change(input, { target: { value: '{"provider": {"order": ["a", "b"], "fallback": null}, "stop": [null, "\\n"], "seed": null}' } });
  expect(input.validity.valid).toBe(true);
  fireEvent.click(save());
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...main,
    request_params: { provider: { order: ['a', 'b'], fallback: null }, stop: [null, '\n'], seed: null } } } });
});

it('an actor-owned reset replaces a mounted JSON buffer, its error and its validity, and a later edit starts from it', async () => {
  const { writes: write, main, input } = await requestParameters();
  // A complete object becomes the actor's draft; incomplete text after it
  // stays in the editor with its error and its invalid state.
  fireEvent.change(input, { target: { value: '{"temperature": 2}' } });
  fireEvent.change(input, { target: { value: '{"temperature": 2' } });
  expect(input.value).toBe('{"temperature": 2');
  expect(screen.getByText(invalidJson)).toBeTruthy();
  expect(input.validity.valid).toBe(false);
  // Discarding the draft is the transaction owner's reset; the same editor
  // stays mounted across it and follows the owner's value.
  fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Discard draft' })).toBeNull());
  expect(screen.getByLabelText('Model request parameters')).toBe(input);
  expect(input.value).toBe('{\n  "temperature": 1,\n  "top_p": 0.5\n}');
  expect(screen.queryByText(invalidJson)).toBeNull();
  expect(input.validity.valid).toBe(true);
  expect(input.validationMessage).toBe('');
  // Following the owner wrote nothing back and began no draft.
  expect(save().disabled).toBe(true);
  fireEvent.change(input, { target: { value: input.value.replace('0.5', '0.25') } });
  fireEvent.click(save());
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...main, request_params: { temperature: 1, top_p: 0.25 } } } });
});

it.each([
  ['{"budget":', invalidJson],
  ['[1, 2]', 'Request parameters must be a JSON object.'],
  ['null', 'Request parameters must be a JSON object.'],
  ['{"a": 1, "a": 2}', 'The JSON object repeats a key at $.a.'],
  ['{"outer": {"list": [{"k": 1, "k": 1}]}}', 'The JSON object repeats a key at $.outer.list[0].k.'],
])('invalid text %s stays visible and diagnosed locally and never reaches the draft', async (text, diagnostic) => {
  const { writes: write, main, input } = await requestParameters();
  fireEvent.change(input, { target: { value: text } });
  expect(input.value).toBe(text);
  expect(screen.getByText(diagnostic)).toBeTruthy();
  expect(input.validity.valid).toBe(false);
  // No draft exists: the invalid text was never offered to the actor.
  expect(save().disabled).toBe(true);
  expect(screen.queryByRole('button', { name: 'Discard draft' })).toBeNull();
  // Correcting it is an ordinary edit; the buffer is not reset by its own echo.
  fireEvent.change(input, { target: { value: '{"budget": 32}' } });
  expect(input.value).toBe('{"budget": 32}');
  expect(screen.queryByText(diagnostic)).toBeNull();
  expect(input.validity.valid).toBe(true);
  fireEvent.click(save());
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...main, request_params: { budget: 32 } } } });
});

it('whitespace and key order alone are not an edit and begin no draft', async () => {
  const { writes: write, input } = await requestParameters();
  fireEvent.change(input, { target: { value: '{ "top_p" : 0.5,\n\n   "temperature":1 }' } });
  expect(input.value).toBe('{ "top_p" : 0.5,\n\n   "temperature":1 }');
  expect(input.validity.valid).toBe(true);
  expect(save().disabled).toBe(true);
  expect(screen.queryByRole('button', { name: 'Discard draft' })).toBeNull();
  expect(write).not.toHaveBeenCalled();
});

it.each([['  ', false], ['{}', true]] as const)('blank optional parameters are absent, while an explicit empty object is authored: %j', async (text, present) => {
  const { writes: write, main, input } = await requestParameters();
  const { request_params: _, ...absent } = main;
  fireEvent.change(input, { target: { value: text } });
  fireEvent.click(save());
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: present ? { ...absent, request_params: {} } : absent } });
});
