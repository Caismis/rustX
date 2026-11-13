import { parseModelToolCall } from '../../../protocol/app-server/model-tool-call';
import type { ForegroundToolExecution, QuestionSpecification, QuestionnaireSubmission } from '../../../protocol/app-server/v79';
import type { QuestionRowView } from '../presentation/agent/QuestionRow';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(item => typeof item === 'string');

function questions(argumentsText: string): string[] | undefined {
  const parsed = parseModelToolCall(argumentsText)?.arguments;
  if (!record(parsed) || !Array.isArray(parsed.questions) || !parsed.questions.length) return undefined;
  const texts = parsed.questions.map(item => record(item) && typeof item.question === 'string' ? item.question : undefined);
  return texts.every(text => text !== undefined) ? texts as string[] : undefined;
}

/** One answer line per selected label or custom text; scalar answers print as JSON. */
function answerLines(entry: Record<string, unknown>): string[] | undefined {
  if (entry.kind === 'multiple') return strings(entry.selected) ? entry.selected : undefined;
  if (entry.answer === undefined) return undefined;
  return [typeof entry.answer === 'string' ? entry.answer : JSON.stringify(entry.answer)];
}

/** The transcript verdict of one native `ask_user` call, read only from its
 * persisted arguments and result. Absent when the call failed or its records
 * cannot be read unambiguously; the generic tool card then shows them raw. */
export function askUserRow(tool: ForegroundToolExecution): QuestionRowView | undefined {
  const id = tool.call_id??undefined;
  if (tool.state.type !== 'settled') return { id, verdict: 'waiting' };
  const status = tool.state.result.status.type;
  const asked = questions(tool.state.arguments);
  if (status === 'cancelled') return asked && { id, verdict: 'interrupted', questions: asked };
  if (status !== 'success' || !asked) return undefined;
  const body = tool.committed_result ?? tool.state.result;
  if (body.status.type !== 'success') return undefined;
  const blocks = body.content ?? [];
  const value = blocks.length === 1 && blocks[0]?.type === 'json' ? blocks[0].value : undefined;
  if (!record(value) || typeof value.cancelled !== 'boolean' || !Array.isArray(value.answers)) return undefined;
  if (value.cancelled) return { id, verdict: 'cancelled', questions: asked };
  // Unanswered questions are omitted from the result; answers echo their question's index.
  const byIndex = new Map<number, string[]>();
  for (const entry of value.answers) {
    if (!record(entry) || typeof entry.question_index !== 'number' || !Number.isInteger(entry.question_index)
      || entry.question_index < 0 || entry.question_index >= asked.length || byIndex.has(entry.question_index)) return undefined;
    const lines = answerLines(entry);
    if (!lines) return undefined;
    byIndex.set(entry.question_index, lines);
  }
  return { id, verdict: 'answered', answered: byIndex.size, total: asked.length,
    questions: asked.map((question, index) => ({ question, answers: byIndex.get(index) ?? [] })) };
}

/** Preserve authored options for Harness's read-only answer panel. No answer channel. */
export function askUserRecord(tool: ForegroundToolExecution): { questions: QuestionSpecification[]; submission: QuestionnaireSubmission } | undefined {
  if (askUserRow(tool)?.verdict !== 'answered' || tool.state.type !== 'settled') return undefined;
  const args = parseModelToolCall(tool.state.arguments)?.arguments;
  if (!record(args) || !Array.isArray(args.questions)) return undefined;
  const questions: QuestionSpecification[] = [];
  for (const question of args.questions) {
    if (!record(question) || typeof question.header !== 'string' || typeof question.question !== 'string'
      || !Array.isArray(question.options) || !question.options.length) return undefined;
    const options: { label: string; description: string; preview?: string }[] = [];
    for (const option of question.options) {
      if (!record(option) || typeof option.label !== 'string' || typeof option.description !== 'string') return undefined;
      options.push({ label: option.label, description: option.description, ...(typeof option.preview === 'string' ? { preview: option.preview } : {}) });
    }
    if (new Set(options.map(option => option.label)).size !== options.length) return undefined;
    questions.push({ header: question.header, question: question.question, answer: question.multi_select === true
      ? { type: 'multi_choice', options, allow_custom: true, min_selected: 1, max_selected: options.length }
      : { type: 'single_choice', options, allow_custom: true } });
  }
  const body = tool.committed_result ?? tool.state.result, value = body.content?.[0];
  if (value?.type !== 'json' || !record(value.value) || !Array.isArray(value.value.answers)) return undefined;
  const answers: QuestionnaireSubmission['answers'] = [];
  for (const entry of value.value.answers) {
    if (!record(entry) || typeof entry.question_index !== 'number') return undefined;
    const question = questions[entry.question_index];
    if (!question || (question.answer.type !== 'single_choice' && question.answer.type !== 'multi_choice')) return undefined;
    const { options } = question.answer;
    if (entry.kind === 'custom' && typeof entry.answer === 'string') answers.push({ question_index: entry.question_index, answer: { type: 'custom', value: { answer: entry.answer } } });
    else if (entry.kind === 'option' && question.answer.type === 'single_choice' && typeof entry.answer === 'string') {
      const index = options.findIndex(option => option.label === entry.answer);
      if (index < 0) return undefined;
      answers.push({ question_index: entry.question_index, answer: { type: 'option', value: { option_index: index } } });
    } else if (entry.kind === 'multiple' && question.answer.type === 'multi_choice' && strings(entry.selected)) {
      const indices = entry.selected.map(label => options.findIndex(option => option.label === label));
      if (indices.some(index => index < 0) || new Set(indices).size !== indices.length) return undefined;
      answers.push({ question_index: entry.question_index, answer: { type: 'options', value: { option_indices: indices } } });
    } else return undefined;
  }
  return { questions, submission: { answers } };
}
