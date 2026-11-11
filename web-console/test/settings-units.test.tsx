import {AvailabilityDetail} from '../src/app/resources/shared/AvailabilityDetail';
import {ResourceAvailability} from '../src/app/resources/shared/ResourceAvailability';
import { changeJson, readJson, jsonValidity } from './json-editor-helpers';
import { sourceReceipt } from './fixture';
// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { cfg3Client } from './cfg3-fixture';
import { userSettingsTarget } from '../src/app/settings/projection';
import { ModelsPage } from '../src/app/settings/models/ModelsPage';
import { AgentPage } from '../src/app/settings/agent/AgentPage';
import { ToolsPage } from '../src/app/settings/tools/ToolsPage';
import { AgentEditor } from '../src/app/resources/subagents/AgentEditor';
import { cfg3Effective, cfg3Source } from './cfg3-data';
import type { ModelLayer, RuntimeLayer, SourceScope, SourceSettings } from '../../protocol/app-server/v78';
import { chooseOption, confirmAction, renderEditor, sameRevision } from './settings-harness';
import type { WriteOutcome } from '../src/app/settings/machines/port';
import { act } from '@testing-library/react';
import { localeController } from '../src/locale/controller';
import { translator } from '../src/locale/translation';
afterEach(cleanup);

const noop = () => {};
const modelProbeFixture = { client: cfg3Client().client, target: userSettingsTarget };

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
  <ModelsPage {...modelProbeFixture} source={source} scope={scope} revision={revision} models={[]} profiles={{}} focus={{ kind: 'model', id }} onFocus={noop} onRefresh={noop} />;
/** The Tools & Permissions page, bound to one authored `rustx.toml`. */
const toolsPage = (source: SourceSettings, document: RuntimeLayer, scope: SourceScope, revision: string) =>
  <ToolsPage source={source} document={document} scope={scope} revision={revision} />;

it('configures a complete reasoning draft, preserves invalid JSON, and clears only the order', async () => {
  const original = cfg3Effective().document.models!.main;
  const { request_params: _, ...rest } = original;
  const main = { ...rest, default_profile: 'z-low', profiles: {
    'z-low': { max_output_tokens: 512, request_params: { effort: 'none' } },
    'a-high': { request_params: { effort: 'high' } },
  } };
  const source = catalogSource('user', { models: { main } });
  const { writes } = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), { source, context: source });
  expect(screen.getByText('Not configured')).toBeTruthy();
  expect(writes).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Configure reasoning order' }));
  const list = () => screen.getByRole('list', { name: 'Reasoning order (low to high)' });
  const ids = () => [...list().children].map(row => row.getAttribute('data-profile-order'));
  expect(ids()).toEqual(['a-high', 'z-low']);
  const json = screen.getByLabelText('API parameters (JSON)');
  changeJson(json, '{"request_params":');
  const up = screen.getByRole('button', { name: 'Move z-low up' });
  fireEvent.click(up);
  expect(ids()).toEqual(['z-low', 'a-high']);
  expect(document.activeElement).toBe(up);
  expect(readJson(json)).toBe('{"request_params":');
  fireEvent.click(screen.getByRole('button', { name: 'Profile a-high' }));
  expect(readJson(screen.getByLabelText('API parameters (JSON)'))).toBe('{"request_params":');
  expect(jsonValidity(json).checkValidity()).toBe(false);
  changeJson(json, JSON.stringify(main.profiles['z-low'].request_params));
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(writes).toHaveBeenCalledTimes(1));
  expect(writes.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: {
    ...main, reasoning_profile_order: ['z-low', 'a-high'],
  } } });
  fireEvent.click(screen.getByRole('button', { name: 'Clear reasoning order' }));
  expect(screen.getByText('Not configured')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(writes).toHaveBeenCalledTimes(2));
  expect(writes.mock.calls[1][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: {
    ...main, reasoning_profile_order: undefined,
  } } });
});

it('keeps reasoning membership coherent on add, delete, final delete and reset', async () => {
  const { request_params: _, ...rest } = cfg3Effective().document.models!.main;
  const main = { ...rest, default_profile: 'b', profiles: { a: { request_params: {} }, b: { request_params: {} } }, reasoning_profile_order: ['b', 'a'] };
  const source = catalogSource('user', { models: { main } });
  const { writes } = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), { source, context: source });
  const long = 'custom-profile-with-a-long-identity-'.repeat(5);
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  fireEvent.change(screen.getByLabelText('New profile identity'), { target: { value: long } });
  fireEvent.keyDown(screen.getByLabelText('New profile identity'), { key: 'Enter' });
  expect(screen.getByRole('status').textContent).toContain(`Profile ${long} was placed last`);
  const ids = () => [...screen.getByRole('list', { name: 'Reasoning order (low to high)' }).children].map(row => row.getAttribute('data-profile-order'));
  expect(ids()).toEqual(['b', 'a', long]);
  fireEvent.click(screen.getByRole('button', { name: 'Delete profile b' }));
  expect(ids()).toEqual(['a', long]);
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(writes).toHaveBeenCalledTimes(1));
  expect(writes.mock.calls[0][0]).toMatchObject({ mutation: { authored: { default_profile: 'a', reasoning_profile_order: ['a', long] } } });
  fireEvent.click(screen.getByRole('button', { name: 'Reset form' }));
  expect(ids()).toEqual(['b', 'a']);
  expect(screen.queryByRole('button', { name: `Profile ${long}` })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Delete profile a' }));
  fireEvent.click(screen.getByRole('button', { name: 'Delete profile b' }));
  expect(screen.queryByRole('list', { name: 'Reasoning order (low to high)' })).toBeNull();
  expect(screen.getByText('Add a profile to configure reasoning levels.')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(writes).toHaveBeenCalledTimes(2));
  expect(writes.mock.calls[1][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: {
    ...rest, profiles: undefined, default_profile: undefined, reasoning_profile_order: undefined,
  } } });
});

