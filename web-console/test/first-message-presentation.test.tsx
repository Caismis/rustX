import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { App } from '../src/app/App';
import { firstSubmissions } from '../src/app/new-conversation/first-submit';
import { Server, snapshot } from './fixture';
afterEach(() => { cleanup(); localStorage.clear(); });
it('keeps later idle input visible after acceptance even when attempt updates precede the acknowledgement', async () => {
  const server = new Server(); await server.attached('A');
  try {
    localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: server.client.getSnapshot().endpoint, openViews: ['A'] }));
    const ui = await act(async () => render(<App client={server.client} workspaceHost={server.workspaceHost}/>));
    const input = screen.getByRole('textbox', { name: 'Message' });
    fireEvent.change(input, { target: { value: 'Later message stays visible' } });
    server.held.add('inbound/queue'); fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    const request = await server.waitFor('inbound/queue', 1);
    const running = { ...snapshot('A'), attempt: { attempt_id: 'later-attempt', phase: { type: 'running' as const }, turn: 2 } };
    await act(async () => server.update('A', running));
    await act(async () => server.reply(request));
    await waitFor(() => expect(input).toHaveProperty('value', ''));
    expect(screen.getAllByText('Later message stays visible')).toHaveLength(1);
    const echo = ui.container.querySelector('[data-provisional-message-id="accepted-user"]');
    const anchor = echo?.querySelector('[data-chat-anchor-key="message:accepted-user"]');
    expect(echo).not.toBeNull(); expect(ui.container.querySelector('[data-submission-echo]')).toBeNull();
    await act(async () => server.update('A', { ...running, attempt: null }));
    expect(ui.container.querySelector('[data-provisional-message-id="accepted-user"]')).toBe(echo);
    expect(screen.getAllByText('Later message stays visible')).toHaveLength(1);
    const message = { role: 'user' as const, source: 'human' as const, id: 'accepted-user', content: [{ type: 'text' as const, text: 'Later message stays visible' }] };
    await act(async () => server.update('A', { ...snapshot('A'), messages: [message], transcript: { entries: [{ cursor: '1', item: { type: 'message', message } }] } }));
    expect(ui.container.querySelector('[data-provisional-message-id="accepted-user"]')).toBeNull();
    expect(ui.container.querySelector('[data-chat-anchor-key="message:accepted-user"]')).toBe(anchor);
    expect(screen.getAllByText('Later message stays visible')).toHaveLength(1);
    expect(screen.queryByText('Ready for a task.')).toBeNull();
  } finally { server.client.dispose(); }
});
it('keeps the first input visible between its ACK and exact canonical observation', async () => {
  const server = new Server(); await server.attached('A');
  server.handlers.set('workflow/list', request => {
    if (request.method !== 'workflow/list') throw Error('library');
    return { type: 'workflow_library', library: { target: request.params.target, directory: '/workflows', absent_revision: 'empty', files: [], diagnostic: null } };
  });
  try {
    localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: server.client.getSnapshot().endpoint, openViews: ['A'] }));
    const ui = await act(async () => render(<App client={server.client} workspaceHost={server.workspaceHost}/>));
    fireEvent.click(await screen.findByRole('button', { name: 'Open Session B' }));
    const input = screen.getByRole('textbox', { name: 'Message' }); fireEvent.change(input, { target: { value: 'First message stays visible' } });
    server.held.add('inbound/queue'); fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    const command = await server.waitFor('inbound/queue', 1);
    expect(screen.getAllByText('First message stays visible')).toHaveLength(1);
    await act(async () => server.reply(command));
    await waitFor(() => expect(firstSubmissions(server.client).session('B')?.phase).toBe('admitted'));
    expect(server.client.getSnapshot().views.B.submissions?.map(value => value.messageId)).toContain('accepted-user');
    expect(screen.getAllByText('First message stays visible')).toHaveLength(1);
    const unrelated = { role: 'user' as const, source: 'human' as const, id: 'another-message', content: [{ type: 'text' as const, text: 'An unrelated observed message' }] };
    server.snapshots.set('B', { ...snapshot('B'), messages: [unrelated], transcript: { entries: [{ cursor: '1', item: { type: 'message', message: unrelated } }] } });
    await act(async () => server.client.refresh('B'));
    expect(server.client.getSnapshot().views.B.submissions?.map(value => value.messageId)).toContain('accepted-user');
    expect(ui.container.querySelector('[data-pending-message]')).not.toBeNull();
    expect(screen.getAllByText('First message stays visible')).toHaveLength(1);
    const message = { role: 'user' as const, source: 'human' as const, id: 'accepted-user', content: [{ type: 'text' as const, text: 'First message stays visible' }] };
    server.snapshots.set('B', { ...snapshot('B'), messages: [message], transcript: { entries: [{ cursor: '1', item: { type: 'message', message } }] } });
    await act(async () => server.client.refresh('B'));
    await waitFor(() => expect(server.client.getSnapshot().views.B.submissions).toHaveLength(0));
    expect(screen.getAllByText('First message stays visible')).toHaveLength(1);
    expect(ui.container.querySelector('[data-pending-message]')).toBeNull();
  } finally { server.client.dispose(); }
});
