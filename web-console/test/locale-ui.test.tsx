import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import { localeController } from '../src/locale/controller';
import { translator, type TranslationKey } from '../src/locale/translation';
import { Interactions } from '../src/app/agent/Interactions';
import { ToolCard } from '../src/presentation/agent/ToolCard';
import { Server, interaction } from './fixture';
const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
let server: Server | undefined;
afterEach(() => { cleanup(); server?.client.disconnect(); localStorage.clear(); if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard); else Reflect.deleteProperty(navigator, 'clipboard'); });
const text = (key: TranslationKey) => translator(localeController.getSnapshot().active)(key);

it('switches the complete active shell and General through Language, preserving the composer and making zero server requests', async () => {
  server = new Server(); await server.connect();
  await act(async () => { render(<App client={server!.client} workspaceHost={server!.workspaceHost}/>); });
  expect(screen.getByRole('tree', { name: 'Session browser' })).toBeTruthy();
  const composer = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'User authored ENGLISH 中文 /path' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Settings' })));
  const before = [...server.requests];
  fireEvent.click(screen.getByRole('button', { name: /Language$/ }));
  await act(async () => fireEvent.click(screen.getByRole('option', { name: '中文' })));
  expect(document.documentElement.lang).toBe('zh-CN');
  expect(localStorage.getItem('rustx-locale-v1')).toBe('zh');
  expect(server.requests).toEqual(before);
  expect(screen.getByRole('heading', { name: '通用设置' })).toBeTruthy();
  expect(screen.getByRole('button', { name: /中文.*语言/ })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: /语言$/ }));
  expect(screen.getByRole('option', { name: 'English' })).toBeTruthy();
  fireEvent.keyDown(screen.getByRole('listbox'), { key: 'Escape' });
  fireEvent.click(screen.getByRole('button', { name: text('settings:settings-root.close-settings') }));
  expect(screen.getByRole('tree', { name: text('workspace:workspace-browser.session-browser') })).toBeTruthy();
  expect(screen.getByRole('region', { name: text('common:conversation-composer.new-conversation') })).toBeTruthy();
  expect(screen.getByRole('textbox', { name: text('agent:agent-composer.message') })).toBe(composer);
  expect((composer as HTMLTextAreaElement).value).toBe('User authored ENGLISH 中文 /path');
  expect(server.requests).toEqual(before);
  act(() => localeController.setLocale('en'));
  expect(screen.getByRole('tree', { name: 'Session browser' })).toBeTruthy();
  expect(server.requests).toEqual(before);
});

it('a native approval has identical request count, ordering, methods and payloads in both locales; switches themselves do nothing', async () => {
  const sequences = [];
  for (const locale of ['en', 'zh'] as const) {
    server = new Server(); server.snapshots.get('A')!.pending_interactions = [interaction('approval')];
    await server.attached('A');
    const activeServer = server;
    function Pending() {
      const state = useSyncExternalStore(activeServer.client.subscribe, activeServer.client.getSnapshot);
      return <Interactions client={activeServer.client} state={state} view={state.views.A}/>;
    }
    render(<Pending/>);
    const before = server.requests.length;
    act(() => localeController.setLocale(locale));
    expect(server.requests).toHaveLength(before);
    expect(screen.getByText(/Developer approval required/)).toBeTruthy();
    server.held.add('interaction/respond');
    const allow = text('interactions:approval-takeover.allow-once');
    await act(async () => fireEvent.click(screen.getByRole('button', { name: allow })));
    await server.waitFor('interaction/respond', 1);
    sequences.push(server.requests.slice(before).map(row => row.request));
    cleanup(); server.client.disconnect();
  }
  expect(sequences[0]).toHaveLength(1);
  expect(sequences[0][0].method).toBe('interaction/respond');
  expect(sequences[1]).toEqual(sequences[0]);
});