it('preserves source-authored protocol overrides when editing unrelated Model fields', async () => {
  const model = { ...cfg3Effective().document.models!.main, capabilities: { ...cfg3Effective().document.models!.main.capabilities }, default_profile: 'deep', reasoning_profile_order: ['quick', 'deep'], profiles: { deep: { max_output_tokens: 2048, request_params: { reasoning: { effort: 'high' }, stop: [null] } }, quick: { request_params: {} } }, compat: { chat_max_tokens_field: 'max_completion_tokens' as const, chat_stream_usage: 'supported' as const, chat_reasoning_replay: 'omit' as const, chat_tool_protocol: 'native' as const } };
  const source = catalogSource('workspace', { models: { main: model } });
  source.user.authored!.models = { main: model };
  const { writes: write, rerender } = await renderEditor(modelDetail(source, 'workspace', 'exact', 'main'), { source, context: source });
  // One unrelated field changes; everything else must survive the complete
  // object replacement untouched.
  fireEvent.change(screen.getByLabelText('Model ID'), { target: { value: 'new-wire' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...model, id: 'new-wire' } } }, 'exact'));
  rerender(<ModelsPage {...modelProbeFixture} source={source} scope="workspace" revision="exact"
    models={['main']} profiles={{}} onFocus={noop} onRefresh={noop} />);
  await confirmAction('Use global default Model main', within(screen.getByRole('row', { name: 'main' })));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: null } }, 'exact'));
});

it('never authors a protocol from the Model editor', async () => {
  const model = { ...cfg3Effective().document.models!.main, compat: { chat_reasoning_replay: 'reasoning' as const } };
  const source = catalogSource('user', { models: { main: model } });
  const { writes } = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), { source, context: source });
  expect(screen.queryByRole('button', { name: /protocol compatibility/i })).toBeNull();
  for (const label of ['Chat reasoning replay', 'Chat output field', 'Chat stream usage', 'Chat tool protocol', 'Responses storage']) {
    expect(screen.queryByLabelText(label)).toBeNull();
  }
  expect(within(screen.getByRole('dialog', { name: 'Model main' })).queryByLabelText('API format')).toBeNull();
  fireEvent.change(screen.getByLabelText('Model ID'), { target: { value: 'changed-wire' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(writes).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...model, id: 'changed-wire' } } }, 'r1'));
});

it('saves a Chat Model without authoring any protocol overrides', async () => {
  const model = { ...cfg3Effective().document.models!.main,  };
  delete model.compat;
  const source = catalogSource('user', { models: { main: model } });
  const { writes } = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), { source, context: source });
  fireEvent.change(screen.getByLabelText('Model ID'), { target: { value: 'plain-chat' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(writes).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...model, id: 'plain-chat' } } }, 'r1'));
});

