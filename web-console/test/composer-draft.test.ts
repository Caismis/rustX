import { expect, it } from 'vitest';
import { ComposerDraft } from '../src/app/composer/draft';
import { capabilities } from './fixture';
import { AttachmentIntake } from '../src/client/uploads';
const accepted = { type: 'inbound_accepted' as const, message_id: 'native-message', inbound_sequence: '7' };
const content: [] = [];
function editor() { const owner = new ComposerDraft(), intake = new AttachmentIntake(); const state = owner.activate('A', content, 'proof-A'); owner.edit(state.incarnation, 'R1'); return { owner, intake, incarnation: state.incarnation }; }
it('acceptance settles once, retains native identity and never recreates unsent content on activation', () => {
  const f = editor(), ticket = f.owner.submit(f.incarnation, f.intake, 'send')!;
  ticket.accepted(accepted); const settled = f.owner.snapshot(); ticket.accepted(accepted);
  expect(f.owner.snapshot()).toBe(settled); expect(settled?.text).toBe(''); expect(ticket.acceptance).toBe(accepted);
  expect(f.owner.activate('A', content, 'proof-A')).toBe(settled);
});
it.each(['R2', 'R1'])('an authored later revision %s survives older acceptance regardless of text equality', text => {
  const f = editor(), ticket = f.owner.submit(f.incarnation, f.intake, 'steer')!;
  f.owner.edit(f.incarnation, text); ticket.accepted(accepted);
  expect(f.owner.snapshot()?.text).toBe(text); expect(ticket.outcome).toBe('accepted');
});
it.each(['B', 'replacement'])('old acknowledgement cannot mutate the %s editor incarnation', replacement => {
  const f = editor(), ticket = f.owner.submit(f.incarnation, f.intake, 'send')!;
  if (replacement === 'B') f.owner.activate('B', content, 'proof-B'); else f.owner.replace(f.incarnation);
  const incarnation = f.owner.snapshot()!.incarnation; f.owner.edit(incarnation, 'replacement'); ticket.accepted(accepted);
  expect(f.owner.snapshot()?.text).toBe('replacement'); expect(f.owner.snapshot()?.incarnation).toBe(incarnation);
});
it('uncertain is not accepted or replayable; remount and failure cannot downgrade it', () => {
  const f = editor(), ticket = f.owner.submit(f.incarnation, f.intake, 'send')!;
  ticket.failed('uncertain'); ticket.failed('unsent'); f.owner.activate('A', content, 'proof-A');
  expect(ticket.outcome).toBe('uncertain'); expect(f.owner.snapshot()?.text).toBe('R1'); expect(f.owner.submit(f.incarnation, f.intake, 'send')).toBeUndefined();
  ticket.accepted(accepted); expect(f.owner.snapshot()?.text).toBe('');
});
it('explicit replacement retains its new incarnation and retires uncertain tickets across repeated activation', () => {
  const f = editor(), old = f.owner.submit(f.incarnation, f.intake, 'send')!; old.failed('uncertain');
  expect(f.owner.replace(f.incarnation)).toBe(true); const incarnation = f.owner.snapshot()!.incarnation;
  f.owner.edit(incarnation, 'replacement'); f.owner.select(incarnation, { start: 2, end: 5, direction: 'backward' });
  for (let i = 0; i < 2; i++) f.owner.activate('A', content, 'proof-A');
  old.accepted(accepted); expect(f.owner.snapshot()).toMatchObject({ incarnation, text: 'replacement', selection: { start: 2, end: 5, direction: 'backward' } });
  expect(f.owner.replace(f.incarnation)).toBe(false);
});
it('native receipt authority replacement cannot consume replacement selections or restored receipts', () => {
  const f = editor(); f.intake.add([{ file: new File(['x'], 'x.txt') }], capabilities.upload_policy);
  const ticket = f.owner.submit(f.incarnation, f.intake, 'send')!;
  f.intake.bind('replacement', 'replacement'); const replacement = f.intake.snapshot();
  f.owner.activate('A', content, 'replacement'); ticket.accepted(accepted);
  expect(f.intake.snapshot()).toBe(replacement); expect(f.owner.snapshot()?.restored).toEqual([]);
});

