import { translator } from '../src/locale/translation';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import type { AgentStatusView, MessageBlock, RuntimeClientSnapshot, RuntimeClientStatusSection, RuntimeClientTranscriptEntry, UserMessageBlock } from '../../protocol/app-server/v23';
import { App } from '../src/app/App';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { agentStatusFacets, agentStatusAnchor, agentStatusPlacement, isAgentStatusContext, statusesAt } from '../src/bindings/agent-status';
import { todoDock } from '../src/bindings/composer-context';
import { replaceTranscript } from '../src/client/transcript';
import { Server, snapshot } from './fixture';

let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); localStorage.clear(); });

/** One composed Agent Status. `sections` is the runtime's closed typed vocabulary;
 * `rendered` is the exact model-facing text and is never a presentation source. */
const status = (id: string, opportunities: AgentStatusView['opportunities'], sections: RuntimeClientStatusSection[] = [{ type: 'temporal', current_time: '2026-09-14T10:42:00Z', timezone: 'UTC' }]): AgentStatusView =>
  ({ attempt_id: 'attempt-A', turn: 1, status_message_id: id, opportunities, sections, rendered: `<status id="${id}">rendered prose ${id}</status>` });
const fresh = (target: string): AgentStatusView['opportunities'] => ({ fresh_inbound: { target_message_id: target } });
const batch = (cursor: string): AgentStatusView['opportunities'] => ({ post_tool_batch: { transcript_anchor: cursor } });
const todoSection = (subject: string): RuntimeClientStatusSection =>
  ({ type: 'todo', current: { id: '1', subject, status: 'in_progress', blocked: false, active_form: `Doing ${subject}` }, tasks: [], active_count: 1, blocked_count: 0, completed_count: 0, deleted_count: 0, omitted_count: 0 });

const user = (id: string, cursor: string, text = `Ask ${id}`): RuntimeClientTranscriptEntry =>
  ({ cursor, item: { type: 'message', message: { role: 'user', id, source: 'human', content: [{ type: 'text', text }] } } });
const assistant = (id: string, cursor: string, text = `Answer ${id}`): RuntimeClientTranscriptEntry =>
  ({ cursor, item: { type: 'message', message: { role: 'assistant', id, content: [{ type: 'text', text }, { type: 'tool_call', id: `call-${id}`, tool_id: 'bash', name: 'bash', arguments: {} }] } },
    tool_calls: [{ message_id: id, block_index: 1, call_id: `call-${id}`, tool_id: 'bash', name: 'bash', state: { type: 'settled', arguments: '{}', result: { status: { type: 'success' }, duration_ms: 1, content: [{ type: 'text', text: 'native tool output' }] } } }] });
/** A canonical ToolResult entry. Its standalone body is suppressed by ordinary Web
 * rendering — the native call projection already draws the result beside the call —
 * yet it keeps its authoritative transcript position. */
const toolResult = (id: string, cursor: string, assistantId: string): RuntimeClientTranscriptEntry =>
  ({ cursor, item: { type: 'message', message: { role: 'tool', id, occurrence: { assistant_message_id: assistantId, block_index: 1 }, tool_call_id: `call-${assistantId}`, tool_id: 'bash', result: { status: { type: 'success' }, duration_ms: 1, content: [{ type: 'text', text: 'native tool output' }] } } } });

/** The conversation used by every placement case: one inbound turn, one Assistant
 * response with a native call, and the settled ToolResult batch that followed it. */
const CONVERSATION = () => [user('u1', '1'), assistant('a1', '2'), toolResult('t1', '3', 'a1')];
const withTranscript = (entries: RuntimeClientTranscriptEntry[], statuses: AgentStatusView[], base = snapshot()): RuntimeClientSnapshot =>
  ({ ...base, transcript: { entries }, statuses });