it('authors general Model Profiles as complete presets, moving model parameters into the first one', async () => {
  const original = cfg3Effective().document.models!.main;
  const main = { ...original, capabilities: { ...original.capabilities }, request_params: { temperature: 0.2 } };
  const source = catalogSource('user', { models: { main } });
  const { writes: write } = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  fireEvent.change(screen.getByLabelText('New profile identity'), { target: { value: 'precise' } });
  fireEvent.keyDown(screen.getByLabelText('New profile identity'), { key: 'Enter' });
  const precise = within(screen.getByRole('group', { name: 'Profile precise' }));
  // The Model's own parameters seed the first profile; the Model no longer
  // declares any, and the new profile is the default.
  expect(JSON.parse(readJson(precise.getByLabelText('API parameters (JSON)')))).toEqual({ temperature: 0.2 });
  expect(precise.getByRole('button', { name: 'Format JSON: API parameters (JSON)' })).toBeTruthy();
  expect(precise.getByLabelText('Profile output limit (optional)')).toHaveProperty('value', '');
  const preciseValue = { max_output_tokens: 1024, request_params: { temperature: 0.1, top_p: 0.5, stop: ['\n', null], metadata: { tier: null } } };
  changeJson(precise.getByLabelText('API parameters (JSON)'), JSON.stringify(preciseValue.request_params));
  fireEvent.change(precise.getByLabelText('Profile output limit (optional)'), { target: { value: '1024' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  fireEvent.change(screen.getByLabelText('New profile identity'), { target: { value: 'creative' } });
  fireEvent.keyDown(screen.getByLabelText('New profile identity'), { key: 'Enter' });
  const creative = within(screen.getByRole('group', { name: 'Profile creative' }));
  // A later profile starts empty: profiles never inherit from one another.
  expect(JSON.parse(readJson(creative.getByLabelText('API parameters (JSON)')))).toEqual({});
  // Switching or adding a chip must not unmount an invalid JSON buffer and
  // silently restore the last valid parameters.
  changeJson(creative.getByLabelText('API parameters (JSON)'), '{"request_params":');
  fireEvent.click(screen.getByRole('button', { name: 'Profile precise' }));
  expect(readJson(screen.getByLabelText('API parameters (JSON)'))).toBe('{"request_params":');
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  expect(screen.queryByLabelText('New profile identity')).toBeNull();
  const creativeValue = { request_params: { temperature: 1.3 } };
  changeJson(creative.getByLabelText('API parameters (JSON)'), JSON.stringify(creativeValue.request_params));
  fireEvent.click(screen.getByRole('button', { name: 'Profile precise' }));
  expect(JSON.parse(readJson(screen.getByLabelText('API parameters (JSON)')))).toEqual(preciseValue.request_params);
  expect(screen.getByLabelText('Profile output limit (optional)')).toHaveProperty('value', '1024');
  fireEvent.click(screen.getByRole('button', { name: 'Profile creative' }));
  expect(JSON.parse(readJson(screen.getByLabelText('API parameters (JSON)')))).toEqual(creativeValue.request_params);
  expect(screen.getByLabelText('Profile output limit (optional)')).toHaveProperty('value', '');
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  const { request_params: _, ...rest } = main;
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...rest, default_profile: 'precise', profiles: {
    precise: preciseValue,
    creative: creativeValue,
  } } } });
  // Removing the default profile moves the default to a remaining one.
  fireEvent.click(screen.getByRole('button', { name: 'Delete profile precise' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save Model main' }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(2));
  expect(write.mock.calls[1][0]).toMatchObject({ mutation: { authored: { default_profile: 'creative', profiles: { creative: { request_params: { temperature: 1.3 } } } } } });
});

it.each(['all', 'none', 'exact'] as const)('changes one Skill from native %s membership without changing the others', async mode => {
 const source=workspaceSource();
 source.workspace!.authored={agent:{skills:mode==='all'?'all':mode==='none'?[]:['existing']}};
 source.prospective_resources={definitions:[{family:'skill',name:'review',valid:true,location:{scope:'workspace',path:'/review'}},{family:'skill',name:'existing',valid:true,location:{scope:'workspace',path:'/existing'}}]} as never;
 const {writes}=await renderEditor(<ResourceAvailability source={source} scope="workspace" revision="s1" family="skill" name="review" valid={true}/>,{source,context:source});
 fireEvent.click(screen.getByRole('switch'));
 await waitFor(()=>expect(writes).toHaveBeenCalledWith({kind:'config',mutation:{unit:'skills',authored:mode==='all'?['existing']:mode==='none'?['review']:['existing','review']}},'s1'));
});

it('keeps automatically composed Agent controls out of settings', async () => {
  // Issue #462 D03: native composes the group from the Available subagents alone.
  const controls = ['subagent', 'list_agents', 'send_message', 'wait_agent', 'interrupt_agent'];
  const source = workspaceSource();
  await renderEditor(toolsPage(source, {}, 'workspace', 'c1'), { source, context: source });
  const whitelist = within(screen.getByRole('group', { name: 'Available tools' }));
  expect(whitelist.getByRole('checkbox', { name: 'read' })).toBeTruthy();
  for (const name of controls) expect(whitelist.queryByRole('checkbox', { name })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Advanced Tool policies' }));
  for (const name of controls) expect(screen.queryByRole('form', { name: `${name} policy` })).toBeNull();
  expect(screen.queryByRole('note', { name: 'Agent controls' })).toBeNull();
  cleanup();
  const composed = workspaceSource();
  composed.prospective_resources = {
    definitions: [], resource_diagnostics: [], agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [],
    main: {
      identity: { kind: 'main' }, tools: [], tool_selection: [], skills: [], agents: ['reviewer'], workflows: [], plugins: [], diagnostics: [],
      agent_controls: controls.map(name => ({ id: `tool-${name}`, name, origin: 'builtin' as const })),
    },
  };
  await renderEditor(toolsPage(composed, {}, 'workspace', 'c2'), { source: composed, context: composed });
  expect(screen.queryByRole('note', { name: 'Agent controls' })).toBeNull();
});

it('writes Native and MCP invocation policies independently, and an empty policy only when explicitly authored', async () => {
  const context = workspaceSource();
  const { writes: write } = await renderEditor(<>{toolsPage(context, {}, 'workspace', 'p1')}<AvailabilityDetail source={context} scope="workspace" revision="p1" family="mcp" name="search"/></>, { source: context, context });
  // Per-Tool invocation policy is progressively disclosed rather than given a
  // page of its own.
  fireEvent.click(screen.getByRole('button', { name: 'Advanced Tool policies' }));
  const form = within(screen.getByRole('form', { name: 'bash policy' }));
  await chooseOption('execution', 'Model selectable', form);
  fireEvent.click(form.getByRole('button', { name: 'Save bash policy' }));
  await waitFor(() => expect(write.mock.calls[0]).toEqual([{ kind: 'config', mutation: { unit: 'native_policy', id: 'bash', authored: { execution: 'model_selectable' } } }, 'p1']));
  const mcp = within(screen.getByRole('form', { name: 'MCP policy search' }));
  // This Workspace authors no `search` policy; opening its editor authors none
  // either, so there is nothing to save until the user says so.
  expect((mcp.getByRole('button', { name: 'Save MCP policy search' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(mcp.getByRole('button', { name: 'Override MCP policy search' }));
  fireEvent.click(mcp.getByRole('button', { name: 'Save MCP policy search' }));
  // An explicitly authored empty policy object stays `{}` and is never null.
  await waitFor(() => expect(write.mock.calls[1][0]).toEqual({ kind: 'config', mutation: { unit: 'mcp_policy', id: 'search', authored: {} } }));
});

it('round-trips an independent Agent profile with guidance and model', async () => {
  const source = cfg3Source();
  const authored = { description: 'Review', instructions: 'Inspect', workflows: ['audit'], model: { model: 'main', profile: 'deep', max_output_tokens: { mode: 'catalog_default' as const }, auxiliary: { mode: 'session' as const }, request_params: { temperature: .5, nested: [null] } }, tools: { builtin: ['read'], sources: { 'python:analysis': 'all' as const, search: [] } }, skills: 'all' as const, agents_md: { inherit: false }, timeout_ms: '30000', plugins: { todo: { enabled: true }, goal: { enabled: false }, agent_status: { enabled: true, time: { enabled: false }, background: { enabled: true } } } };
  source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/workspace/.agents/agents/reviewer.toml', revision: 'agent-r1', authored } }];
  const { writes: write } = await renderEditor(<AgentEditor source={source} target={{ kind: "workspace", id: "A", displayName: "A" }} onClose={() => {}} models={['main']} profiles={{}} name="reviewer" />, { source, context: source });
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
  const { writes: write } = await renderEditor(id==='search'?<AvailabilityDetail source={{...source,user:{...source.user,authored:document}}} scope="user" revision="tools-r1" family="mcp" name="search"/>:toolsPage(source, document, 'user', 'tools-r1'), { source, context: source });
  const form = within(screen.getByRole('form', { name: `Tools: ${id}` }));
  for (const mode of ['all', 'none', 'exact'] as const) {
    await chooseOption('Selection', mode === 'all' ? 'All' : mode === 'none' ? 'None' : 'Exact identities', form);
    if (mode === 'exact') fireEvent.change(form.getByRole('textbox', { name: `${id} names 1` }), { target: { value: 'inspect' } });
    fireEvent.click(form.getByRole('button', { name: `Save Tools: ${id}` }));
    await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'source_tools', id, authored: mode === 'all' ? 'all' : mode === 'none' ? [] : ['inspect'] } }, 'tools-r1'));
  }
});

it('replaces default model intent on the Models page', async () => {
  const auxiliary = { mode: 'explicit' as const, model: 'summary' };
  const main = cfg3Effective().document.models!.main;
  const source = catalogSource('workspace', {
    providers: cfg3Source().user.authored!.providers,
    models: { main, other: { ...main, id: 'other' }, plain: { ...main, id: 'plain' } },
    agent: { model: {
      model: 'main', profile: 'quick', request_params: { temperature: .4 },
      max_output_tokens: { mode: 'limit', tokens: 4096 }, auxiliary,
    } },
  });
  const { writes: write } = await renderEditor(
    <ModelsPage {...modelProbeFixture} source={source} scope="workspace" revision="root-r1" models={['main', 'other', 'plain']}
      profiles={{ main: ['deep', 'quick'], other: ['quick', 'high'] }} onFocus={noop} onRefresh={noop} />,
    { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Default model for new Sessions' }));
  const model = within(screen.getByRole('form', { name: 'Default model' }));
  for (const label of ['Output limit', 'Request parameter overrides']) {
    expect(model.queryByLabelText(label, { exact: true })).toBeNull();
  }
  expect(model.getByRole('button', { name: 'Auxiliary models' })).toBeTruthy();
  await chooseOption('Profile', 'deep', model);
  fireEvent.click(model.getByRole('button', { name: 'Save Default model' }));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'root_model',
    authored: { model: 'main', profile: 'deep', auxiliary },
  } }, 'root-r1'));
  await chooseOption('Profile', 'quick', model);
  await chooseOption('Model', 'other', model);
  expect(model.getByRole('button', { name: /Profile$/ }).textContent).toContain('Model default profile');
  fireEvent.click(model.getByRole('button', { name: /Profile$/ }));
  expect(screen.queryByRole('option', { name: 'deep' })).toBeNull();
  fireEvent.click(screen.getByRole('option', { name: 'high' }));
  await chooseOption('Model', 'plain', model);
  expect(model.queryByRole('button', { name: /Profile$/ })).toBeNull();
  fireEvent.click(model.getByRole('button', { name: 'Save Default model' }));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'root_model',
    authored: { model: 'plain', auxiliary },
  } }, 'root-r1'));

  fireEvent.click(model.getByRole('button', { name: 'Auxiliary models' }));
  await chooseOption('Auxiliary model', 'other', model);
  await chooseOption('Auxiliary reasoning profile', 'Explicit profile', model);
  await chooseOption('Auxiliary profile', 'quick', model);
  fireEvent.click(model.getByRole('button', { name: 'Save Default model' }));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'root_model',
    authored: { model: 'plain',
      auxiliary: { mode: 'explicit', model: 'other', profile: { mode: 'explicit', id: 'quick' } } },
  } }, 'root-r1'));
});

