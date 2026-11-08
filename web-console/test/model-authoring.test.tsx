import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { cfg3Client } from './cfg3-fixture';
import { chooseOption, openSettingsPage, settingsReady, SettingsSurface } from './settings-harness';
import { userSettingsTarget } from '../src/app/settings/projection';
import { RpcFailure } from '../src/client/errors';

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
  await second.findByText('This configuration name already exists.');
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
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider transport' }));
  await waitFor(() => expect(writes(subject)).toHaveLength(1));
  expect(writes(subject)[0][0]).toMatchObject({ params: { mutation: { mutation: { unit: 'provider', id: 'transport', authored: { protocol: 'anthropic_messages', credential: { kind: 'retain' } } } } } });
  await waitFor(() => {
    for (const id of ['one', 'two']) expect(within(screen.getByRole('row', { name: id })).queryByText('anthropic_messages')).toBeNull();
    expect(screen.getByRole('button', { name: /API format$/ }).textContent).toContain('Anthropic Messages');
  });
  fireEvent.click(screen.getByRole('button', { name: '+ Add Model' }));
  expect(within(screen.getByRole('dialog', { name: 'Add Model' })).queryByLabelText('API format')).toBeNull();
});

it('invalid model limits and duplicate model identities never reach native writes', async () => {
  const subject = cfg3Client();
  subject.source.user.authored!.models = { 'transport/duplicate': { provider: 'transport', id: 'duplicate',  context_window: '128000', max_output_tokens: 8192, capabilities: { input_modalities: ['text'], output_modalities: ['text'], json_schema_output: false } } };
  const modal = await addModel(subject);
  fireEvent.change(modal.getByLabelText('Model ID'), { target: { value: 'duplicate' } });
  fireEvent.submit(modal.getByRole('form'));
  await modal.findByText('This configuration name already exists.');
  expect(writes(subject)).toHaveLength(0);
  fireEvent.change(modal.getByLabelText('Model ID'), { target: { value: 'new' } });
  fireEvent.change(modal.getByLabelText('Context window'), { target: { value: '1000' } });
  fireEvent.submit(modal.getByRole('form'));
  await modal.findByText('Maximum output tokens must be smaller than the context window.');
  expect(writes(subject)).toHaveLength(0);
  fireEvent.change(modal.getByLabelText('Context window'), { target: { value: '128000' } });
  await waitFor(() => expect(modal.queryByText('Maximum output tokens must be smaller than the context window.')).toBeNull());
});

it('a refused save keeps the model dialog and all input; Cancel discards the creation draft', async () => {
  const subject = cfg3Client(async operation => {
    if (operation.method === 'configuration/sourceWrite') throw new RpcFailure({ code: -32602, message: 'Native rejected this model', data: { kind: 'invalid_params' } });
  });
  const modal = await addModel(subject);
  fireEvent.change(modal.getByLabelText('Model ID'), { target: { value: 'retained' } });
  fireEvent.click(modal.getByRole('button', { name: 'Save Add Model' }));
  await modal.findByText(/Native rejected this model/);
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
    await modal.findByText('Enter a positive whole number within the supported range.');
    expect(writes(subject)).toHaveLength(0);
  }
  fireEvent.change(modal.getByLabelText('Context window'), { target: { value: '128000' } });
  for (const value of ['1.5', '4294967296']) {
    fireEvent.change(modal.getByLabelText('Maximum output tokens'), { target: { value } });
    fireEvent.submit(modal.getByRole('form'));
    await modal.findByText('Enter a positive whole number within the supported range.');
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
  await screen.findByText('This field is required.');
  expect(writes(subject)).toHaveLength(0);
});

it('switching credential modes removes the previous mode and its secret from the submitted provider', async () => {
  const subject = cfg3Client();
  await openModels(subject);
  await chooseOption('API Key source', 'Enter a literal secret');
  fireEvent.change(screen.getByLabelText('API Key'), { target: { value: 'DISCARDED_FIXTURE_SECRET' } });
  await chooseOption('API Key source', 'Read it from an environment variable');
  fireEvent.change(screen.getByLabelText('Environment variable'), { target: { value: 'PROVIDER_KEY' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider transport' }));
  await waitFor(() => expect(writes(subject)).toHaveLength(1));
  expect(writes(subject)[0][0]).toMatchObject({ params: { mutation: { mutation: { authored: { credential: { kind: 'environment', variable: 'PROVIDER_KEY' } } } } } });
  expect(JSON.stringify(writes(subject))).not.toContain('DISCARDED_FIXTURE_SECRET');
  expect(JSON.stringify(writes(subject))).not.toContain('"value"');
});
