import { providerAction } from './settings-harness';
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { cfg3Client, cfg3Host } from './cfg3-fixture';
import { cfg3Effective, cfg3Source } from './cfg3-data';
import { chooseOption, openSettingsPage, renderEditor, settingsReady, SettingsSurface } from './settings-harness';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { ModelsPage } from '../src/app/settings/models/ModelsPage';
import { OutcomeUncertain, RpcFailure } from '../src/client/errors';

afterEach(cleanup);
const writes = (subject: ReturnType<typeof cfg3Client>) => subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite');
async function openModels(subject: ReturnType<typeof cfg3Client>) {
  render(<SettingsSurface client={subject.client} target={userSettingsTarget} />);
  await settingsReady();
  await openSettingsPage('Models');
  fireEvent.click(screen.getByRole('row', { name: 'transport' }));
}
async function addModel(subject: ReturnType<typeof cfg3Client>) {
  await openModels(subject);
  fireEvent.click(screen.getByRole('button', { name: '+ Add Model' }));
  return within(screen.getByRole('dialog', { name: 'Add Model' }));
}

it('a model is added in one dialog, confirmed by native observation, and listed under its provider', async () => {
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method !== 'configuration/sourceWrite') return;
    const mutation = operation.params.mutation;
    if (mutation.kind === 'config' && mutation.mutation.unit === 'model') {
      source.user.authored!.models = { ...source.user.authored!.models, [mutation.mutation.id]: mutation.mutation.authored! };
    }
  });
  const modal = await addModel(subject);
  expect(modal.queryByLabelText('New Model identity')).toBeNull();
  expect(modal.queryByLabelText('API format')).toBeNull();
  fireEvent.change(modal.getByLabelText('Model ID'), { target: { value: 'deepseek-flash' } });
  fireEvent.click(modal.getByRole('button', { name: 'Save Add Model' }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Add Model' })).toBeNull());
  expect(writes(subject)).toHaveLength(1);
  const operation = writes(subject)[0][0];
  if (operation.method === 'configuration/sourceWrite' && operation.params.mutation.kind === 'config' && operation.params.mutation.mutation.unit === 'model') {
    expect(operation.params.mutation.mutation.authored).not.toHaveProperty('protocol');
  } else throw new Error('Expected a native Model write');
  expect(writes(subject)[0][0]).toMatchObject({ params: { mutation: { mutation: { id: 'transport/deepseek-flash', authored: { provider: 'transport', id: 'deepseek-flash' } } } } });
  expect(await screen.findByRole('row', { name: 'transport/deepseek-flash' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: '+ Add Model' }));
  const second = within(screen.getByRole('dialog', { name: 'Add Model' }));
  fireEvent.change(second.getByLabelText('Model ID'), { target: { value: 'deepseek-flash' } });
  fireEvent.submit(second.getByRole('form'));
  await screen.findByText(/This configuration name already exists\./);
  fireEvent.click(second.getByRole('button', { name: 'Reset form' }));
  await waitFor(() => expect((second.getByLabelText('Model ID') as HTMLInputElement).value).toBe(''));
  expect(writes(subject)).toHaveLength(1);
});

it('changes API format once on the provider without exposing protocol identifiers on model rows', async () => {
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method !== 'configuration/sourceWrite') return;
    const mutation = operation.params.mutation;
    if (mutation.kind === 'config' && mutation.mutation.unit === 'provider' && mutation.mutation.authored) {
      source.user.authored!.providers!.transport.protocol = mutation.mutation.authored.protocol;
    }
  });
  const model = { provider: 'transport', id: 'one', context_window: '128000', max_output_tokens: 8192, capabilities: { input_modalities: ['text' as const], output_modalities: ['text' as const], json_schema_output: false } };
  subject.source.user.authored!.models = { one: model, two: { ...model, id: 'two' } };
  await openModels(subject);
  await chooseOption('API format', 'Anthropic Messages (/v1/messages)');
  await waitFor(() => expect(writes(subject)).toHaveLength(1));
  expect(writes(subject)[0][0]).toMatchObject({ params: { mutation: { mutation: { unit: 'provider', id: 'transport', authored: { protocol: 'anthropic_messages', credential: { kind: 'retain' } } } } } });
  await waitFor(() => {
    for (const id of ['one', 'two']) expect(within(screen.getByRole('row', { name: id })).queryByText('anthropic_messages')).toBeNull();
    expect(screen.getByRole('button', { name: /API format$/ }).textContent).toContain('Anthropic Messages');
  });
  fireEvent.click(screen.getByRole('button', { name: '+ Add Model' }));
  expect(within(screen.getByRole('dialog', { name: 'Add Model' })).queryByLabelText('API format')).toBeNull();
});