it('saves the automatic project guidance switch', async () => {
  const { writes: write } = await renderEditor(<AgentPage config={{ document: {}, revision: 'root-r1' }} />);
  const guidance = within(screen.getByRole('form', { name: 'Project guidance' }));
  const automatic = guidance.getByRole('switch', { name: 'Automatically load project guidance' });
  expect(automatic.getAttribute('aria-checked')).toBe('true');
  expect(guidance.queryByRole('button', { name: 'Advanced options' })).toBeNull();
  fireEvent.click(automatic);
  fireEvent.click(guidance.getByRole('button', { name: 'Save Project guidance' }));
  await waitFor(() => expect(write).toHaveBeenLastCalledWith({ kind: 'config', mutation: { unit: 'project_guidance', authored: { inherit: false } } }, 'root-r1'));
});

it('a named Agent preserves and edits the complete explicit Auxiliary Model independently', async () => {
  const summary = { mode: 'explicit' as const, model: 'summary-a', profile: { mode: 'explicit' as const, id: 'deep' }, max_output_tokens: { mode: 'limit' as const, tokens: 2048 }, request_params: { temperature: .2 } };
  const model = { model: 'main', profile: 'fast', request_params: { temperature: .7 }, auxiliary: summary };
  const models = ['main', 'summary-a', 'summary-b'];
  const profiles = { main: ['fast'], 'summary-a': ['deep'], 'summary-b': ['deep', 'quick'] };
  const source = cfg3Source();
  source.target = { kind: 'workspace', directory: '/workspace/A' };
  source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/agent.toml', revision: 'r1', authored: { model } } }];
  const { writes: write } = await renderEditor(<AgentEditor source={source} target={{ kind: "workspace", id: "A", displayName: "A" }} onClose={() => {}} models={models} profiles={profiles} name="reviewer" />, { source, context: source });
  const commit = async (expected: ModelLayer) => {
    const before = write.mock.calls.length;
    const form = screen.getByRole('form', { name: 'Agent reviewer' }) as HTMLFormElement;
    expect(form.checkValidity()).toBe(true);
    fireEvent.click(within(form).getByRole('button', { name: 'Save Agent reviewer' }));
    await waitFor(() => expect(write).toHaveBeenCalledTimes(before + 1));
    expect(write.mock.calls.at(-1)).toEqual([{ kind: 'agent', name: 'reviewer', authored: { model: expected } }, 'r1']);
  };
  // Changing identity must not discard any nested authored intent.
  await chooseOption('Auxiliary model', 'summary-b');
  await commit({ ...model, auxiliary: { ...summary, model: 'summary-b' } });
  const nested = within(screen.getByRole('group', { name: 'Auxiliary model settings' }));
  await chooseOption('Auxiliary reasoning profile', 'Explicit profile');
  await chooseOption('Auxiliary profile', 'quick');
  fireEvent.change(nested.getByLabelText('Output limit (Auxiliary)'), { target: { value: '1024' } });
  changeJson(nested.getByLabelText('Request parameter overrides (Auxiliary)'), '{"temperature": 0.4, "seed": null}');
  await commit({ ...model, auxiliary: { ...summary, model: 'summary-b', profile: { mode: 'explicit', id: 'quick' }, max_output_tokens: { mode: 'limit', tokens: 1024 }, request_params: { temperature: .4, seed: null } } });
  await chooseOption('Auxiliary reasoning profile', 'Model default');
  fireEvent.change(nested.getByLabelText('Output limit (Auxiliary)'), { target: { value: '' } });
  const { profile: _, ...withoutProfile } = summary;
  await commit({ ...model, auxiliary: { ...withoutProfile, model: 'summary-b', profile: { mode: 'model_default' }, max_output_tokens: { mode: 'catalog_default' }, request_params: { temperature: .4, seed: null } } });
  await chooseOption('Auxiliary model', 'Follow selected model');
  await commit({ ...model, auxiliary: { mode: 'session', profile: { mode: 'lowest' } } });
  await chooseOption('Auxiliary model', 'summary-a');
  await commit({ ...model, auxiliary: { mode: 'explicit', model: 'summary-a', profile: { mode: 'lowest' } } });
});

