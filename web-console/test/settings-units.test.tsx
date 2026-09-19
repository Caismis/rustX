// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { CatalogEditor } from '../src/app/settings/CatalogEditor';
import { RootEditor } from '../src/app/settings/RootEditor';
import { RuntimeEditor } from '../src/app/settings/RuntimeEditor';
import { AgentEditor } from '../src/app/settings/AgentEditor';
import { cfg3Effective, cfg3Source } from './cfg3-data';
import type { ModelLayer } from '../../protocol/app-server/v9';
import type { SaveSource } from '../src/app/settings/controls';
afterEach(cleanup);
const save = () => vi.fn<SaveSource>().mockResolvedValue(undefined);
it('preserves complete Model replacement, profile params and all compatibility fields', async () => {
  const model = { ...cfg3Effective().document.models!.main, request_params: { temperature: .3, nested: { budget: 32 } }, reasoning: { default_profile: 'deep', profiles: { deep: { enabled: true, request_params: { reasoning: { effort: 'high' } } } } }, compat: { chat_max_tokens_field: 'max_completion_tokens' as const, chat_stream_usage: 'supported' as const, chat_reasoning_replay: 'omit' as const, chat_tool_protocol: 'native' as const, responses_storage: 'stateless' as const } };
  const write = save(); render(<CatalogEditor document={{ models: { main: model } }} scope="workspace" revision="exact" save={write} />);
  fireEvent.click(screen.getByRole('button', { name: 'Edit Model main' }));
  fireEvent.change(screen.getByLabelText('Wire model identity'), { target: { value: 'new-wire' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write).toHaveBeenCalledWith({ kind: 'config', scope: 'workspace', mutation: { unit: 'model', id: 'main', authored: { ...model, id: 'new-wire' } } }, 'exact'));
  fireEvent.click(screen.getByRole('button', { name: 'Remove Model main' }));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', scope: 'workspace', mutation: { unit: 'model', id: 'main', authored: null } }, 'exact'));
});
it('edits reasoning profiles and native request values without interpreting semantics', async () => {
  const write = save(); render(<CatalogEditor document={{ models: { main: cfg3Effective().document.models!.main } }} scope="user" revision="r1" save={write} />);
  fireEvent.click(screen.getByRole('button', { name: 'Edit Model main' })); fireEvent.click(screen.getByText('Reasoning profiles'));
  fireEvent.change(screen.getByLabelText('New reasoning profile'), { target: { value: 'deep' } }); fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  const profile = screen.getByLabelText('deep').parentElement!.parentElement!;
  fireEvent.change(within(profile).getByLabelText('Parameter name'), { target: { value: 'budget' } }); fireEvent.click(within(profile).getByRole('button', { name: 'Add parameter' }));
  fireEvent.change(within(profile).getByLabelText('budget'), { target: { value: '1024' } }); fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write.mock.calls[0][0]).toMatchObject({ mutation: { authored: { reasoning: { default_profile: 'deep', profiles: { deep: { enabled: true, request_params: { budget: 1024 } } } } } } }));
});
it.each(['all', 'none', 'exact'] as const)('writes exact native %s Skill selection', async mode => {
  const write = save(); render(<RootEditor document={{}} scope="workspace" revision="s1" save={write} section="root-skills" models={[]} skillRoots={['/user/skills', '/workspace/skills']} />);
  fireEvent.change(screen.getByLabelText('Selection'), { target: { value: mode } });
  if (mode === 'exact') fireEvent.change(screen.getByLabelText('Visible Skills identities 1'), { target: { value: 'review' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Skill visibility' }));
  await waitFor(() => expect(write).toHaveBeenCalledWith({ kind: 'config', scope: 'workspace', mutation: { unit: 'skills', authored: mode === 'all' ? 'all' : mode === 'none' ? [] : ['review'] } }, 's1'));
});
it('writes Native and MCP invocation policies independently, including empty native defaults', async () => {
  const write = save(); render(<RuntimeEditor policyOnly document={{}} scope="workspace" revision="p1" save={write} />);
  const form = within(screen.getByRole('form', { name: 'bash policy' }));
  fireEvent.change(form.getByLabelText('execution'), { target: { value: 'model_selectable' } }); fireEvent.change(form.getByLabelText('concurrency'), { target: { value: 'parallel' } }); fireEvent.change(form.getByLabelText('approval'), { target: { value: 'always' } });
  fireEvent.click(form.getByRole('button', { name: 'Save bash policy' }));
  await waitFor(() => expect(write.mock.calls[0]).toEqual([{ kind: 'config', scope: 'workspace', mutation: { unit: 'native_policy', id: 'bash', authored: { execution: 'model_selectable', concurrency: 'parallel', approval: 'always' } } }, 'p1']));
  fireEvent.change(screen.getByLabelText('MCP policy identity'), { target: { value: 'search' } }); fireEvent.click(screen.getByRole('button', { name: 'Edit MCP policy' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save MCP policy search' }));
  await waitFor(() => expect(write.mock.calls[1][0]).toEqual({ kind: 'config', scope: 'workspace', mutation: { unit: 'mcp_policy', id: 'search', authored: {} } }));
});
it('round-trips an independent Agent profile with delegation, guidance, model and worktree', async () => {
  const source = cfg3Source(), write = save();
  const authored = { description: 'Review', instructions: 'Inspect', agents: ['helper'], workflows: ['audit'], model: { model: 'main', reasoning_profile: { mode: 'catalog_default' as const }, max_output_tokens: { mode: 'catalog_default' as const }, summary_model: { mode: 'session' as const }, request_params: { temperature: .5 } }, tools: { builtin: ['read'], sources: { 'python:analysis': 'all' as const, search: [] } }, skills: 'all' as const, agents_md: { inherit: false, files: ['REVIEW.md'] }, timeout_ms: '30000', worktree: { enabled: true, require_clean_parent: false }, plugins: { todo: { enabled: true }, goal: { enabled: false }, agent_status: { enabled: true, time: { enabled: false }, background: { enabled: true } } } };
  source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/workspace/.agents/agents/reviewer.toml', revision: 'agent-r1', authored } }];
  render(<AgentEditor source={source} scope="workspace" models={['main']} save={write} />);
  fireEvent.click(screen.getByRole('button', { name: 'Edit Agent reviewer' })); fireEvent.click(screen.getByRole('button', { name: 'Save Agent reviewer' }));
  await waitFor(() => expect(write).toHaveBeenCalledWith({ kind: 'agent', scope: 'workspace', name: 'reviewer', authored }, 'agent-r1'));
});
it.each(['search', 'python:analysis'])('keeps all, none and exact Tool selection distinct for %s', async id => {
  const write = save(); render(<RootEditor document={{ agent: { tools: { sources: { [id]: [] } } } }} scope="user" revision="tools-r1" save={write} section="root-tools" models={[]} skillRoots={[]} />);
  const form = within(screen.getByRole('form', { name: `Source ${id}` }));
  for (const mode of ['all', 'none', 'exact']) {
    fireEvent.change(form.getByLabelText('Selection'), { target: { value: mode } });
    if (mode === 'exact') fireEvent.change(form.getByRole('textbox', { name: `${id} identities 1` }), { target: { value: 'inspect' } });
    fireEvent.click(form.getByRole('button', { name: `Save Source ${id}` }));
    await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', scope: 'user', mutation: { unit: 'source_tools', id, authored: mode === 'all' ? 'all' : mode === 'none' ? [] : ['inspect'] } }, 'tools-r1'));
  }
});
it('replaces Root model intent and project guidance as independent native units', async () => {
  const write = save();
  render(<RootEditor document={{ agent: { model: { model: 'main', request_params: { temperature: .4 } } } }} scope="workspace" revision="root-r1" save={write} section="root-model" models={['main', 'summary']} skillRoots={[]} />);
  const model = within(screen.getByRole('form', { name: 'Root model' }));
  fireEvent.change(model.getByLabelText('Reasoning profile'), { target: { value: 'profile' } });
  fireEvent.change(model.getByLabelText('Profile identity'), { target: { value: 'deep' } });
  fireEvent.change(model.getByLabelText('Output limit'), { target: { value: '4096' } });
  fireEvent.change(model.getByLabelText('Summary model'), { target: { value: 'summary' } });
  fireEvent.click(model.getByRole('button', { name: 'Save Root model' }));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', scope: 'workspace', mutation: { unit: 'root_model', authored: { model: 'main', request_params: { temperature: .4 }, reasoning_profile: { mode: 'profile', name: 'deep' }, max_output_tokens: { mode: 'limit', tokens: 4096 }, summary_model: { mode: 'explicit', model: 'summary' } } } }, 'root-r1'));
  fireEvent.change(model.getByLabelText('Reasoning profile'), { target: { value: 'catalog_default' } });
  fireEvent.change(model.getByLabelText('Output limit'), { target: { value: '' } });
  fireEvent.click(model.getByRole('button', { name: 'Save Root model' }));
  await waitFor(() => expect(write.mock.calls.at(-1)?.[0]).toMatchObject({ mutation: { authored: { reasoning_profile: { mode: 'catalog_default' }, max_output_tokens: { mode: 'catalog_default' } } } }));
  const guidance = within(screen.getByRole('form', { name: 'Project guidance' }));
  fireEvent.click(guidance.getByLabelText('Include project guidance'));
  fireEvent.click(guidance.getByRole('button', { name: 'Save Project guidance' }));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', scope: 'workspace', mutation: { unit: 'project_guidance', authored: { inherit: false } } }, 'root-r1'));
  fireEvent.click(guidance.getByRole('button', { name: 'Add Guidance files' }));
  fireEvent.change(guidance.getByRole('textbox', { name: 'Guidance files 1' }), { target: { value: 'REVIEW.md' } });
  fireEvent.click(guidance.getByRole('button', { name: 'Save Project guidance' }));
  await waitFor(() => expect(write.mock.calls.at(-1)?.[0]).toMatchObject({ mutation: { authored: { inherit: false, files: ['REVIEW.md'] } } }));
});
it.each(['Root', 'named Agent'] as const)('%s preserves and edits the complete explicit Summary Model independently', async owner => {
  const write = save();
  const summary = { mode: 'explicit' as const, model: 'summary-a', reasoning_profile: { mode: 'profile' as const, name: 'deep' }, max_output_tokens: { mode: 'limit' as const, tokens: 2048 }, request_params: { temperature: .2 } };
  const model = { model: 'main', reasoning_profile: { mode: 'catalog_default' as const }, request_params: { temperature: .7 }, summary_model: summary };
  const models = ['main', 'summary-a', 'summary-b'];
  if (owner === 'Root') render(<RootEditor document={{ agent: { model } }} scope="workspace" revision="r1" save={write} section="root-model" models={models} skillRoots={[]} />);
  else {
    const source = cfg3Source();
    source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/agent.toml', revision: 'r1', authored: { model } } }];
    render(<AgentEditor source={source} scope="workspace" models={models} save={write} />);
    fireEvent.click(screen.getByRole('button', { name: 'Edit Agent reviewer' }));
  }
  const commit = async (expected: ModelLayer) => {
    const before = write.mock.calls.length;
    const form = screen.getByRole('form', { name: owner === 'Root' ? 'Root model' : 'Agent reviewer' }) as HTMLFormElement;
    expect(form.checkValidity()).toBe(true);
    fireEvent.click(within(form).getByRole('button', { name: owner === 'Root' ? 'Save Root model' : 'Save Agent reviewer' }));
    await waitFor(() => expect(write).toHaveBeenCalledTimes(before + 1));
    expect(write.mock.calls.at(-1)).toEqual([owner === 'Root'
      ? { kind: 'config', scope: 'workspace', mutation: { unit: 'root_model', authored: expected } }
      : { kind: 'agent', scope: 'workspace', name: 'reviewer', authored: { model: expected } }, 'r1']);
  };
  // Changing identity must not discard any nested authored intent.
  fireEvent.change(screen.getByLabelText('Summary model'), { target: { value: 'summary-b' } });
  await commit({ ...model, summary_model: { ...summary, model: 'summary-b' } });
  const nested = within(screen.getByRole('group', { name: 'Explicit Summary Model settings' }));
  fireEvent.change(nested.getByLabelText('Summary Profile identity'), { target: { value: 'quick' } });
  fireEvent.change(nested.getByLabelText('Summary Output limit'), { target: { value: '1024' } });
  fireEvent.change(nested.getByLabelText('temperature'), { target: { value: '0.4' } });
  await commit({ ...model, summary_model: { ...summary, model: 'summary-b', reasoning_profile: { mode: 'profile', name: 'quick' }, max_output_tokens: { mode: 'limit', tokens: 1024 }, request_params: { temperature: .4 } } });
  fireEvent.change(nested.getByLabelText('Summary Reasoning profile'), { target: { value: 'catalog_default' } });
  fireEvent.change(nested.getByLabelText('Summary Output limit'), { target: { value: '' } });
  await commit({ ...model, summary_model: { ...summary, model: 'summary-b', reasoning_profile: { mode: 'catalog_default' }, max_output_tokens: { mode: 'catalog_default' }, request_params: { temperature: .4 } } });
  fireEvent.change(screen.getByLabelText('Summary model'), { target: { value: '' } });
  await commit({ ...model, summary_model: { mode: 'session' } });
  expect(screen.queryByRole('group', { name: 'Explicit Summary Model settings' })).toBeNull();
  fireEvent.change(screen.getByLabelText('Summary model'), { target: { value: 'summary-a' } });
  await commit({ ...model, summary_model: { mode: 'explicit', model: 'summary-a' } });
});
it.each([{}, { description: 'Review' }, { instructions: 'Inspect' }])('saves an Agent with optional profile text omitted: %j', async authored => {
  const source = cfg3Source(), write = save();
  source.agents = [{ name: 'optional', scope: 'workspace', source: { path: '/agent.toml', revision: 'optional-r1', authored } }];
  render(<AgentEditor source={source} scope="workspace" models={[]} save={write} />);
  fireEvent.click(screen.getByRole('button', { name: 'Edit Agent optional' }));
  fireEvent.click(screen.getByLabelText('read'));
  expect((screen.getByRole('form', { name: 'Agent optional' }) as HTMLFormElement).checkValidity()).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Save Agent optional' }));
  await waitFor(() => expect(write).toHaveBeenCalledWith({ kind: 'agent', scope: 'workspace', name: 'optional', authored: { ...authored, tools: { builtin: ['read'] } } }, 'optional-r1'));
});