it('Tool chrome and global connection notices switch while opaque tool output stays byte-for-byte intact', async () => {
  server = new Server();
  await act(async () => { render(<App client={server!.client} workspaceHost={server!.workspaceHost}/>); });
  expect(screen.getByText('Unable to connect to rustX', { exact: true })).toBeTruthy();
  act(() => localeController.setLocale('zh'));
  expect(screen.getByText(text('common:app.unable-to-connect-to-rustx'), { exact: true })).toBeTruthy();
  expect(server.requests).toHaveLength(0);
  cleanup();
  render(<ToolCard tool={{ id: 'native-id', title: 'native.tool', summary: '/raw/path', variant: 'generic', state: 'failure', output: 'Native failure: do NOT translate {name}' }}/>);
  fireEvent.click(screen.getByText('native.tool'));
  expect(screen.getByText('Native failure: do NOT translate {name}')).toBeTruthy();
  expect(screen.getByLabelText(text('tools:tool-card.tool-status')).textContent).toBe('失败');
  act(() => localeController.setLocale('en'));
  expect(screen.getByText('Native failure: do NOT translate {name}')).toBeTruthy();
  expect(screen.getByLabelText('Tool status').textContent).toBe('failure');
});

it('retained questionnaire validation switches language without changing the draft or submitting', async () => {
  const { Questionnaire } = await import('../src/app/agent/Questionnaire');
  let submissions = 0;
  render(<Questionnaire questions={[{ header: 'Native number', question: 'User-authored question', answer: { type: 'integer' } }]}
    disabled={false} status="native-status" onSubmit={() => { submissions++; }} onDecline={() => {}}/>);
  const input = screen.getByRole('textbox');
  fireEvent.change(input, { target: { value: 'not-a-number' } });
  fireEvent.click(screen.getByRole('button', { name: 'Submit answers' }));
  const key = 'interactions:copy.value-enter-a-canonical-whole-number';
  expect(screen.getByRole('status').textContent).toContain(translator('en')(key, { p0: 'Native number' }));
  act(() => localeController.setLocale('zh'));
  expect(screen.getByRole('status').textContent).toContain(translator('zh')(key, { p0: 'Native number' }));
  expect((input as HTMLTextAreaElement).value).toBe('not-a-number');
  expect(screen.getByText('User-authored question')).toBeTruthy();
  expect(submissions).toBe(0);
});

it('the Questionnaire textarea mirror is the exact draft plus its layout newline in every locale', async () => {
  const { Questionnaire } = await import('../src/app/agent/Questionnaire');
  act(() => localeController.setLocale('en'));
  render(<Questionnaire questions={[{ header: 'Free text', question: 'User-authored question', answer: { type: 'text' } }]}
    disabled={false} status="native-status" onSubmit={() => {}} onDecline={() => {}}/>);
  const input = screen.getByRole('textbox') as HTMLTextAreaElement;
  const mirror = input.previousElementSibling!;
  expect(mirror.getAttribute('aria-hidden')).toBe('true');
  for (const draft of ['', 'single line', 'first\nsecond', 'ends with newline\n', '\n\n', ' {p0} 中文 ']) {
    fireEvent.change(input, { target: { value: draft } });
    for (const locale of ['zh', 'en'] as const) {
      act(() => localeController.setLocale(locale));
      expect(mirror.textContent).toBe(`${draft}\n`);
      expect(input.value).toBe(draft);
    }
  }
});