it('the explicit Auxiliary Model variant owns complete labels in each locale without changing native payloads', async () => {
  const summary = { mode: 'explicit' as const, model: 'summary-a', profile: { mode: 'explicit' as const, id: 'deep' }, max_output_tokens: { mode: 'limit' as const, tokens: 2048 }, request_params: { temperature: .2 } };
  const model = { model: 'main', profile: 'main-profile', max_output_tokens: { mode: 'limit' as const, tokens: 4096 }, request_params: { temperature: .7 }, auxiliary: summary };
  const source = cfg3Source();
  source.agents = [{ name: 'reviewer', scope: 'workspace', source: { path: '/agent.toml', revision: 'r1', authored: { model } } }];
  const { writes: write, context } = await renderEditor(<AgentEditor source={source} target={{ kind: "workspace", id: "A", displayName: "A" }} onClose={() => {}} models={['main', 'summary-a']} profiles={{ main: ['main-profile'], 'summary-a': ['deep', 'quick'] }} name="reviewer" />, { source, context: source });
  const labels = (locale: 'en' | 'zh') => {
    const tx = translator(locale);
    const group = screen.getByRole('group', { name: tx('settings:models-page.auxiliary-model-settings') });
    const nested = within(group);
    return {
      group,
      profile: screen.getByRole('button', { name: (name: string) => name.endsWith(tx('settings:models-page.auxiliary-profile')) }),
      limit: nested.getByLabelText(tx('settings:models-page.output-limit-auxiliary')) as HTMLInputElement,
      params: nested.getByLabelText(tx('settings:models-page.request-parameter-overrides-auxiliary')) as HTMLElement,
      outer: screen.getByLabelText(tx('settings:models-page.request-parameter-overrides')) as HTMLElement,
    };
  };
  const english = labels('en');
  expect([english.limit.value, readJson(english.params), readJson(english.outer)]).toEqual(['2048', '{\n  "temperature": 0.2\n}', '{\n  "temperature": 0.7\n}']);
  expect(english.profile.textContent).toContain('deep');
  expect(screen.getByLabelText('Output limit (Auxiliary)')).toBe(english.limit);
  const snapshot = context();

  act(() => localeController.setLocale('zh'));
  const chinese = labels('zh');
  expect([chinese.limit, chinese.params, chinese.outer]).toEqual([english.limit, english.params, english.outer]);
  expect(screen.getByLabelText('输出上限（辅助）')).toBe(english.limit);
  expect(screen.getByLabelText('请求参数覆盖（辅助）')).toBe(english.params);
  expect(screen.getByRole('button', { name: /辅助配置档$/ })).toBe(chinese.profile);
  expect(within(chinese.group).queryAllByRole('button', { name: /Summary|Auxiliary/ })).toEqual([]);
  expect(chinese.group.textContent).not.toMatch(/Summary|Profile|Output limit|Request parameter/);
  expect(write).not.toHaveBeenCalled();
  expect(context()).toBe(snapshot);

  fireEvent.change(chinese.limit, { target: { value: '1024' } });
  const zh = translator('zh');
  const title = zh('settings:extension-detail.agent-value', { p0: 'reviewer' });
  const form = screen.getByRole('form', { name: title });
  fireEvent.click(within(form).getByRole('button', { name: `${zh('settings:bridge.save')} ${title}` }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0]).toEqual([{ kind: 'agent', name: 'reviewer', authored: { model: {
    ...model, auxiliary: { ...summary, max_output_tokens: { mode: 'limit', tokens: 1024 } },
  } } }, 'r1']);
});

