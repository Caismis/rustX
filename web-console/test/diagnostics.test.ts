import { expect, it } from 'vitest';
import { ProtocolLog, filterLog } from '../src/client/protocol-log';
import { displayNumber, emptyDraft, finiteNumber, submission } from '../src/bindings/questionnaire';
const frame = (id: number, session = 'A') => JSON.stringify({ jsonrpc: '2.0', id, method: 'session/read', params: { session_id: session } });
it('bounds raw retention by count and bytes and reports dropped/truncated frames', () => {
  const log = new ProtocolLog(3, 500, 100);
  for (let i = 0; i < 20; i++) log.observe('out', 1, frame(i));
  expect(log.getSnapshot().entries).toHaveLength(3);
  expect(log.getSnapshot().dropped).toBe(17); expect(log.getSnapshot().truncated).toBe(20);
  expect(log.getSnapshot().entries.reduce((sum, entry) => sum + entry.json.length * 2, 0)).toBeLessThanOrEqual(500);
});
it('filtering, pause, clear and resume are deterministic and keep bounded storage', () => {
  const log = new ProtocolLog(2);
  log.observe('out', 1, frame(1)); log.observe('out', 1, frame(2, 'B'));
  expect(filterLog(log.getSnapshot(), 'session/read', 'B', 'request')).toHaveLength(1);
  log.pause(true); const frozen = log.getSnapshot().entries;
  for (let i = 3; i <= 10; i++) log.observe('out', 1, frame(i));
  expect(log.getSnapshot().entries).toBe(frozen);
  log.pause(false); expect(log.getSnapshot().entries).toHaveLength(2);
  expect(log.getSnapshot().dropped).toBe(8);
  log.pause(true); log.clear(); expect(log.getSnapshot().entries).toEqual([]);
  log.observe('out', 2, frame(11)); expect(log.getSnapshot().entries).toEqual([]);
  log.pause(false); expect(log.getSnapshot().entries).toHaveLength(1);
});
it('encodes native Questionnaire scalar, choice, custom, boolean and partial answers exactly', () => {
  expect(finiteNumber('1.25')).toBe('3ff4000000000000'); expect(displayNumber(finiteNumber('1.25'))).toBe(1.25);
  expect(finiteNumber('-0')).toBe('0000000000000000'); expect(() => finiteNumber('Infinity')).toThrow('finite');
  const response = submission([
    { header: 'Integer', question: 'Integer?', answer: { type: 'integer' } },
    { header: 'Number', question: 'Number?', answer: { type: 'number' } },
    { header: 'Bool', question: 'Bool?', answer: { type: 'boolean' } },
    { header: 'Text', question: 'Text?', answer: { type: 'text' } },
    { header: 'Skip', question: 'Skip?', answer: { type: 'text' } },
    { header: 'Multi', question: 'Multi?', answer: { type: 'multi_choice', allow_custom: true, min_selected: 1, max_selected: 2, options: [{ label: 'One', description: '' }, { label: 'Two', description: '' }] } },
    { header: 'Custom', question: 'Custom?', answer: { type: 'single_choice', allow_custom: true, options: [] } },
  ], [{ ...emptyDraft(), text: '9007199254740993' }, { ...emptyDraft(), text: '1.25' }, { ...emptyDraft(), boolean: false },
    { ...emptyDraft(), text: 'rustX' }, emptyDraft(), { ...emptyDraft(), selected: [1, 0] }, { ...emptyDraft(), text: 'Custom' }]);
  expect(response.answers.map(item => item.answer)).toEqual([
    { type: 'integer', value: { value: '9007199254740993' } }, { type: 'number', value: { value: '3ff4000000000000' } },
    { type: 'boolean', value: { value: false } }, { type: 'text', value: { value: 'rustX' } },
    { type: 'options', value: { option_indices: [0, 1] } }, { type: 'custom', value: { answer: 'Custom' } },
  ]);
  expect(() => submission([{ header: 'i', question: 'i', answer: { type: 'integer' } }], [{ ...emptyDraft(), text: '9223372036854775808' }])).toThrow('bounds');
});

it('identifies a Session in a native create response even before an attachment exists', () => {
  const log = new ProtocolLog();
  log.observe('in', 1, JSON.stringify({ jsonrpc: '2.0', id: 'create', result: { type: 'session_transition', session: { id: 'new-session' } } }), { method: 'session/create' });
  expect(filterLog(log.getSnapshot(), 'session/create', 'new-session', 'response')).toHaveLength(1);
});
