import { translator } from '../src/locale/translation';
import { expect, it } from 'vitest';
import { displayNumber, emptyDraft, finiteNumber, submission } from '../src/bindings/questionnaire';
it('encodes native Questionnaire scalar, choice, custom, boolean and partial answers exactly', () => {
  expect(finiteNumber(translator('en'), '1.25')).toBe('3ff4000000000000'); expect(displayNumber(finiteNumber(translator('en'), '1.25'))).toBe(1.25);
  expect(finiteNumber(translator('en'), '-0')).toBe('0000000000000000'); expect(() => finiteNumber(translator('en'), 'Infinity')).toThrow('finite');
  const response = submission(translator('en'), [
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
  expect(() => submission(translator('en'), [{ header: 'i', question: 'i', answer: { type: 'integer' } }], [{ ...emptyDraft(), text: '9223372036854775808' }])).toThrow('bounds');
});