it.each([{}, { description: 'Review' }, { instructions: 'Inspect' }])('saves an Agent with optional profile text omitted: %j', async authored => {
  const source = cfg3Source();
  source.agents = [{ name: 'optional', scope: 'workspace', source: { path: '/agent.toml', revision: 'optional-r1', authored } }];
  const { writes: write } = await renderEditor(<AgentEditor source={source} target={{ kind: "workspace", id: "A", displayName: "A" }} onClose={() => {}} models={[]} profiles={{}} name="optional" />, { source, context: source });
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
  return { ...editor, main, input: screen.getByLabelText('Model request parameters') as HTMLElement };
}
const invalidJson = 'Not valid JSON. Nothing is saved until the text is one complete JSON object.';
const save = () => screen.getByRole('button', { name: 'Save Model main' }) as HTMLButtonElement;

it('keeps invalid Model parameters mounted until repaired before creating the first profile', async () => {
  const { writes, main, input } = await requestParameters();
  changeJson(input, '{"temperature":');
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  expect(screen.queryByLabelText('New profile identity')).toBeNull();
  expect(screen.getByLabelText('Model request parameters')).toBe(input);
  expect(readJson(input)).toBe('{"temperature":');
  expect(jsonValidity(input).validity.valid).toBe(false);
  expect(screen.getByRole('alert').textContent).toBe(invalidJson);
  expect(writes).not.toHaveBeenCalled();
  changeJson(input, '{"temperature": 0.3}');
  fireEvent.click(screen.getByRole('button', { name: 'Add profile' }));
  fireEvent.change(screen.getByLabelText('New profile identity'), { target: { value: 'precise' } });
  fireEvent.keyDown(screen.getByLabelText('New profile identity'), { key: 'Enter' });
  expect(JSON.parse(readJson(screen.getByLabelText('API parameters (JSON)')))).toEqual({ temperature: 0.3 });
  fireEvent.click(save());
  const { request_params: _, ...rest } = main;
  await waitFor(() => expect(writes).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: {
    ...rest, default_profile: 'precise', profiles: { precise: { request_params: { temperature: 0.3 } } },
  } } }, 'r1'));
});

it('a pasted object with nested values, arrays and null becomes the structured draft', async () => {
  const { writes: write, main, input } = await requestParameters();
  expect(readJson(input)).toBe('{\n  "temperature": 1,\n  "top_p": 0.5\n}');
  changeJson(input, '{"provider": {"order": ["a", "b"], "fallback": null}, "stop": [null, "\\n"], "seed": null}');
  expect(jsonValidity(input).validity.valid).toBe(true);
  fireEvent.click(save());
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...main,
    request_params: { provider: { order: ['a', 'b'], fallback: null }, stop: [null, '\n'], seed: null } } } });
});

it('an actor-owned reset replaces a mounted JSON buffer, its error and its validity, and a later edit starts from it', async () => {
  const { writes: write, main, input } = await requestParameters();
  // A complete object becomes the actor's draft; incomplete text after it
  // stays in the editor with its error and its invalid state.
  changeJson(input, '{"temperature": 2}');
  changeJson(input, '{"temperature": 2');
  expect(readJson(input)).toBe('{"temperature": 2');
  expect(screen.getByText(invalidJson)).toBeTruthy();
  expect(jsonValidity(input).validity.valid).toBe(false);
  // Discarding the draft is the transaction owner's reset; the same editor
  // stays mounted across it and follows the owner's value.
  fireEvent.click(screen.getByRole('button', { name: 'Reset form' }));
  await waitFor(() => expect((screen.getByRole('button', { name: 'Reset form' }) as HTMLButtonElement).disabled).toBe(true));
  expect(screen.getByLabelText('Model request parameters')).toBe(input);
  expect(readJson(input)).toBe('{\n  "temperature": 1,\n  "top_p": 0.5\n}');
  expect(screen.queryByText(invalidJson)).toBeNull();
  expect(jsonValidity(input).validity.valid).toBe(true);
  expect(jsonValidity(input).validationMessage).toBe('');
  // Following the owner wrote nothing back and began no draft.
  expect(save().disabled).toBe(true);
  changeJson(input, readJson(input).replace('0.5', '0.25'));
  fireEvent.click(save());
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...main, request_params: { temperature: 1, top_p: 0.25 } } } });
});

/** One deferred value, resolved explicitly by the test. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(settle => { resolve = settle; });
  return { promise, resolve };
}
const canonical = '{\n  "temperature": 1,\n  "top_p": 0.5\n}';
/** The same Model detail as `requestParameters`, with the native write and the
 * reads after the first under the test's control, so source observations and
 * post-commit rereads arrive exactly when the test says. */
async function ownedRequestParameters() {
  const main = { ...cfg3Effective().document.models!.main, request_params: { temperature: 1, top_p: 0.5 } };
  const source = catalogSource('user', { models: { main } });
  const reads: Array<ReturnType<typeof deferred<SourceSettings>>> = [];
  const writes: Array<ReturnType<typeof deferred<WriteOutcome>>> = [];
  const editor = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), {
    source, context: source,
    write: () => { const next = deferred<WriteOutcome>(); writes.push(next); return next.promise; },
    reread: () => { const next = deferred<SourceSettings>(); reads.push(next); return next.promise; },
  });
  const input = screen.getByLabelText('Model request parameters') as HTMLElement;
  /** A native source revision carrying `models`, as the authoritative read
   * and the Settings page that renders it both see it. */
  const revision = (name: string, models: Record<string, unknown>) => {
    const next = sameRevision(catalogSource('user', { models }), name);
    return next;
  };
  /** Deliver one authoritative observation to the actor and the page. */
  const observe = async (next: SourceSettings) => {
    await act(async () => { editor.actor.send({ type: 'REFRESH' }); });
    await waitFor(() => expect(reads.length).toBeGreaterThan(0));
    await act(async () => { reads.shift()!.resolve(structuredClone(next)); });
    await waitFor(() => expect(editor.context().observation?.user.revision).toBe(next.user.revision));
    editor.rerender(modelDetail(next, 'user', next.user.revision, 'main'));
  };
  return { ...editor, main, input, revision, observe, reads, nativeWrites: writes };
}
/** The editor shows exactly the owner's value again: canonical text, no
 * diagnostic, valid, and the same mounted element. */
