import { displayText, message, type Message, type Translate } from '../locale/translation';
import type { QuestionSpecification, QuestionnaireAnswerEntry, QuestionnaireSubmission } from '../../../protocol/app-server/v23';
export class QuestionnaireValidationError extends Error {
  constructor(tx: Translate, readonly notice: Message) { super(displayText(tx, notice)); }
}
export interface QuestionDraft { selected: number[]; text: string; boolean?: boolean }
export const emptyDraft = (): QuestionDraft => ({ selected: [], text: '' });

/** Encode the existing lossless binary64 wire scalar, never a JSON number. */
export function finiteNumber(tx: Translate, text: string): string {
  if (!text.trim()) throw new QuestionnaireValidationError(tx, message('interactions:copy.enter-a-number'));
  const number = Number(text);
  if (!Number.isFinite(number)) throw new QuestionnaireValidationError(tx, message('interactions:copy.enter-a-finite-number'));
  const bytes = new DataView(new ArrayBuffer(8));
  bytes.setFloat64(0, Object.is(number, -0) ? 0 : number, false);
  return bytes.getBigUint64(0, false).toString(16).padStart(16, '0');
}
export function displayNumber(wire: string): number {
  const bytes = new DataView(new ArrayBuffer(8));
  bytes.setBigUint64(0, BigInt(`0x${wire}`), false);
  return bytes.getFloat64(0, false);
}
export function submission(tx: Translate, questions: QuestionSpecification[], drafts: QuestionDraft[]): QuestionnaireSubmission {
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
        if (length < (shape.min_length ?? 0) || length > (shape.max_length ?? Infinity)) throw new QuestionnaireValidationError(tx, message('interactions:copy.value-text-length-is-outside-the-declared-bounds', { p0: question.header }));
        answer = { type: 'text', value: { value: draft.text } }; break;
      }
      case 'integer': {
        if (!/^(0|[1-9][0-9]*|-[1-9][0-9]*)$/.test(draft.text)) throw new QuestionnaireValidationError(tx, message('interactions:copy.value-enter-a-canonical-whole-number', { p0: question.header }));
        const integer = BigInt(draft.text);
        if (integer < BigInt(shape.minimum ?? '-9223372036854775808') || integer > BigInt(shape.maximum ?? '9223372036854775807') || integer < -9223372036854775808n || integer > 9223372036854775807n) throw new QuestionnaireValidationError(tx, message('interactions:copy.value-integer-is-outside-the-declared-bounds', { p0: question.header }));
        answer = { type: 'integer', value: { value: draft.text } }; break;
      }
      case 'number': {
        const value = finiteNumber(tx, draft.text);
        const numeric = displayNumber(value);
        if (numeric < (shape.minimum ? displayNumber(shape.minimum) : -Infinity) || numeric > (shape.maximum ? displayNumber(shape.maximum) : Infinity)) throw new QuestionnaireValidationError(tx, message('interactions:copy.value-number-is-outside-the-declared-bounds', { p0: question.header }));
        answer = { type: 'number', value: { value } }; break;
      }
      case 'boolean': answer = { type: 'boolean', value: { value: draft.boolean! } }; break;
      case 'single_choice': case 'multi_choice': {
        if (draft.text !== '' && shape.allow_custom) { answer = { type: 'custom', value: { answer: draft.text } }; break; }
        const selected = [...new Set(draft.selected)].sort((a, b) => a - b);
        if (selected.some(index => index < 0 || index >= shape.options.length)) throw new QuestionnaireValidationError(tx, message('interactions:copy.invalid-option-index'));
        if (shape.type === 'single_choice') {
          if (selected.length !== 1) throw new QuestionnaireValidationError(tx, message('interactions:copy.value-choose-one-option', { p0: question.header }));
          answer = { type: 'option', value: { option_index: selected[0] } };
        } else {
          if (selected.length < shape.min_selected || selected.length > shape.max_selected) throw new QuestionnaireValidationError(tx, message('interactions:copy.value-choose-value-value-options', { p0: question.header, p1: shape.min_selected, p2: shape.max_selected }));
          answer = { type: 'options', value: { option_indices: selected } };
        }
        break;
      }
    }
    answers.push({ question_index, answer });
  });
  return { answers };
}