it('autosaves a newer Provider edit after the previous write is observed', async () => {
  let release!: () => void;
  const held = new Promise<void>(done => { release = done; });
  let revision = 1;
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method !== 'configuration/sourceWrite' || operation.params.mutation.kind !== 'config') return;
    const mutation = operation.params.mutation.mutation;
    if (mutation.unit !== 'provider' || !mutation.authored) return;
    if (revision === 1) await held;
    source.user.authored!.providers!.transport.base_url = mutation.authored.base_url;
    source.user.revision = `user-${++revision}`;
    return {type:'sources_written', revision:source.user.revision, configuration_revision:null};
  });
  await openModels(subject);
  const url = screen.getByLabelText('Base URL');
  fireEvent.change(url, {target:{value:'https://first.example/v1'}});
  fireEvent.keyDown(url, {key:'Enter'});
  await waitFor(() => expect(writes(subject)).toHaveLength(1));
  fireEvent.change(url, {target:{value:'https://latest.example/v1'}});
  fireEvent.keyDown(url, {key:'Enter'});
  expect(writes(subject)).toHaveLength(1);
  await act(async () => release());
  await waitFor(() => expect(writes(subject)).toHaveLength(2));
  expect(writes(subject)[1][0]).toMatchObject({params:{expected_revision:'user-2', mutation:{mutation:{
    id:'transport', authored:{base_url:'https://latest.example/v1', credential:{kind:'retain'}},
  }}}});
});

it('offers a manual Provider retry once an uncertain write is reread at the same revision', async () => {
  let attempts = 0;
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method !== 'configuration/sourceWrite') return;
    if (++attempts === 1) throw new OutcomeUncertain();
    source.user.authored!.providers!.transport.base_url = 'https://retry.example/v1';
    source.user.revision = 'user-2';
    return { type: 'sources_written', revision: 'user-2', configuration_revision: null };
  });
  await openModels(subject);
  const url = screen.getByLabelText('Base URL');
  fireEvent.change(url, { target: { value: 'https://retry.example/v1' } });
  fireEvent.keyDown(url, { key: 'Enter' });
  await waitFor(() => expect(subject.request.mock.calls.filter(([request]) => request.method === 'configuration/sourcesRead')).toHaveLength(2));
  expect(writes(subject)).toHaveLength(1);
  await providerAction('Retry');
  await waitFor(() => expect(writes(subject)).toHaveLength(2));
  expect(writes(subject)[1][0]).toMatchObject({
    params: { expected_revision: 'user-1', mutation: { mutation: { id: 'transport', authored: { base_url: 'https://retry.example/v1' } } } },
  });
});

it('invalid model limits and duplicate model identities never reach native writes', async () => {
  const subject = cfg3Client();
  subject.source.user.authored!.models = { 'transport/duplicate': { provider: 'transport', id: 'duplicate',  context_window: '128000', max_output_tokens: 8192, capabilities: { input_modalities: ['text'], output_modalities: ['text'], json_schema_output: false } } };
  const modal = await addModel(subject);
  fireEvent.change(modal.getByLabelText('Model ID'), { target: { value: 'duplicate' } });
  fireEvent.submit(modal.getByRole('form'));
  await screen.findByText(/This configuration name already exists\./);
  expect(writes(subject)).toHaveLength(0);
  fireEvent.change(modal.getByLabelText('Model ID'), { target: { value: 'new' } });
  fireEvent.change(modal.getByLabelText('Context window'), { target: { value: '1000' } });
  fireEvent.submit(modal.getByRole('form'));
  await screen.findByText(/Maximum output tokens must be smaller than the context window\./);
  expect(writes(subject)).toHaveLength(0);
  fireEvent.change(modal.getByLabelText('Context window'), { target: { value: '128000' } });
  await waitFor(() => expect(screen.queryByText(/Maximum output tokens must be smaller than the context window/)).toBeNull());
});

