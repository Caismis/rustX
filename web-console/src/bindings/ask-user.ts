import type { ForegroundToolExecution } from '../../../protocol/app-server/v36';
import type { QuestionRowView } from '../presentation/agent/QuestionRow';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(item => typeof item === 'string');

function questions(argumentsText: string): string[] | undefined {
  let parsed: unknown;
  try { parsed = JSON.parse(argumentsText); } catch { return undefined; }
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
  const id = tool.call_id;
  if (tool.state.type !== 'settled') return { id, verdict: 'waiting' };
  const status = tool.state.result.status.type;
  const asked = questions(tool.state.arguments);
  if (status === 'cancelled') return asked && { id, verdict: 'interrupted', questions: asked };
  if (status !== 'success' || !asked) return undefined;
  const value = (tool.state.result.content ?? []).find(block => block.type === 'json')?.value;
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