it('Session renaming never promotes a localized fallback title into native data and keeps the same request sequence', async () => {
  const sequences = [];
  for (const locale of ['en', 'zh'] as const) {
    server = new Server(); server.summaries.set('A', { name: null, preview: null });
    const activeServer = server;
    server.handlers.set('session/name', request => {
      if (request.method !== 'session/name') throw new Error('Wrong fixture method');
      activeServer.summaries.set('A', { name: request.params.name });
      return { type: 'session', session: { id: 'A', name: request.params.name, active_node: 'node-A', active_conversation_id: 'conversation-A', node_count: 1, created_at: '0', updated_at: '0' } };
    });
    await server.connect();
    act(() => localeController.setLocale(locale));
    await act(async () => { render(<App client={activeServer.client} workspaceHost={activeServer.workspaceHost}/>); });
    fireEvent.click(document.querySelector<HTMLButtonElement>('button[data-session-actions="A"]')!);
    fireEvent.click(screen.getByRole('menuitem', { name: text('workspace:rename') }));
    const input = screen.getByRole('textbox', { name: text('workspace:workspace-navigation.name') }) as HTMLInputElement;
    expect(input.value).toBe('');
    expect((screen.getByRole('button', { name: text('workspace:workspace-navigation.save-name') }) as HTMLButtonElement).disabled).toBe(true);
    const before = server.requests.length;
    fireEvent.change(input, { target: { value: 'User-authored 名称' } });
    await act(async () => fireEvent.click(screen.getByRole('button', { name: text('workspace:workspace-navigation.save-name') })));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    sequences.push(server.requests.slice(before).map(row => row.request));
    cleanup(); server.client.disconnect();
  }
  expect(sequences[0][0]).toMatchObject({ method: 'session/name', params: { session_id: 'A', name: 'User-authored 名称' } });
  expect(sequences[0].length).toBeGreaterThan(1);
  expect(sequences[1]).toEqual(sequences[0]);
});

it('Inspector filters retain semantic wire kinds across a live locale switch', async () => {
  const { LiveInspector } = await import('../src/app/Inspector');
  server = new Server(); await server.connect();
  render(<LiveInspector client={server.client}/>);
  const select = screen.getByRole('combobox', { name: 'Message kind filter' }) as HTMLSelectElement;
  fireEvent.change(select, { target: { value: 'request' } });
  const before = [...document.querySelectorAll('.protocol-log pre')].map(node => node.textContent);
  expect(before.length).toBeGreaterThan(0);
  const requests = [...server.requests];
  act(() => localeController.setLocale('zh'));
  expect(select.value).toBe('request');
  expect([...select.options].map(option => option.value)).toEqual(['', 'request', 'response', 'notification', 'invalid']);
  expect([...document.querySelectorAll('.protocol-log pre')].map(node => node.textContent)).toEqual(before);
  expect(server.requests).toEqual(requests);
});


it.each([true, false])('retained Inspector clipboard completion (%s) live-switches without repeating any operation', async success => {
  const { LiveInspector } = await import('../src/app/Inspector');
  server = new Server(); await server.connect();
  const writeText = vi.fn(() => success ? Promise.resolve() : Promise.reject(new Error('Clipboard denied')));
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  render(<LiveInspector client={server.client}/>);
  const before = [...server.requests];
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Copy JSON' })));
  const key = success ? 'inspector:copy.copied-json' : 'inspector:copy.copy-failed-clipboard-unavailable';
  const notice = screen.getByRole('status');
  expect(notice.textContent).toContain(translator('en')(key));
  act(() => localeController.setLocale('zh'));
  expect(screen.getByRole('status')).toBe(notice);
  expect(notice.textContent).toContain(translator('zh')(key));
  expect(notice.textContent).not.toContain(translator('en')(key));
  expect(writeText).toHaveBeenCalledTimes(1);
  expect(server.requests).toEqual(before);
});


it.each(['en', 'zh'] as const)('policy display choices preserve exact native values in %s', async locale => {
  const { PolicyFields } = await import('../src/app/settings/tools/ToolsPage');
  act(() => localeController.setLocale(locale));
  const change = vi.fn(), tx = translator(locale);
  render(<PolicyFields value={{}} change={change}/>);
  for (const [field, label, values] of [
    ['execution', 'settings:tools-page.execution', ['foreground_only', 'background_only', 'model_selectable']],
    ['concurrency', 'settings:tools-page.concurrency', ['sequential', 'parallel']],
    ['approval', 'settings:tools-page.approval-2', ['never', 'always']],
  ] as const) {
    for (const value of values) {
      fireEvent.click(screen.getByRole('button', { name: new RegExp(`${tx(label)}$`) }));
      fireEvent.click(screen.getByRole('option', { name: tx(`settings:policy.${value}`) }));
      expect(change).toHaveBeenLastCalledWith({ [field]: value });
    }
  }
  expect(change).toHaveBeenCalledTimes(7);
});