it('a refused save keeps the model dialog and all input; Cancel discards the creation draft', async () => {
  const subject = cfg3Client(async operation => {
    if (operation.method === 'configuration/sourceWrite') throw new RpcFailure({ code: -32602, message: 'Native rejected this model', data: { kind: 'invalid_params' } });
  });
  const modal = await addModel(subject);
  fireEvent.change(modal.getByLabelText('Model ID'), { target: { value: 'retained' } });
  fireEvent.click(modal.getByRole('button', { name: 'Save Add Model' }));
  await screen.findByText(/Native rejected this model/);
  expect((modal.getByLabelText('Model ID') as HTMLInputElement).value).toBe('retained');
  fireEvent.click(modal.getByRole('button', { name: 'Cancel' }));
  fireEvent.click(screen.getByRole('button', { name: '+ Add Model' }));
  expect((screen.getByLabelText('Model ID') as HTMLInputElement).value).toBe('');
});

it('invalid token integers are rejected before serialization to native quantities', async () => {
  const subject = cfg3Client();
  const modal = await addModel(subject);
  fireEvent.change(modal.getByLabelText('Model ID'), { target: { value: 'new' } });
  for (const value of ['1e5', '128000.5', '18446744073709551616']) {
    fireEvent.change(modal.getByLabelText('Context window'), { target: { value } });
    fireEvent.submit(modal.getByRole('form'));
    await screen.findByText(/Enter a positive whole number within the supported range\./);
    expect(writes(subject)).toHaveLength(0);
  }
  fireEvent.change(modal.getByLabelText('Context window'), { target: { value: '128000' } });
  for (const value of ['1.5', '4294967296']) {
    fireEvent.change(modal.getByLabelText('Maximum output tokens'), { target: { value } });
    fireEvent.submit(modal.getByRole('form'));
    await screen.findByText(/Enter a positive whole number within the supported range\./);
    expect(writes(subject)).toHaveLength(0);
  }
});

