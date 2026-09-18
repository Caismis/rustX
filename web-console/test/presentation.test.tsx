import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { App } from '../src/app/App';
import { Questionnaire } from '../src/app/agent/Questionnaire';
import { conversation } from '../src/bindings/projection';
import { interaction, Server, snapshot } from './fixture';
let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); });

it('boots the source-derived shell with no Host backend and exposes only supported controls', () => {
  render(<App client={server.client} workspaceHost={server.workspaceHost} />);
  expect(document.querySelector('[data-harness-frame]')).toBeTruthy();
  expect(screen.getAllByText('rustX').length).toBeGreaterThan(0);
  expect(screen.getByRole('button', { name: 'Connect' })).toBeTruthy();
  expect(screen.queryByText(/Workspace manager|Provider settings|Install plugin|Retry turn|Queue prompt|Open file/)).toBeNull();
  expect(server.requests).toEqual([]);
});
it('switching and unmounting open Session views remains presentation-only', async () => {
  await server.attached('A', 'B');
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A', 'B'] }));
  const ui = render(<App client={server.client} workspaceHost={server.workspaceHost} />);
  const baseline = server.requests.length;
  fireEvent.click(screen.getByRole('tab', { name: 'Session B' }));
  expect(screen.getByText('/workspace/B · attached')).toBeTruthy();
  fireEvent.click(screen.getByRole('tab', { name: 'Session A' }));
  expect(screen.getAllByRole('tab', { name: /^Session / })).toHaveLength(2);
  ui.unmount();
  expect(server.requests.slice(baseline).every(row => row.request.method === 'settings/read')).toBe(true);
  expect(server.client.getSnapshot().views.A.attachment).toBe('attached');
  expect(server.client.getSnapshot().views.B.attachment).toBe('attached');
});
it('renders streaming then one committed response, and an authoritative Approval removes obsolete controls', async () => {
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A'] }));
  render(<App client={server.client} workspaceHost={server.workspaceHost} />);
  const live = { ...snapshot(), attempt: { attempt_id: 'attempt-A', phase: { type: 'running' as const }, turn: 1,
    in_flight: { message_id: 'assistant-1', blocks: [{ type: 'text' as const, block_index: 0, text: 'Streaming response' }] } } };
  await act(() => server.update('A', live));
  expect(screen.getByLabelText('Streaming · assistant-1')).toBeTruthy();
  await act(() => server.update('A', { ...snapshot(), messages: [{ role: 'assistant', id: 'assistant-1', content: [{ type: 'text', text: 'Committed response' }] }], transcript: { entries: [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: 'assistant-1', content: [{ type: 'text', text: 'Committed response' }] } } }] }, pending_interactions: [interaction('approval')] }));
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
  render(<App client={server.client} workspaceHost={server.workspaceHost} />);
  act(() => server.socket.close());
  expect(screen.getByRole('region', { name: 'Questionnaire' })).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Submit answers' }) as HTMLButtonElement).disabled).toBe(true);
  await act(() => server.connect());
  expect((screen.getByRole('button', { name: 'Submit answers' }) as HTMLButtonElement).disabled).toBe(false);
});
it('option labels are presentation; duplicate labels still submit the selected native index', () => {
  let submitted: unknown;
  render(<Questionnaire disabled={false} status="Pending" questions={[{ header: 'Pick', question: 'Which one?', answer: {
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
  render(<App client={server.client} workspaceHost={server.workspaceHost} />);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Accept review' })); await server.waitFor('interaction/respond', 1); await server.client.refresh('A'); });
  const request = server.requests.find(item => item.request.method === 'interaction/respond')!.request;
  expect(request.params).toMatchObject({ interaction: review.interaction, response: { type: 'review', response: { instance, subject_digest: 'native-subject-digest', decision: { type: 'accepted' } } } });
  expect(screen.queryByRole('button', { name: 'Accept review' })).toBeNull();
});

it('closing A immediately emits exactly its detach, preserving B, runtime work and pending facts', async () => {
  server.snapshots.set('A', { ...snapshot(), attempt: { attempt_id: 'active-A', phase: { type: 'running' }, turn: 1 }, pending_interactions: [interaction('approval')] });
  await server.attached('A', 'B'); server.held.add('session/detach');
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A', 'B'] }));
  render(<App client={server.client} workspaceHost={server.workspaceHost} />);
  const target = server.client.target('A'), beforeA = server.client.getSnapshot().views.A.snapshot, beforeB = server.client.getSnapshot().views.B;
  const baseline = server.requests.length;
  act(() => { fireEvent.click(screen.getByRole('button', { name: 'Close view A' })); });
  expect(screen.queryByRole('tab', { name: 'Session A' })).toBeNull();
  expect(server.client.getSnapshot().views.A.attachmentIntent).toBe('released');
  const detach = await server.waitFor('session/detach', 1);
  expect(server.requests.slice(baseline).filter(({ request }) => request.method !== 'settings/read').map(({ request }) => ({ method: request.method, params: request.params }))).toEqual([{ method: 'session/detach', params: { target } }]);
  expect(server.client.getSnapshot().views.A.attachment).toBe('attached'); // No optimistic server fact.
  await act(async () => { server.reply(detach); await server.client.release('A', false); });
  expect(server.claims().map(item => item.session_id)).toEqual(['B']);
  expect(server.client.getSnapshot().views.A.snapshot).toBe(beforeA);
  expect(server.client.getSnapshot().views.B).toBe(beforeB);
  expect(server.loaded.has('A')).toBe(true);
  expect(server.snapshots.get('A')?.attempt?.phase.type).toBe('running');
  await act(() => server.connect());
  expect(server.requests.slice(baseline).filter(({ request }) => request.method === 'session/attach').map(({ request }) => request.params)).toEqual([{ session_id: 'B' }]);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })); await server.client.attach('A'); });
  expect(screen.getByRole('button', { name: 'Allow once' })).toBeTruthy();
  expect(server.client.getSnapshot().views.A.attachmentIntent).toBe('wanted');
  expect(server.requests.slice(baseline).filter(({ request }) => request.method === 'session/attach' && request.params.session_id === 'A')).toHaveLength(1);
});
it('lost close-tab detach acknowledgement leaves the tab closed and intent released across reconnect', async () => {
  await server.attached('A', 'B'); server.held.add('session/detach');
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A', 'B'] }));
  render(<App client={server.client} workspaceHost={server.workspaceHost} />);
  act(() => { fireEvent.click(screen.getByRole('button', { name: 'Close view A' })); });
  const detach = await server.waitFor('session/detach', 1); server.commit(detach);
  await act(async () => { server.socket.close(); await server.connect(); });
  expect(screen.queryByRole('tab', { name: 'Session A' })).toBeNull();
  expect(screen.getByText('Outcome uncertain: session/detach')).toBeTruthy();
  expect(server.client.getSnapshot().views.A.attachmentIntent).toBe('released');
  expect(server.claims().map(item => item.session_id)).toEqual(['B']);
  expect(server.requests.filter(({ request }) => request.method === 'session/detach')).toHaveLength(1);
  expect(server.requests.filter(({ request }) => request.method === 'session/attach' && request.params.session_id === 'A')).toHaveLength(1);
  expect(JSON.parse(localStorage.getItem('rustx-console-view-v1')!).tabs).toEqual(['B']);
});

it('releasing an open tab also removes its existing reload hint without closing its presentation', async () => {
  await server.attached('A', 'B'); server.held.add('session/unload');
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A', 'B'] }));
  const ui = render(<App client={server.client} workspaceHost={server.workspaceHost} />);
  act(() => { fireEvent.click(screen.getByRole('button', { name: 'Unload runtime' })); });
  const unload = await server.waitFor('session/unload', 1); server.commit(unload);
  expect(screen.getByRole('tab', { name: 'Session A' })).toBeTruthy();
  expect(JSON.parse(localStorage.getItem('rustx-console-view-v1')!).tabs).toEqual(['B']);
  await act(async () => { server.socket.close(); }); ui.unmount();
  const fresh = new Server(); fresh.snapshots = server.snapshots;
  const page = render(<App client={fresh.client} workspaceHost={fresh.workspaceHost} />); await act(() => fresh.connect());
  expect(fresh.claims().map(item => item.session_id)).toEqual(['B']);
  expect(screen.queryByRole('tab', { name: 'Session A' })).toBeNull();
  page.unmount(); fresh.client.disconnect();
});
