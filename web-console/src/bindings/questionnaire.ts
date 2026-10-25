import { displayText, message, type Message, type Translate } from '../locale/translation';
import type { QuestionSpecification, QuestionnaireAnswerEntry, QuestionnaireSubmission } from '../../../protocol/app-server/v57';
import { readScalarAnswer, type ScalarAnswerRejection } from '../../../protocol/app-server/number';
export class QuestionnaireValidationError extends Error {
  constructor(tx: Translate, readonly notice: Message) { super(displayText(tx, notice)); }
}
/** `skipped` marks a question the user passed on; it is omitted like any unanswered one. */
export interface QuestionDraft { selected: number[]; text: string; boolean?: boolean; skipped?: boolean }
export const emptyDraft = (): QuestionDraft => ({ selected: [], text: '' });

/** This client's words for one shared machine-readable numeric rejection;
 * the reading, exactness rule and wire encoding are the protocol's. */
function scalarRejection(header: string, type: 'number' | 'integer', rejection: ScalarAnswerRejection): Message {
  if (type === 'integer') return rejection.reason === 'malformed'
    ? message('interactions:copy.value-enter-a-canonical-whole-number', { p0: header })
    : message('interactions:copy.value-integer-is-outside-the-declared-bounds', { p0: header });
  switch (rejection.reason) {
    case 'malformed': return message('interactions:copy.enter-a-number');
    case 'not-finite': return message('interactions:copy.enter-a-finite-number');
    case 'inexact': return message('interactions:copy.value-number-is-not-exactly-representable', { p0: header });
    default: return message('interactions:copy.value-number-is-outside-the-declared-bounds', { p0: header });
  }
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
      case 'integer': case 'number': {
        const reading = readScalarAnswer(shape, draft.text);
        if (reading.kind === 'rejected') throw new QuestionnaireValidationError(tx, scalarRejection(question.header, shape.type, reading));
        answer = reading.answer; break;
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