it('older acceptance cannot unblock a newer already submitted revision', () => {
  const f = editor(), first = f.owner.submit(f.incarnation, f.intake, 'send')!;
  f.owner.edit(f.incarnation, 'R2'); const second = f.owner.submit(f.incarnation, f.intake, 'send')!;
  first.accepted(accepted); expect(f.owner.blocked()).toBe(true); expect(second.outcome).toBe('pending');
  expect(f.owner.submit(f.incarnation, f.intake, 'send')).toBeUndefined(); second.accepted(accepted);
  expect(f.owner.snapshot()?.text).toBe('');
});

it('initial content is adopted once; equivalent, empty and cleared presentation arrays cannot reset authored content', () => {
  const owner = new ComposerDraft(), initial = [{ type: 'upload' as const, session_id: 'A', batch_id: 'batch', token: 'token' }, { type: 'text' as const, text: 'Native seed' }];
  const identity = { session_id: 'A', conversation_id: 'C1' };
  const state = owner.activate('A', initial, 'proof', identity);
  owner.edit(state.incarnation, 'Authored replacement'); owner.select(state.incarnation, { start: 2, end: 7, direction: 'backward' });
  const authored = owner.snapshot();
  for (const value of [initial, structuredClone(initial), [], [], [{ type: 'text' as const, text: 'Late native seed' }]]) {
    owner.activate('A', value, 'proof', { ...identity }); expect(owner.snapshot()).toBe(authored);
  }
  owner.replace(state.incarnation); owner.edit(owner.snapshot()!.incarnation, 'After Discard');
  owner.activate('A', initial, 'proof', identity); expect(owner.snapshot()?.text).toBe('After Discard'); expect(owner.snapshot()?.restored).toEqual([]);
  const next = owner.activate('B', [{ type: 'text', text: 'New authorized seed' }], 'proof-B', { session_id: 'B', conversation_id: 'C2' });
  expect(next.text).toBe('New authorized seed'); expect(next.revision).toBe(0); expect(next.incarnation).not.toBe(state.incarnation);
});
it('C1 acceptance is recorded without changing a retained same-revision editor now owned by C2', () => {
  const f = editor(), c1 = { session_id: 'A', conversation_id: 'C1' }, c2 = { session_id: 'A', conversation_id: 'C2' };
  f.owner.activate('A', content, 'proof-A', c1);
  const ticket = f.owner.submit(f.incarnation, f.intake, 'send')!;
  f.owner.activate('A', content, 'proof-A', c2); const replacement = f.owner.snapshot();
  expect(replacement?.incarnation).toBe(ticket.incarnation); expect(replacement?.revision).toBe(ticket.revision);
  ticket.accepted(accepted); expect(ticket.outcome).toBe('accepted'); expect(ticket.acceptance).toBe(accepted);
  expect(f.owner.snapshot()).toBe(replacement); expect(replacement?.text).toBe('R1'); expect(f.owner.blocked()).toBe(false);
});
it('same canonical Conversation permits text settlement after reattachment but never consumes replacement receipt selections', () => {
  const f = editor(), c1 = { session_id: 'A', conversation_id: 'C1' };
  f.owner.activate('A', content, 'proof-A', c1);
  const file = new File(['one'], 'one.txt'); f.intake.add([{ file }], capabilities.upload_policy);
  const ticket = f.owner.submit(f.incarnation, f.intake, 'send')!;
  f.intake.bind('replacement', 'replacement'); const selections = f.intake.snapshot();
  f.owner.activate('A', content, 'replacement', { ...c1 }); ticket.accepted(accepted);
  expect(f.owner.snapshot()?.text).toBe(''); expect(f.intake.snapshot()).toBe(selections); expect(ticket.outcome).toBe('accepted');
});