/** Every rendered annotation, in DOM order, as [composition identity, anchor row]. */
const annotations = (container: HTMLElement) => [...container.querySelectorAll('[data-agent-status]')].map(node =>
  [node.getAttribute('data-agent-status'), node.closest('[data-chat-anchor-key]')?.getAttribute('data-chat-anchor-key')]);
const anchorKeys = (container: HTMLElement) => [...container.querySelectorAll('[data-chat-anchor-key]')].map(node => node.getAttribute('data-chat-anchor-key'));

describe('single-anchor Agent Status placement', () => {
  it('R01: a FreshInbound composition renders once under its exact inbound message', () => {
    const ui = render(<AgentTranscript snapshot={withTranscript(CONVERSATION(), [status('s1', fresh('u1'))])} />);
    expect(annotations(ui.container)).toEqual([['s1', 'message:u1']]);
    // Not on the Assistant response, the Tool entry, or anywhere else.
    expect(ui.container.querySelectorAll('[data-agent-status="s1"]')).toHaveLength(1);
    expect(within(ui.container.querySelector('[data-chat-anchor-key="message:a1"]') as HTMLElement).queryByRole('note')).toBeNull();
  });

  it('R02: a PostToolBatch composition renders once after its exact transcript cursor, including a suppressed Tool body', () => {
    const ui = render(<AgentTranscript snapshot={withTranscript(CONVERSATION(), [status('s2', batch('3'))])} />);
    expect(annotations(ui.container)).toEqual([['s2', 'message:t1']]);
    // The annotation-only slot follows the Assistant row and draws no second Tool body.
    expect(anchorKeys(ui.container)).toEqual(['message:u1', 'message:a1', 'message:t1']);
    const slot = ui.container.querySelector('[data-chat-anchor-key="message:t1"]') as HTMLElement;
    expect(within(slot).getByRole('note', { name: 'Agent Status' })).toBeTruthy();
    expect(slot.textContent).not.toContain('native tool output');
    expect(ui.container.querySelectorAll('[data-tool-call-id]')).toHaveLength(1);
  });

  it('R02: a batch that committed no visible transcript item is unplaced and draws nothing', () => {
    const unplaced = status('s0', { post_tool_batch: { transcript_anchor: null } });
    expect(agentStatusAnchor(unplaced)).toEqual({ kind: 'unplaced' });
    const ui = render(<AgentTranscript snapshot={withTranscript(CONVERSATION(), [unplaced, status('sx', {})])} />);
    expect(annotations(ui.container)).toEqual([]);
  });

  it('R03: both opportunities render once at FreshInbound, never also at the batch anchor', () => {
    const both = status('s3', { ...fresh('u1'), ...batch('3') });
    expect(agentStatusAnchor(both)).toEqual({ kind: 'inbound_message', messageId: 'u1' });
    const ui = render(<AgentTranscript snapshot={withTranscript(CONVERSATION(), [both])} />);
    expect(annotations(ui.container)).toEqual([['s3', 'message:u1']]);
  });

  it('R03: an off-page FreshInbound target never falls back to an on-page batch anchor', () => {
    const both = status('s3', { ...fresh('u0'), ...batch('3') });
    const page = [assistant('a1', '2'), toolResult('t1', '3', 'a1')];
    const ui = render(<AgentTranscript snapshot={withTranscript(page, [both])} />);
    expect(annotations(ui.container)).toEqual([]);
    // Loading the selected anchor reveals it there, and only there.
    ui.rerender(<AgentTranscript snapshot={withTranscript([user('u0', '0'), ...page], [both])} />);
    expect(annotations(ui.container)).toEqual([['s3', 'message:u0']]);
  });

  it('R04: an off-page anchor relocates to nothing, and paging it in places it correctly', () => {
    const statuses = [status('old', fresh('u0')), status('new', batch('3'))];
    const page = CONVERSATION();
    const ui = render(<AgentTranscript snapshot={withTranscript(page, statuses)} history={replaceTranscript({ entries: page, next_cursor: '0' })} />);
    expect(annotations(ui.container)).toEqual([['new', 'message:t1']]);
    const earlier = [user('u0', '0'), ...page];
    ui.rerender(<AgentTranscript snapshot={withTranscript(page, statuses)} history={replaceTranscript({ entries: earlier })} />);
    expect(annotations(ui.container)).toEqual([['old', 'message:u0'], ['new', 'message:t1']]);
  });

  it('R06: compositions resolving to one transcript position keep runtime composition order and render once each', () => {
    // The inbound entry owns both a message identity and a cursor, so message- and
    // position-anchored compositions can resolve to the same row; they interleave by
    // composition order, never by anchor kind, timestamp or array identity.
    const statuses = [status('first', batch('1')), status('second', fresh('u1')), status('third', batch('1'))];
    const ui = render(<AgentTranscript snapshot={withTranscript(CONVERSATION(), statuses)} />);
    expect(annotations(ui.container)).toEqual([['first', 'message:u1'], ['second', 'message:u1'], ['third', 'message:u1']]);
  });

  it('R05: repeated observation of one status_message_id yields one annotation', () => {
    const once = status('s1', fresh('u1'));
    const twice = agentStatusPlacement([once, { ...once, sections: [todoSection('restated')] }, status('s2', fresh('u1'))]);
    expect(twice.byMessageId.get('u1')?.map(anchored => anchored.status.status_message_id)).toEqual(['s1', 's2']);
    const ui = render(<AgentTranscript snapshot={withTranscript(CONVERSATION(), [once, once])} />);
    expect(annotations(ui.container)).toEqual([['s1', 'message:u1']]);
  });

  it('renders only typed sections and never parses the rendered prose', () => {
    const ui = render(<AgentTranscript snapshot={withTranscript(CONVERSATION(), [status('s1', fresh('u1'), [todoSection('Ship the dock'), { type: 'background_executions', executions: [{ execution_id: 'e1', tool_id: 'bash', tool_name: 'bash', state: 'running' }], omitted_count: 1 }])])} />);
    const note = screen.getByRole('note', { name: 'Agent Status' });
    expect(note.textContent).toContain('todo 1 · background 2');
    expect(ui.container.textContent).not.toContain('rendered prose');
    fireEvent.click(within(note).getByRole('button', { expanded: false }));
    const sections = [...note.querySelectorAll('[data-status-section]')].map(node => node.getAttribute('data-status-section'));
    expect(sections).toEqual(['todo', 'background_executions']);
    expect(note.textContent).toContain('Doing Ship the dock');
    expect(note.textContent).toContain('bash · running');
    expect(note.textContent).toContain('… and 1 more');
    expect(ui.container.textContent).not.toContain('rendered prose');
  });

  it('is a subordinate annotation, not a conversation speaker or a response with actions', () => {
    const ui = render(<AgentTranscript snapshot={withTranscript(CONVERSATION(), [status('s1', fresh('u1'))])} />);
    expect(ui.container.querySelectorAll('[aria-label="Your message"]')).toHaveLength(1);
    expect(ui.container.querySelectorAll('[aria-label="Assistant response"]')).toHaveLength(1);
    const note = screen.getByRole('note', { name: 'Agent Status' });
    expect(note.closest('[aria-label="Your message"], [data-assistant-message]')).toBeNull();
    expect(within(note).queryByRole('button', { name: 'Copy' })).toBeNull();
  });

  it('keeps the streaming response, its position and the transcript paging controls intact', () => {
    const live: RuntimeClientSnapshot = { ...withTranscript(CONVERSATION(), [status('s2', batch('3'))]),
      attempt: { attempt_id: 'attempt-A', turn: 2, phase: { type: 'running' }, in_flight: { message_id: 'live', blocks: [{ type: 'text', block_index: 0, text: 'Still answering' }] } } };
    const ui = render(<AgentTranscript snapshot={live} history={replaceTranscript({ entries: CONVERSATION(), next_cursor: '0' })} loadEarlier={() => {}} />);
    expect(screen.getByRole('button', { name: 'Load earlier' })).toBeTruthy();
    expect(screen.getByLabelText('Streaming response').textContent).toContain('Still answering');
    expect(anchorKeys(ui.container)).toEqual(['message:u1', 'message:a1', 'message:t1', 'message:live']);
    expect(annotations(ui.container)).toEqual([['s2', 'message:t1']]);
  });
});

