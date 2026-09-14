import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { App } from '../src/app/App';
import { QuestionComposer } from '../src/presentation/QuestionComposer';
import { conversation } from '../src/bindings/projection';
import { interaction, Server, snapshot } from './fixture';
let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); });

it('boots the source-derived shell with no Host backend and exposes only supported controls', () => {
  render(<App client={server.client} />);
  expect(screen.getByRole('heading', { name: 'Sessions, in motion.' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Connect' })).toBeTruthy();
  expect(screen.queryByText(/Workspace manager|Provider settings|Install plugin|Retry turn|Queue prompt|Open file/)).toBeNull();
  expect(server.requests).toEqual([]);
});
it('switching, closing, and unmounting Session views emits no implicit runtime operations', async () => {
  await server.attached('A', 'B');
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A', 'B'] }));
  const ui = render(<App client={server.client} />);
  const baseline = server.requests.length;
  fireEvent.click(screen.getByRole('tab', { name: 'Session B' }));
  expect(screen.getByText('/workspace/B · attached')).toBeTruthy();
  fireEvent.click(screen.getByRole('tab', { name: 'Session A' }));
  fireEvent.click(screen.getByRole('button', { name: 'Close view A' }));
  ui.unmount();
  expect(server.requests).toHaveLength(baseline);
  expect(server.client.getSnapshot().views.A.attachment).toBe('attached');
  expect(server.client.getSnapshot().views.B.attachment).toBe('attached');
});
it('renders streaming then one committed response, and an authoritative Approval removes obsolete controls', async () => {
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A'] }));
  render(<App client={server.client} />);
  const live = { ...snapshot(), attempt: { attempt_id: 'attempt-A', phase: { type: 'running' as const }, turn: 1,
    in_flight: { message_id: 'assistant-1', blocks: [{ type: 'text' as const, block_index: 0, text: 'Streaming response' }] } } };
  await act(() => server.update('A', live));
  expect(screen.getByLabelText('Streaming · assistant-1')).toBeTruthy();
  await act(() => server.update('A', { ...snapshot(), messages: [{ role: 'assistant', id: 'assistant-1', content: [{ type: 'text', text: 'Committed response' }] }], pending_interactions: [interaction('approval')] }));
  expect(screen.queryByLabelText('Streaming · assistant-1')).toBeNull();
  expect(screen.getAllByText('Committed response')).toHaveLength(1);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Allow once' })); await server.waitFor('interaction/respond', 1); await server.client.refresh('A'); });
  expect(screen.queryByRole('button', { name: 'Allow once' })).toBeNull();
  expect(conversation(server.client.getSnapshot().views.A.snapshot!).messages).toHaveLength(1);
});
it('pending interaction remains visible but disabled across transport loss and recovers after connect', async () => {
  server.snapshots.get('A')!.pending_interactions = [interaction('questionnaire')];
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A'] }));
  render(<App client={server.client} />);
  act(() => server.socket.close());
  expect(screen.getByRole('region', { name: 'Questionnaire' })).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Submit answers' }) as HTMLButtonElement).disabled).toBe(true);
  await act(() => server.connect());
  expect((screen.getByRole('button', { name: 'Submit answers' }) as HTMLButtonElement).disabled).toBe(false);
});
it('option labels are presentation; duplicate labels still submit the selected native index', () => {
  let submitted: unknown;
  render(<QuestionComposer disabled={false} status="Pending" questions={[{ header: 'Pick', question: 'Which one?', answer: {
    type: 'single_choice', allow_custom: false, options: [{ label: 'Same', description: 'First' }, { label: 'Same', description: 'Second' }],
  } }]} onDecline={() => {}} onSubmit={value => { submitted = value; }} />);
  fireEvent.click(screen.getAllByRole('radio')[1]); fireEvent.click(screen.getByRole('button', { name: 'Submit answers' }));
  expect(submitted).toEqual({ answers: [{ question_index: 0, answer: { type: 'option', value: { option_index: 1 } } }] });
  expect(screen.queryByLabelText('Custom answer')).toBeNull();
});

it('native Review preserves instance and subject digest and waits for authoritative removal', async () => {
  const review = interaction('approval');
  const instance = { block: { run: { conversation_id: 'conversation-A', attempt_id: 'attempt-A', invocation: '1' }, definition: { workflow_id: 'workflow', blocks: [] }, invocations: [0] }, node: 'review', visit: 1 };
  review.request.kind = { type: 'review', subject_digest: 'native-subject-digest', review: { instance, subject: { type: 'plan', content: 'Review this plan' }, context: [] } };
  server.snapshots.get('A')!.pending_interactions = [review]; await server.attached('A');
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A'] }));
  render(<App client={server.client} />);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Accept review' })); await server.waitFor('interaction/respond', 1); await server.client.refresh('A'); });
  const request = server.requests.find(item => item.request.method === 'interaction/respond')!.request;
  expect(request.params).toMatchObject({ interaction: review.interaction, response: { type: 'review', response: { instance, subject_digest: 'native-subject-digest', decision: { type: 'accepted' } } } });
  expect(screen.queryByRole('button', { name: 'Accept review' })).toBeNull();
});
