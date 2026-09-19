import type { QuestionSpecification, QuestionnaireAnswerEntry, QuestionnaireSubmission } from '../../../protocol/app-server/v9';
export interface QuestionDraft { selected: number[]; text: string; boolean?: boolean }
export const emptyDraft = (): QuestionDraft => ({ selected: [], text: '' });

/** Encode the existing lossless binary64 wire scalar, never a JSON number. */
export function finiteNumber(text: string): string {
  if (!text.trim()) throw new Error('Enter a number.');
  const number = Number(text);
  if (!Number.isFinite(number)) throw new Error('Enter a finite number.');
  const bytes = new DataView(new ArrayBuffer(8));
  bytes.setFloat64(0, Object.is(number, -0) ? 0 : number, false);
  return bytes.getBigUint64(0, false).toString(16).padStart(16, '0');
}
export function displayNumber(wire: string): number {
  const bytes = new DataView(new ArrayBuffer(8));
  bytes.setBigUint64(0, BigInt(`0x${wire}`), false);
  return bytes.getFloat64(0, false);
}
export function submission(questions: QuestionSpecification[], drafts: QuestionDraft[]): QuestionnaireSubmission {
  const answers: QuestionnaireAnswerEntry[] = [];
  questions.forEach((question, question_index) => {
    const draft = drafts[question_index];
    const shape = question.answer;
    // Native partial submissions omit unanswered questions. No fabricated skip.
    if (draft.text === '' && draft.selected.length === 0 && draft.boolean === undefined) return;
    let answer: QuestionnaireAnswerEntry['answer'];
    switch (shape.type) {
      case 'text': {
        const length = [...draft.text].length;
        if (length < (shape.min_length ?? 0) || length > (shape.max_length ?? Infinity)) throw new Error(`${question.header}: text length is outside the declared bounds.`);
        answer = { type: 'text', value: { value: draft.text } }; break;
      }
      case 'integer': {
        if (!/^(0|[1-9][0-9]*|-[1-9][0-9]*)$/.test(draft.text)) throw new Error(`${question.header}: enter a canonical whole number.`);
        const integer = BigInt(draft.text);
        if (integer < BigInt(shape.minimum ?? '-9223372036854775808') || integer > BigInt(shape.maximum ?? '9223372036854775807') || integer < -9223372036854775808n || integer > 9223372036854775807n) throw new Error(`${question.header}: integer is outside the declared bounds.`);
        answer = { type: 'integer', value: { value: draft.text } }; break;
      }
      case 'number': {
        const value = finiteNumber(draft.text);
        const numeric = displayNumber(value);
        if (numeric < (shape.minimum ? displayNumber(shape.minimum) : -Infinity) || numeric > (shape.maximum ? displayNumber(shape.maximum) : Infinity)) throw new Error(`${question.header}: number is outside the declared bounds.`);
        answer = { type: 'number', value: { value } }; break;
      }
      case 'boolean': answer = { type: 'boolean', value: { value: draft.boolean! } }; break;
      case 'single_choice': case 'multi_choice': {
        if (draft.text !== '' && shape.allow_custom) { answer = { type: 'custom', value: { answer: draft.text } }; break; }
        const selected = [...new Set(draft.selected)].sort((a, b) => a - b);
        if (selected.some(index => index < 0 || index >= shape.options.length)) throw new Error('Invalid option index.');
        if (shape.type === 'single_choice') {
          if (selected.length !== 1) throw new Error(`${question.header}: choose one option.`);
          answer = { type: 'option', value: { option_index: selected[0] } };
        } else {
          if (selected.length < shape.min_selected || selected.length > shape.max_selected) throw new Error(`${question.header}: choose ${shape.min_selected}–${shape.max_selected} options.`);
          answer = { type: 'options', value: { option_indices: selected } };
        }
        break;
      }
    }
    answers.push({ question_index, answer });
  });
  return { answers };
}