function restored(input: HTMLElement, text = canonical) {
  expect(screen.getByLabelText('Model request parameters')).toBe(input);
  expect(readJson(input)).toBe(text);
  expect(screen.queryByRole('alert')).toBeNull();
  expect(input.getAttribute('aria-invalid')).not.toBe('true');
  expect(jsonValidity(input).validity.valid).toBe(true);
  expect(jsonValidity(input).validationMessage).toBe('');
}
/** Unparsed text held by the editor alone, diagnosed and invalid. */
function unparsed(input: HTMLElement, text = '{"temperature": 2') {
  changeJson(input, text);
  expect(readJson(input)).toBe(text);
  expect(screen.getByText(invalidJson)).toBeTruthy();
  expect(input.getAttribute('aria-invalid')).toBe('true');
  expect(jsonValidity(input).validity.valid).toBe(false);
}
const contextWindow = () => screen.getByLabelText('Context window') as HTMLInputElement;
const discardButton = () => screen.queryAllByRole('button', { name: 'Reset form' }).find(button => !(button as HTMLButtonElement).disabled) ?? null;

it('Discard resets unparsed JSON even when the discarded draft never changed request_params', async () => {
  const { writes: write, main, input } = await requestParameters();
  // The draft is an unrelated field; the JSON text never became part of it.
  fireEvent.change(contextWindow(), { target: { value: '64000' } });
  unparsed(input);
  fireEvent.click(discardButton()!);
  await waitFor(() => expect(discardButton()).toBeNull());
  restored(input);
  expect(contextWindow().value).toBe(String(main.context_window));
  expect(save().disabled).toBe(true);
  // A later valid edit starts from the owner's value and is the only write.
  changeJson(input, '{"temperature": 0.3}');
  fireEvent.click(save());
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...main, request_params: { temperature: 0.3 } } } });
});

it('an unrelated source observation keeps unparsed JSON, and the explicit reviewed revision resets it', async () => {
  const { main, input, revision, observe, nativeWrites } = await ownedRequestParameters();
  // No draft yet: only unparsed text. A new source revision that leaves this
  // Model's request_params untouched is not an owner reset of the text.
  unparsed(input);
  await observe(revision('r2', { main: { ...main, max_output_tokens: main.max_output_tokens - 1 } }));
  expect(readJson(input)).toBe('{"temperature": 2');
  expect(screen.getByText(invalidJson)).toBeTruthy();
  expect(jsonValidity(input).validity.valid).toBe(false);
  // A draft of another field, then a conflicting external revision: the
  // unparsed text still survives the observation itself…
  fireEvent.change(contextWindow(), { target: { value: '64000' } });
  unparsed(input, '{"top_p": ');
  await observe(revision('r3', { main: { ...main, max_output_tokens: main.max_output_tokens - 2 } }));
  expect(await screen.findByRole('button', { name: 'Use latest setting' })).toBeTruthy();
  expect(readJson(input)).toBe('{"top_p": ');
  // …and the explicit review gesture is the owner reset: the editor shows the
  // draft's own request_params, which the draft keeps, fenced on r3.
  fireEvent.click(screen.getByRole('button', { name: 'Use latest setting' }));
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Use latest setting' })).toBeNull());
  restored(input);
  expect(contextWindow().value).toBe('64000');
  expect(discardButton()).toBeTruthy();
  expect(nativeWrites).toHaveLength(0);
  changeJson(input, '{"temperature": 0.3}');
  fireEvent.click(save());
  await waitFor(() => expect(nativeWrites).toHaveLength(1));
});

it('the authoritative observation that settles a commit resets unparsed JSON even when request_params never changed', async () => {
  const { writes: write, main, input, revision, nativeWrites, reads, rerender } = await ownedRequestParameters();
  // The committed draft is an unrelated field, so request_params presents the
  // same value before the commit, after it, and in the reread.
  fireEvent.change(contextWindow(), { target: { value: '64000' } });
  fireEvent.click(save());
  await waitFor(() => expect(nativeWrites).toHaveLength(1));
  const committed = revision('r2', { main: { ...main, context_window: '64000' } });
  await act(async () => { nativeWrites[0]!.resolve({ acknowledgement: sourceReceipt(committed) }); });
  // Until the commit is observed the unit accepts no edit; text typed now is
  // the editor's alone.
  unparsed(input, '{"temperature": 3');
  await waitFor(() => expect(reads.length).toBeGreaterThan(0));
  await act(async () => { reads.shift()!.resolve(structuredClone(committed)); });
  rerender(modelDetail(committed, 'user', 'r2', 'main'));
  await waitFor(() => expect(readJson(input)).toBe(canonical));
  restored(input);
  expect(contextWindow().value).toBe('64000');
  expect(discardButton()).toBeNull();
  expect(write).toHaveBeenCalledTimes(1);
  // Editing resumes from the committed value, fenced on its revision.
  changeJson(input, '{"temperature": 4}');
  fireEvent.click(save());
  await waitFor(() => expect(nativeWrites).toHaveLength(2));
  expect(write.mock.calls[1]).toEqual([{ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...main, context_window: '64000', request_params: { temperature: 4 } } } }, 'r2']);
});

