import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { App } from '../src/app/App';
import { Server, endpoint } from './fixture';

let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); });
const refreshTitles = () => act(async () => { await server.client.listSessions(); });
const open = (title: string) => act(async () => { fireEvent.click(screen.getByRole('button', { name: `Open ${title}` })); });

it('follows native naming and committed conversation focus without exposing previews or identities', async () => {
  server.summaries.set('A', { name: null, preview: 'Opening question' });
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
  document.title = 'stale title';
  const app = await act(async () => render(<App client={server.client} workspaceHost={server.workspaceHost}/>));
  expect(document.title).toBe('rustX');
  server.summaries.set('A', { name: null, generated_title: { text: 'Generated A', provenance: { type: 'local', conversation_id: 'conversation-A', message_id: 'opening', call_id: 'title-call' } } });
  await refreshTitles();
  expect(document.title).toBe('Generated A — rustX');
  await act(async () => { await server.client.renameSession('A', 'Manual A'); });
  expect(document.title).toBe('Manual A — rustX');
  // Background catalog updates never retitle the foreground conversation.
  server.summaries.set('B', { name: 'Background B' });
  await refreshTitles();
  expect(document.title).toBe('Manual A — rustX');
  await open('Background B');
  expect(document.title).toBe('Background B — rustX');
  await act(async () => { fireEvent.click(screen.getAllByRole('button', { name: 'New Conversation' })[0]); });
  expect(document.title).toBe('rustX');
  await open('Manual A');
  expect(document.title).toBe('Manual A — rustX');
  app.unmount();
  expect(document.title).toBe('rustX');
});

it('retains the conversation title under Settings and uses the product title in Workflows', async () => {
  server.handlers.set('workflow/list', request => {
    if (request.method !== 'workflow/list') throw new Error('Expected Workflow library read');
    return { type: 'workflow_library', library: { target: request.params.target, directory: '/workspace/.agents/workflows', absent_revision: 'missing', files: [], diagnostic: null } };
  });
  await server.attached('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A'] }));
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost}/>); });
  expect(document.title).toBe('Session A — rustX');
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Settings' })); });
  expect(document.title).toBe('Session A — rustX');
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Close Settings' })); });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Workflows' })); });
  expect(document.title).toBe('rustX');
  await act(async () => { await server.client.renameSession('A', 'Revised while away'); });
  expect(document.title).toBe('rustX');
  await open('Revised while away');
  expect(document.title).toBe('Revised while away — rustX');
});
