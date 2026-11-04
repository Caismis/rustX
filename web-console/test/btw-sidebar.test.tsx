import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { BtwPanel } from '../src/app/agent/BtwPanel';
import { App } from '../src/app/App';
import { Server, endpoint } from './fixture';

let server: Server;
afterEach(() => { cleanup(); server?.client.dispose(); localStorage.clear(); vi.useRealTimers(); });
async function connected() {
  server = new Server(); server.capabilities = { ...server.capabilities, delivery_access: true };
  await server.attached('A', 'B');
  server.handlers.set('auxiliary/btw', request => ({ type: 'auxiliary_submitted', receipt: { call_id: 'call-A', correlation: request.method === 'auxiliary/btw' ? request.params.submission.correlation : '', scope: 'scope-A' } }));
  server.handlers.set('auxiliary/status', () => ({ type: 'auxiliary_status', status: { call_id: 'call-A', executing: true, inference_stopped: false, settlement_confirmed: false, outcome: null, result: 'pending' } }));
  server.handlers.set('delivery/cancel', () => ({ type: 'delivery_cancel', accepted: true }));
}
const input = () => screen.getByRole('textbox', { name: 'Temporary question' });

it('keeps drafts with their session across tab removal and renders inline with keyboard focus', async () => {
  await connected(); const close = vi.fn();
  const panel = (sessionId: string) => <BtwPanel key={sessionId} client={server.client} sessionId={sessionId} visible close={close}/>;
  const ui = render(panel('A'));
  expect(screen.queryByRole('dialog')).toBeNull(); expect(document.activeElement).toBe(input());
  fireEvent.change(input(), { target: { value: 'Draft for A' } });
  ui.rerender(panel('B')); expect(input()).toHaveProperty('value', '');
  fireEvent.change(input(), { target: { value: 'Draft for B' } });
  ui.unmount(); render(panel('A')); expect(input()).toHaveProperty('value', 'Draft for A');
  fireEvent.keyDown(input(), { key: 'Escape', isComposing: true }); expect(close).not.toHaveBeenCalled();
  fireEvent.keyDown(input(), { key: 'Escape' }); expect(close).toHaveBeenCalledOnce();
  await act(async () => { await server.client.disconnect(); });
  expect(input()).toHaveProperty('disabled', true);
  expect(input()).toHaveProperty('value', 'Draft for A');
  expect(server.requests.some(row => row.request.method === 'auxiliary/btw' || row.request.method === 'auxiliary/cancel')).toBe(false);
});

it('hides a running question without replay or cancel, then recovers its result and explicitly cancels the next question', async () => {
  await connected(); const close = vi.fn();
  const panel = (visible: boolean) => <BtwPanel client={server.client} sessionId="A" visible={visible} close={close}/>;
  const ui = render(panel(true));
  fireEvent.change(input(), { target: { value: 'What is committed?' } });
  await act(async () => fireEvent.keyDown(input(), { key: 'Enter', ctrlKey: true }));
  expect(input()).toHaveProperty('disabled', true);
  expect(input()).toHaveProperty('value', '');
  expect(screen.getByRole('article', { name: 'Your message' }).textContent).toBe('What is committed?');
  expect(screen.queryByRole('button', { name: 'Ask' })).toBeNull(); expect(screen.getByRole('button', { name: 'Cancel question' })).toBeTruthy();
  ui.rerender(panel(false)); ui.unmount();
  expect(server.requests.filter(row => row.request.method === 'auxiliary/btw')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'auxiliary/cancel')).toHaveLength(0);
  server.handlers.set('auxiliary/status', () => ({ type: 'auxiliary_status', status: { call_id: 'call-A', executing: false, inference_stopped: true, settlement_confirmed: true, outcome: 'completed', result: 'retained' } }));
  server.handlers.set('auxiliary/result', () => ({ type: 'auxiliary_result', result: { call_id: 'call-A', final_request_id: 'answer-A', execution: 'completed', value: { kind: 'btw', text: '**Retained answer** with `code`.' } } }));
  render(panel(true)); await screen.findByText('Retained answer');
  expect(screen.getByText('Retained answer').tagName).toBe('STRONG'); expect(screen.getByText('code').tagName).toBe('CODE');
  expect(screen.getByRole('article', { name: 'Assistant response' })).toBeTruthy();
  expect(input()).toHaveProperty('value', '');
  fireEvent.change(input(), { target: { value: 'Cancel this one' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Ask' })));
  server.handlers.set('auxiliary/cancel', () => ({ type: 'auxiliary_cancelled', accepted: true }));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Cancel question' })));
  expect(server.requests.filter(row => row.request.method === 'auxiliary/btw')).toHaveLength(2);
  expect(server.requests.filter(row => row.request.method === 'auxiliary/cancel')).toHaveLength(1);
});

it('opens from collapsed sidebar chrome, coexists with files and restores the retained draft and focus', async () => {
  await connected(); localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint, openViews: ['A', 'B'] }));
  const host = { ...server.workspaceHost, workbench: async () => ({ entries: [], terminals: [], shells: [] }) };
  render(<App client={server.client} workspaceHost={host}/>); await act(async () => {});
  const opener = () => document.querySelector<HTMLButtonElement>('[data-side-question-toggle]')!;
  expect(opener().getAttribute('aria-label')).toBe('Ask a side question');
  await act(async () => { opener().focus(); fireEvent.click(opener()); });
  const panel = screen.getByRole('form', { name: 'Ask a side question' });
  expect(screen.queryByRole('dialog', { name: 'Ask a side question' })).toBeNull();
  expect(document.activeElement).toBe(input());
  fireEvent.change(within(panel).getByRole('textbox'), { target: { value: 'Keep in the sidebar' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'New tab' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: /^Workspace files/ })));
  expect(screen.getByRole('tab', { name: 'Files' })).toBeTruthy();
  await act(async () => fireEvent.click(screen.getByRole('tab', { name: 'Ask a side question' })));
  expect(input()).toHaveProperty('value', 'Keep in the sidebar');
  await act(async () => fireEvent.keyDown(input(), { key: 'Escape' }));
  expect(document.activeElement).toBe(opener());
  await act(async () => fireEvent.click(opener()));
  expect(screen.getByRole('textbox', { name: 'Temporary question' })).toHaveProperty('value', 'Keep in the sidebar');
  expect(document.activeElement).toBe(input());
  expect(server.requests.some(row => row.request.method === 'auxiliary/btw' || row.request.method === 'auxiliary/cancel')).toBe(false);
});