it('an untouched required field is validated on submission, and a new provider cannot add models before saving', async () => {
  const subject = cfg3Client();
  await openModels(subject);
  fireEvent.click(screen.getByRole('button', { name: '+ Add Provider' }));
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'new-provider' } });
  fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
  expect((screen.getByRole('button', { name: '+ Add Model' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(screen.getByLabelText('API Key'), { target: { value: 'fixture-only' } });
  fireEvent.submit(screen.getByRole('form', { name: 'Provider new-provider' }));
  await screen.findByText(/Use an HTTP or HTTPS URL/);
  expect(writes(subject)).toHaveLength(0);
});

it('switching credential modes removes the previous mode and its secret from the submitted provider', async () => {
  const subject = cfg3Client();
  await openModels(subject);
  if (!screen.queryByLabelText('API Key')) await providerAction('Use an API Key');
  fireEvent.change(screen.getByLabelText('API Key'), { target: { value: 'DISCARDED_FIXTURE_SECRET' } });
  await providerAction('Use an environment variable');
  fireEvent.change(screen.getByLabelText('Environment variable'), { target: { value: 'PROVIDER_KEY' } });
  fireEvent.keyDown(screen.getByLabelText('Environment variable'), { key: 'Enter' });
  await waitFor(() => expect(writes(subject)).toHaveLength(1));
  expect(writes(subject)[0][0]).toMatchObject({ params: { mutation: { mutation: { authored: { credential: { kind: 'environment', variable: 'PROVIDER_KEY' } } } } } });
  expect(JSON.stringify(writes(subject))).not.toContain('DISCARDED_FIXTURE_SECRET');
  expect(JSON.stringify(writes(subject))).not.toContain('"value"');
});

it('automatically saves provider icons and retains saved credentials', async () => {
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method !== 'configuration/sourceWrite') return;
    const mutation = operation.params.mutation;
    if (mutation.kind === 'config' && mutation.mutation.unit === 'provider' && mutation.mutation.authored) {
      const value = mutation.mutation.authored;
      source.user.authored!.providers![mutation.mutation.id] = { icon: value.icon, protocol: value.protocol, base_url: value.base_url, credential: { type: 'literal' } };
    }
  });
  await openModels(subject);
  const form = screen.getByRole('form', { name: 'Provider transport' });
  await providerAction('Choose provider icon', within(form));
  let dialog = within(screen.getByRole('dialog', { name: 'Choose provider icon' }));
  fireEvent.click(dialog.getByRole('row', { name: 'Default icon' }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Choose provider icon' })).toBeNull());
  expect(writes(subject)).toHaveLength(0);
  await providerAction('Choose provider icon', within(form));
  dialog = within(screen.getByRole('dialog', { name: 'Choose provider icon' }));
  fireEvent.change(dialog.getByRole('searchbox'), { target: { value: 'deepseek' } });
  expect(dialog.getAllByRole('row')).toHaveLength(1);
  fireEvent.click(dialog.getByRole('row', { name: 'DeepSeek' }));
  await waitFor(() => expect(writes(subject)).toHaveLength(1));
  const operation = writes(subject)[0][0];
  if (operation.method !== 'configuration/sourceWrite') throw new Error('write required');
  expect(operation.params.mutation).toMatchObject({ kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: { icon: 'deepseek', credential: { kind: 'retain' } } } });
  await waitFor(() => expect(screen.getByRole('row', { name: 'transport' }).querySelector('[data-provider-icon="deepseek"]')).not.toBeNull());
  expect(within(form).queryByRole('button', { name: 'Save Provider transport' })).toBeNull();
});

it('requires a new workspace credential for an inherited provider override and returns to read-only after restoring', async () => {
  let revision = 1;
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method !== 'configuration/sourceWrite' || operation.params.mutation.kind !== 'config') return;
    const mutation = operation.params.mutation.mutation;
    if (mutation.unit !== 'provider' && mutation.unit !== 'restore_provider') return;
    const workspace = source.workspace!;
    const authored = mutation.unit === 'provider' ? mutation.authored : undefined;
    workspace.authored!.providers = authored ? { [mutation.id]: {
      base_url: authored.base_url, protocol: authored.protocol, credential: { type: 'literal' },
    } } : {};
    workspace.revision = `workspace-${++revision}`;
    return { type: 'sources_written', revision: workspace.revision, configuration_revision: null };
  });
  subject.source.resolved = subject.source.user.authored;
  render(<SettingsSurface client={subject.client} host={cfg3Host(subject)} target={workspaceSettingsTarget('A', 'A')} />);
  await settingsReady();
  await openSettingsPage('Models');
  const form = within(screen.getByRole('form', { name: 'Provider transport' }));
  expect((form.getByLabelText('Environment variable') as HTMLInputElement).value).toBe('USER_KEY');

  await providerAction('Customize', form);
  const url = form.getByLabelText('Base URL');
  expect((url as HTMLInputElement).value).toBe('https://user.invalid');
  fireEvent.change(url, { target: { value: 'https://workspace.invalid' } });
  fireEvent.keyDown(url, { key: 'Enter' });
  expect(writes(subject)).toHaveLength(0);
  await screen.findByText('Enter a valid API Key or environment variable name.');

  const key = form.getByLabelText('API Key');
  expect((key as HTMLInputElement).value).toBe('');
  fireEvent.change(key, { target: { value: 'workspace-only' } });
  fireEvent.keyDown(key, { key: 'Enter' });
  await waitFor(() => expect(writes(subject)).toHaveLength(1));
  expect(writes(subject)[0][0]).toMatchObject({ params: {
    target: { kind: 'workspace', directory: '/workspace/A' },
    expected_revision: 'workspace-1',
    mutation: { mutation: {
      unit: 'provider', id: 'transport', authored: {
        base_url: 'https://workspace.invalid', protocol: 'openai_responses',
        credential: { kind: 'literal', value: 'workspace-only' },
      },
    } },
  } });
  await waitFor(() => expect(form.queryByText('Using global settings.')).toBeNull());
  await waitFor(() => expect(form.getByRole('button', { name: 'Provider actions' }).getAttribute('aria-disabled')).toBe('false'));
  await providerAction('Use global default', form);
  fireEvent.click(screen.getByRole('button', { name: 'Use global default Provider transport' }));
  await screen.findByText('Using global settings.');
  expect((form.getByLabelText('Base URL') as HTMLInputElement).value).toBe('https://user.invalid');
  expect((form.getByLabelText('Environment variable') as HTMLInputElement).value).toBe('USER_KEY');
  expect(writes(subject)).toHaveLength(2);
});

