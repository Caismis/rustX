import { cleanup, fireEvent, render, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { PreviewContext } from '../src/app/components/ArtifactPreview';
import { deliveryConversation } from './turn-deliveries-fixture';
import { latestTurnDeliveries, turnDeliveries } from '../src/bindings/present';
import { readCut } from './fixture';

afterEach(cleanup);
const before = (first: Node, second: Node) => expect(first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

it.each([false, true])('native declarations form one Turn tail after final prose, before actions and the next user (deferred=%s)', deferred => {
  const state = deliveryConversation(deferred), openPreview = vi.fn(), download = vi.fn();
  state.transcript.entries!.push({ cursor: '5', item: { type: 'message', message: { role: 'user', source: 'human', id: 'next-user', content: [{ type: 'text', text: 'Next question' }] } } });
  const ui = render(<PreviewContext value={{ openPreview, download }}><AgentTranscript snapshot={state}/></PreviewContext>);
  const cards = ui.container.querySelector('[data-turn-deliveries]')!;
  expect(ui.container.querySelectorAll('[data-turn-deliveries]')).toHaveLength(1);
  expect(cards.closest('[hidden]')).toBeNull();
  before(ui.getByText('All checks passed.'), cards);
  before(cards, ui.getByLabelText('Completed Turn'));
  before(ui.getByLabelText('Completed Turn'), ui.getByText('Next question'));
  expect(cards.querySelectorAll('[data-delivery-card]')).toHaveLength(4);
  fireEvent.click(within(cards as HTMLElement).getByRole('button', { name: 'Show all 7 delivered files' }));
  expect([...cards.querySelectorAll('[data-presented-name]')].map(node => node.textContent)).toEqual(['report.md', 'b.txt', 'c.rs', 'd.png', 'e.bin', 'f.txt', 'README.md']);
  expect(cards.querySelector('[data-presented-description]')!.textContent).toContain('Latest report');
  fireEvent.click(ui.getByRole('button', { name: 'Preview report.md in sidebar' }));
  fireEvent.click(ui.getByRole('button', { name: 'Download report.md' }));
  const locator = deferred ? { type: 'by_ledger_position', position: 103 } : { type: 'by_message_id', message_id: 'declaration-3' };
  for (const action of [openPreview, download]) expect(action).toHaveBeenCalledWith(expect.objectContaining({ source: expect.objectContaining({ conversationId: state.conversation_id, locator, index: 0 }) }));
  if (!deferred) { fireEvent.click(ui.getByRole('button', { name: 'Worked' })); expect(cards.closest('[hidden]')).toBeNull(); }
});

it('running and incomplete windows publish no Turn-tail cards; other Turns never borrow declarations', () => {
  const state = deliveryConversation();
  for (const entry of state.transcript.entries!) if (entry.turn_process) entry.turn_process = { ...entry.turn_process, outcome: 'running' };
  delete state.transcript.entries!.at(-1)!.completed_response;
  const ui = render(<AgentTranscript snapshot={state}/>);
  expect(ui.container.querySelector('[data-delivery-card]')).toBeNull();
  const settled = deliveryConversation();
  settled.transcript.entries!.at(-1)!.completed_response!.origin.attempt_id = 'other-attempt';
  ui.rerender(<AgentTranscript snapshot={settled}/>);
  expect(ui.container.querySelector('[data-delivery-card]')).toBeNull();
  settled.transcript.entries = settled.transcript.entries!.slice(0, -1);
  ui.rerender(<AgentTranscript snapshot={settled}/>);
  expect(ui.container.querySelector('[data-delivery-card]')).toBeNull();
});

it('selection retains latest original coordinates, canonical order, and each separate Turn', () => {
  const state = deliveryConversation(), entries = state.transcript.entries!;
  const groups = turnDeliveries(entries, [], state.transcript), group = [...groups.values()][0];
  const files = latestTurnDeliveries(group.batches);
  expect(files).toHaveLength(7);
  expect(files[0].locator).toEqual({ type: 'by_message_id', message_id: 'declaration-3' });
  expect(files[0].file.description).toBe('Latest report');
  const second = structuredClone(entries[1]); second.cursor = '6'; second.turn_process!.attempt_id = 'second-attempt';
  expect(turnDeliveries([...entries, second], [], state.transcript).size).toBe(2);
  const failed = structuredClone(entries[1]);
  if (failed.item.type === 'message' && failed.item.message.role === 'tool') failed.item.message.result.status = { type: 'failed', error: 'No declaration' };
  expect(turnDeliveries([failed], [], state.transcript).size).toBe(0);
});

it('a frozen historical delivery tail does not borrow a newer live declaration', () => {
  const frozen = deliveryConversation(true), live = deliveryConversation(true), page = frozen.transcript;
  for (const descriptor of live.message_descriptors) for (const item of descriptor.deliveries!.items) item.file!.description = 'New live declaration';
  live.messages = []; live.transcript = { entries: [] };
  const ui = render(<AgentTranscript snapshot={live} history={{ epoch: 1, page, window: { cut: readCut(), page } }}/>);
  expect(ui.container.querySelectorAll('[data-turn-deliveries]')).toHaveLength(1);
  expect(ui.container.querySelector('[data-presented-description]')!.textContent).toContain('Latest report');
  expect(ui.container.textContent).not.toContain('New live declaration');
});

it.each(['cancelled', 'failed'] as const)('a %s Turn without a closing response keeps deliveries after its terminal content', outcome => {
  const state = deliveryConversation();
  for (const entry of state.transcript.entries!) if (entry.turn_process) entry.turn_process = { ...entry.turn_process, outcome, final_message_id: null, final_message_locator: null };
  const final = state.transcript.entries!.at(-1)!; delete final.completed_response;
  state.transcript.entries!.push({ cursor: '5', turn_process: final.turn_process, item: { type: 'attempt_terminal', turn: final.turn_process! } });
  const ui = render(<AgentTranscript snapshot={state}/>);
  const cards = ui.container.querySelector('[data-turn-deliveries]')!;
  expect(cards).not.toBeNull(); before(ui.getByText('All checks passed.'), cards);
  expect(ui.queryByLabelText('Completed Turn')).toBeNull();
});

it('a closing tail excludes declarations after its native closing position', () => {
  const state = deliveryConversation(), later = structuredClone(state.transcript.entries![1]);
  later.cursor = '6';
  if (later.item.type !== 'message' || later.item.message.role !== 'tool') throw Error('Expected declaration');
  later.item.message.id = 'late-declaration'; later.item.message.result.deliveries![0].path = 'out/late.txt'; later.item.message.result.deliveries![0].name = 'late.txt';
  state.transcript.entries!.push(later);
  const ui = render(<AgentTranscript snapshot={state}/>);
  fireEvent.click(ui.getByRole('button', { name: 'Show all 7 delivered files' }));
  expect(ui.queryByText('late.txt')).toBeNull();
});

it('an unrelated canonical position cannot borrow a transcript member with the same number', () => {
  const state = deliveryConversation(), foreign = deliveryConversation(true).message_descriptors[0];
  foreign.identity = null; foreign.turn_process = null; foreign.locator = { type: 'by_ledger_position', position: 2 };
  for (const item of foreign.deliveries!.items) item.file!.description = 'Wrong owner';
  const groups = turnDeliveries(state.transcript.entries!, [foreign], state.transcript);
  expect([...groups.values()][0].batches).toHaveLength(2);
  expect(latestTurnDeliveries([...groups.values()][0].batches)[0].file.description).toBe('Latest report');
});