it('an unrelated Model edit resubmits binary64-exact request parameters exactly as native emitted them', async () => {
  // The projection exactly as the App Server client decodes response text.
  const emitted = '{"seed":9007199254740992,"floor":-9007199254740991,"big":10000000000000000000,"tiny":5.357830195732913e-76,"huge":1e300,"nested":{"list":[0.1,{"n":-2.5e-7}]}}';
  const main = { ...cfg3Effective().document.models!.main, request_params: JSON.parse(emitted) as Record<string, unknown> };
  const source = catalogSource('user', JSON.parse(JSON.stringify({ models: { main } })) as Record<string, unknown>);
  const { writes: write } = await renderEditor(modelDetail(source, 'user', 'r1', 'main'), { source, context: source });
  fireEvent.change(screen.getByLabelText('Context window'), { target: { value: '64000' } });
  fireEvent.click(save());
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  // The mutation text the client sends carries every number unchanged, so
  // native reads back the very values it emitted.
  const sent = JSON.stringify(write.mock.calls[0][0]);
  const params = (JSON.parse(sent) as { mutation: { authored: { request_params: unknown } } }).mutation.authored.request_params;
  expect(JSON.stringify(params)).toBe(JSON.stringify(JSON.parse(emitted)));
  for (const literal of ['9007199254740992', '-9007199254740991', '10000000000000000000', '5.357830195732913e-76', '1e+300', '0.1', '-2.5e-7'])
    expect(sent).toContain(literal);
});

const duplicate = (line: number, column: number) => `The JSON text repeats an object key at line ${line}, column ${column}.`;
const inexact = (line: number, column: number) => `The number at line ${line}, column ${column} cannot be kept exactly: rustX, the browser and providers read JSON numbers as IEEE 754 binary64 values, and this one would be rounded. Nothing is saved until it is changed.`;

it.each([
  ['{"budget":', invalidJson],
  ['[1, 2]', 'Request parameters must be a JSON object.'],
  ['null', 'Request parameters must be a JSON object.'],
  ['{"a": 1, "a": 2}', duplicate(1, 10)],
  ['{"outer": {"list": [{"k": 1, "k": 1}]}}', duplicate(1, 30)],
  // A key is located, never quoted: it may itself hold a secret.
  ['{"sk-SECRET [x] \\"q\\"": 1, "sk-SECRET [x] \\"q\\"": 2}', duplicate(1, 28)],
  // A number JSON.parse would round is refused on its text, before parsing.
  ['{"seed": 9007199254740993}', inexact(1, 10)],
  ['{\n  "a": [1.5, {"b": -9007199254740993}]\n}', inexact(2, 20)],
  ['{"t": 0.12345678901234567890}', inexact(1, 7)],
])('invalid text %s stays visible and diagnosed locally and never reaches the draft', async (text, diagnostic) => {
  const { writes: write, main, input } = await requestParameters();
  changeJson(input, text);
  expect(readJson(input)).toBe(text);
  expect(screen.getByText(diagnostic)).toBeTruthy();
  expect(jsonValidity(input).validationMessage).toBe(diagnostic);
  expect(jsonValidity(input).validity.valid).toBe(false);
  // No draft exists: the invalid text was never offered to the actor.
  expect(save().disabled).toBe(true);
  expect((screen.getByRole('button', { name: 'Reset form' }) as HTMLButtonElement).disabled).toBe(true);
  // Correcting it is an ordinary edit; the buffer is not reset by its own echo.
  changeJson(input, '{"budget": 32}');
  expect(readJson(input)).toBe('{"budget": 32}');
  expect(screen.queryByText(diagnostic)).toBeNull();
  expect(jsonValidity(input).validity.valid).toBe(true);
  fireEvent.click(save());
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: { ...main, request_params: { budget: 32 } } } });
});

it('whitespace and key order alone are not an edit and begin no draft', async () => {
  const { writes: write, input } = await requestParameters();
  changeJson(input, '{ "top_p" : 0.5,\n\n   "temperature":1 }');
  expect(readJson(input)).toBe('{ "top_p" : 0.5,\n\n   "temperature":1 }');
  expect(jsonValidity(input).validity.valid).toBe(true);
  expect(save().disabled).toBe(true);
  expect((screen.getByRole('button', { name: 'Reset form' }) as HTMLButtonElement).disabled).toBe(true);
  expect(write).not.toHaveBeenCalled();
});

it.each([['  ', false], ['{}', true]] as const)('blank optional parameters are absent, while an explicit empty object is authored: %j', async (text, present) => {
  const { writes: write, main, input } = await requestParameters();
  const { request_params: _, ...absent } = main;
  changeJson(input, text);
  fireEvent.click(save());
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0][0]).toEqual({ kind: 'config', mutation: { unit: 'model', id: 'main', authored: present ? { ...absent, request_params: {} } : absent } });
});

it('a portaled Model editor closes its native fields when transport is lost, while its close control remains available', async () => {
  const source = catalogSource('user', { models: { main: cfg3Effective().document.models!.main } });
  const { actor, writes: write } = await renderEditor(modelDetail(source,'user','r1','main'), {source,context:source});
  const input = screen.getByLabelText('Model ID') as HTMLInputElement;
  fireEvent.change(input,{target:{value:'preserved-wire-id'}});
  await act(async () => actor.send({type:'TRANSPORT',connection:'disconnected',generation:1,publication:undefined}));
  expect(input.matches(':disabled')).toBe(true);
  expect(screen.getByText('Disconnected. Reconnect to edit or save.')).toBeTruthy();
  expect((screen.getByRole('button',{name:'Close'}) as HTMLButtonElement).disabled).toBe(false);
  expect(input.value).toBe('preserved-wire-id');
  expect(write).not.toHaveBeenCalled();
});