it('keeps auxiliary model settings editable after the primary model is removed', async () => {
  const source = cfg3Source();
  const authored = source.user.authored!;
  authored.models = { main: cfg3Effective().document.models!.main };
  authored.agent = { model: { model: 'removed', summary_model: { mode: 'explicit', model: 'main' } } };
  const { writes } = await renderEditor(<ModelsPage client={cfg3Client().client} target={userSettingsTarget}
    source={source} scope="user" revision={source.user.revision} models={['main']} profiles={{}} onFocus={() => {}} />,
  { source, context: source });
  fireEvent.click(screen.getByRole('button', { name: 'Default model for new Sessions' }));
  const form = within(screen.getByRole('form', { name: 'Default model' }));
  fireEvent.click(form.getByRole('button', { name: 'Auxiliary models' }));
  await chooseOption('Title model', 'main', form);
  fireEvent.click(form.getByRole('button', { name: 'Save Default model' }));
  await waitFor(() => expect(writes).toHaveBeenCalledWith({ kind: 'config', mutation: { unit: 'root_model', authored: {
      model: 'removed', summary_model: { mode: 'explicit', model: 'main' }, title_model: { mode: 'explicit', model: 'main' },
  } } }, source.user.revision));
});

it('tests the saved model and revision without opening its editor or writing configuration', async () => {
  const subject = cfg3Client();
  subject.source.user.authored!.models = { main: {
    provider: 'transport', id: 'wire', context_window: '128000', max_output_tokens: 8192,
    capabilities: { input_modalities: ['text'], output_modalities: ['text'], json_schema_output: false },
  } };
  await openModels(subject);
  fireEvent.click(screen.getByRole('button', { name: 'Test connection for wire' }));
  await screen.findByText('Connection successful');
  expect(screen.queryByRole('dialog', { name: 'Model main' })).toBeNull();
  expect(subject.request.mock.calls.find(([operation]) => operation.method === 'model/probe')?.[0]).toMatchObject({
    params: { target: { kind: 'user' }, model: 'main', source: { user_revision: subject.source.user.revision } },
  });
  expect(writes(subject)).toHaveLength(0);
});

it('keeps a new provider template as a draft across provider and settings page changes', async () => {
  const subject = cfg3Client();
  await openModels(subject);
  fireEvent.click(screen.getByRole('button', { name: '+ Add Provider' }));
  fireEvent.click(within(screen.getByRole('dialog', { name: 'Add Provider' })).getByRole('button', { name: 'DeepSeek' }));
  fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
  expect((screen.getByLabelText('Base URL') as HTMLInputElement).value).toBe('https://api.deepseek.com/anthropic');
  fireEvent.click(screen.getByRole('row', { name: 'transport' }));
  await openSettingsPage('Agent');
  await openSettingsPage('Models');
  fireEvent.click(await screen.findByRole('row', { name: 'DeepSeek' }));
  expect((screen.getByLabelText('Base URL') as HTMLInputElement).value).toBe('https://api.deepseek.com/anthropic');
  await providerAction('Delete', within(screen.getByRole('form', { name: 'Provider DeepSeek' })));
  expect(screen.queryByRole('row', { name: 'DeepSeek' })).toBeNull();
  expect(writes(subject)).toHaveLength(0);
});