describe('Agent Status has one representation, and current Todo is never read from it', () => {
  const context = (id: string, kind: UserMessageBlock['kind'], text: string): MessageBlock =>
    ({ role: 'user', id, source: 'runtime', kind, content: [{ type: 'text', text }] });
  const agentStatusMessage = context('ctx-status', { context: { agent_status: { generated_at: '2026-09-14T10:42:00Z', modules: ['time'] } } }, 'Todo A is in progress');
  const environment = context('ctx-env', { context: 'extension_environment' }, 'Environment context');

  it('the canonical Agent Status Context message never reappears through generic Context chrome', () => {
    expect(isAgentStatusContext(agentStatusMessage)).toBe(true);
    expect(isAgentStatusContext(environment)).toBe(false);
    const base = withTranscript(CONVERSATION(), [status('s1', fresh('u1'), [todoSection('Todo A')])]);
    const ui = render(<AgentTranscript snapshot={{ ...base, messages: [agentStatusMessage, environment] }} />);
    const current = screen.getByText('Current context').parentElement as HTMLElement;
    expect(current.textContent).toContain('Environment context');
    expect(current.textContent).not.toContain('Todo A is in progress');
    // Exactly one representation of the composition: the anchored annotation.
    expect(annotations(ui.container)).toEqual([['s1', 'message:u1']]);
    expect(ui.container.textContent).not.toContain('Todo A is in progress');
  });

  it('a snapshot whose only Todo evidence is Agent Status history still has no current Todo dock', () => {
    const historical = withTranscript(CONVERSATION(), [status('s1', fresh('u1'), [todoSection('Todo A')])]);
    expect(todoDock(historical)).toEqual({ kind: 'absent' });
    expect(todoDock({ ...historical, todos: { tasks: [], next_id: '1' } })).toEqual({ kind: 'current', tasks: [] });
  });

  it('placement reads only opportunity facts, never sections, prose or arrival order', () => {
    const placement = agentStatusPlacement([status('s1', fresh('u1'), [todoSection('Todo A')]), status('s2', batch('3'))]);
    expect(statusesAt(placement, { messageId: 'u1', cursor: '1' }).map(item => item.status_message_id)).toEqual(['s1']);
    expect(statusesAt(placement, { messageId: 't1', cursor: '3' }).map(item => item.status_message_id)).toEqual(['s2']);
    expect(statusesAt(placement, { messageId: 'a1', cursor: '2' })).toEqual([]);
  });
});

