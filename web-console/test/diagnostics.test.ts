import { translator } from '../src/locale/translation';
import { expect, it } from 'vitest';
import { emptyDraft, submission } from '../src/bindings/questionnaire';
it('encodes native Questionnaire scalar, choice, custom, boolean and partial answers exactly', () => {
  const number = (text: string) => submission(translator('en'), [{ header: 'N', question: 'N?', answer: { type: 'number' } }], [{ ...emptyDraft(), text }]).answers[0]?.answer;
  expect(number('1.25')).toEqual({ type: 'number', value: { value: '3ff4000000000000' } });
  expect(number('-0')).toEqual({ type: 'number', value: { value: '0000000000000000' } });
  expect(() => number('Infinity')).toThrow('Enter a number.'); expect(() => number('1e400')).toThrow('finite');
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
it('shares the exact native numeric contract with the terminal client', () => {
  const number = (text: string) => submission(translator('en'), [{ header: 'N', question: 'N?', answer: { type: 'number' } }], [{ ...emptyDraft(), text }]).answers[0]?.answer;
  // Inexact whole numbers are refused however they are spelled, never rounded.
  for (const text of ['9007199254740993', '9007199254740993.0', '9.007199254740993e15']) expect(() => number(text)).toThrow('represent exactly');
  // 2^63 is exactly representable, so the blanket safe-integer rule is not the contract.
  expect(number('9223372036854775808')).toEqual({ type: 'number', value: { value: '43e0000000000000' } });
  expect(number('1.25')).toEqual({ type: 'number', value: { value: '3ff4000000000000' } });
  const integer = (text: string) => submission(translator('en'), [{ header: 'I', question: 'I?', answer: { type: 'integer' } }], [{ ...emptyDraft(), text }]).answers[0]?.answer;
  expect(integer(' 42 ')).toEqual({ type: 'integer', value: { value: '42' } });
  for (const text of ['007', '-0', '1e3']) expect(() => integer(text)).toThrow('canonical whole number');
});