describe('cold attach, live folding and resync converge on one placement', () => {
  const statuses = [status('s1', fresh('u1'), [todoSection('Todo A')]), status('s2', batch('3'))];
  const full = () => withTranscript(CONVERSATION(), statuses, { ...snapshot(), todos: { tasks: [{ id: '1', subject: 'Todo A', status: 'in_progress' }], next_id: '2' } });
  const mount = async (initial: RuntimeClientSnapshot) => {
    server = new Server(); server.snapshots.set('A', initial);
    await server.attached('A');
    localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A'] }));
    await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost} />); });
  };
  const transcript = () => document.querySelector('[aria-label="Canonical conversation"]') as HTMLElement;

  it('R12: a cold snapshot reproduces the identity, placement and order that live folding produced', async () => {
    await mount(withTranscript([], []));
    await act(() => server.update('A', withTranscript(CONVERSATION(), [statuses[0]])));
    await act(() => server.update('A', full()));
    const live = annotations(transcript());
    expect(live).toEqual([['s1', 'message:u1'], ['s2', 'message:t1']]);
    expect(screen.getByRole('region', { name: 'To-dos' }).textContent).toContain('1 in progress');
    cleanup(); await server.client.disconnect();

    await mount(full());
    expect(annotations(transcript())).toEqual(live);
    expect(screen.getByRole('region', { name: 'To-dos' }).textContent).toContain('1 in progress');
  });

  it('R05: repeated events and snapshot replacement never duplicate a composition', async () => {
    await mount(full());
    const before = annotations(transcript());
    // The same authoritative window observed again through the real event/refresh path.
    await act(() => server.update('A', full()));
    await act(() => server.update('A', { ...full(), statuses: [...statuses, statuses[0]] }));
    expect(annotations(transcript())).toEqual(before);
    expect(transcript().querySelectorAll('[data-agent-status="s1"]')).toHaveLength(1);
  });

  it('R04: paging manufactures no extra retention; the window stays the runtime\'s', async () => {
    const page = CONVERSATION();
    await mount({ ...snapshot(), statuses: [status('old', fresh('u0')), ...statuses], transcript: { entries: page, next_cursor: '0' } });
    expect(annotations(transcript())).toEqual([['s1', 'message:u1'], ['s2', 'message:t1']]);
    server.held.add('session/transcript');
    const earlier = server.client.loadEarlier('A');
    const request = await server.waitFor('session/transcript', 1);
    await act(async () => { server.socket.success(request, { type: 'transcript', page: { entries: [user('u0', '0')] } }); await earlier; });
    expect(annotations(transcript())).toEqual([['old', 'message:u0'], ['s1', 'message:u1'], ['s2', 'message:t1']]);
    expect(server.client.getSnapshot().views.A.snapshot?.statuses).toHaveLength(3);
  });

  it('the Inspector names the bounded window as history, not one current value', async () => {
    await mount(full());
    fireEvent.click(screen.getByRole('button', { name: 'Toggle Inspector' }));
    const panel = screen.getByRole('complementary', { name: 'Developer inspector' });
    const history = within(panel).getByText(/Agent Status history/);
    expect(history.textContent).toContain('recent compositions');
    fireEvent.click(history);
    expect((history.parentElement as HTMLElement).textContent).toContain('not current Agent, Todo, Goal or Queue state');
    // Raw native diagnostics, including the model-facing rendered text, are retained.
    expect((history.parentElement as HTMLElement).querySelector('pre')?.textContent).toContain('rendered prose s1');
    expect(within(panel).queryByText(/Tools, Subagents, Workflows and Agent status/)).toBeNull();
  });
});


it('background Agent Status translates every lifecycle label while preserving exact native Tool identity', () => {
  const tool = 'native.Tool /路径  exact';
  const labels = {
    starting: ['starting', '启动中'], running: ['running', '运行中'], cancelling: ['cancelling', '取消中'],
    publishing_terminal: ['publishing terminal', '发布结束状态中'], succeeded: ['succeeded', '成功'],
    failed: ['failed', '失败'], denied: ['denied', '已拒绝'], cancelled: ['cancelled', '已取消'],
    timed_out: ['timed out', '已超时'], outcome_unknown: ['outcome unknown', '结果未知'],
  } as const;
  for (const state of Object.keys(labels) as (keyof typeof labels)[]) {
    const composition = status('status-native', fresh('u1'), [{ type: 'background_executions', executions: [{ execution_id: 'e-native', tool_id: 'native-id', tool_name: tool, state }], omitted_count: 0 }]);
    const before = structuredClone(composition);
    for (const [index, locale] of (['en', 'zh'] as const).entries()) {
      expect(agentStatusFacets(translator(locale), composition)[0].values).toEqual([`${tool} · ${labels[state][index]}`]);
    }
    expect(composition).toEqual(before);
  }
});
