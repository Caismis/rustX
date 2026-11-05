import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { ForegroundToolExecution, QuestionSpecification, QuestionnaireSubmission } from '../../protocol/app-server/v72';
import { Questionnaire } from '../src/app/agent/Questionnaire';
import { QuestionPanels, RecordedQuestionPanel, useQuestionPanels } from '../src/app/agent/QuestionPanels';
import { QuestionRow } from '../src/presentation/agent/QuestionRow';
import { askUserRecord } from '../src/bindings/ask-user';
import { InteractionAudit } from '../src/app/agent/InteractionAudit';

afterEach(cleanup);
const questions: QuestionSpecification[] = [
  { header: 'Direction', question: 'Which direction?', answer: { type: 'single_choice', allow_custom: true, options: [
    { label: 'Keep (Recommended)', description: 'Keep the native owner.', preview: '**Native** preview' }, { label: 'Drop', description: 'Drop the owner.' },
  ] } },
  { header: 'Scope', question: 'Which scope?', answer: { type: 'text', min_length: null, max_length: null } },
];
const submission: QuestionnaireSubmission = { answers: [{ question_index: 0, answer: { type: 'option', value: { option_index: 0 } } }] };

it('reads recorded options and Markdown previews without offering a response or a blank custom input', () => {
  const close = vi.fn();
  render(<Questionnaire questions={questions} record={submission} onClose={close}/>);
  expect((screen.getByRole('radio', { name: 'Keep' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByRole('radio', { name: 'Keep' }).getAttribute('aria-checked')).toBe('true');
  expect(screen.getByText('Native').tagName).toBe('STRONG');
  expect(screen.queryByRole('textbox')).toBeNull();
  expect(screen.queryByRole('button', { name: /Submit|Skip|Dismiss/ })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Next question' }));
  expect(screen.getByText('Not answered')).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Previous question' }) as HTMLButtonElement).disabled).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'Close answer record' }));
  expect(close).toHaveBeenCalledOnce();
});

it('restores exact custom answers and keeps a skipped recommended question unselected', () => {
  render(<Questionnaire questions={questions} record={{ answers: [{ question_index: 1, answer: { type: 'text', value: { value: 'Only the UI' } } }] }} onClose={() => {}}/>);
  expect(screen.getByRole('radio', { name: 'Keep' }).getAttribute('aria-checked')).toBe('false');
  expect(screen.getByText('Not answered')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Next question' }));
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('Only the UI');
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).disabled).toBe(true);
});

function OpenRecord() {
  const panels = useQuestionPanels()!;
  return <button onClick={() => panels.show('record-1', { questions, submission })}>Open record</button>;
}
it('retires a read-only panel when the Conversation changes, including when returning to the old Conversation', () => {
  const surface = (scope: string) => <QuestionPanels scope={scope}><OpenRecord/><RecordedQuestionPanel/></QuestionPanels>;
  const ui = render(surface('conversation-A'));
  fireEvent.click(screen.getByText('Open record'));
  expect(screen.getByRole('radio', { name: 'Keep' })).toBeTruthy();
  ui.rerender(surface('conversation-B')); expect(screen.queryByRole('radio')).toBeNull();
  ui.rerender(surface('conversation-A')); expect(screen.queryByRole('radio')).toBeNull();
});

it('keeps the answer-panel action independent of the leading transcript disclosure', () => {
  const open = vi.fn();
  const ui = render(<QuestionRow row={{ verdict: 'answered', answered: 1, total: 1, questions: [{ question: 'Question', answers: ['Answer'] }] }} panelAction={{ label: 'View answers', open }}/>);
  expect(ui.container.querySelector('[data-disclosure-row]')?.getAttribute('role')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'View answers' })); expect(open).toHaveBeenCalledOnce();
  expect(screen.queryByText('Question')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Ask question' })); expect(screen.getByText('Question')).toBeTruthy();
  fireEvent.keyDown(screen.getByRole('button', { name: 'View answers' }), { key: 'Enter' });
  expect(screen.getByText('Question')).toBeTruthy();
});

it('builds the option-preserving panel from native ask_user fields and rejects an unpairable selection', () => {
  const tool: ForegroundToolExecution = { call_id: 'ask-1', tool_id: 'tool-ask-user', name: 'ask_user', block_index: 0, assembly_complete: true, committed_result_deferred: false,
    occurrence: { locator: { type: 'by_message_id', message_id: 'ask-message' }, block_index: 0 },
    state: { type: 'settled', arguments: JSON.stringify({ questions: questions.slice(0, 1).map(question => ({
    header: question.header, question: question.question, options: question.answer.type === 'single_choice' ? question.answer.options : [],
  })) }), result: { status: { type: 'success' }, duration_ms: '0', content: [{ type: 'json', value: { cancelled: false, answers: [{ question_index: 0, kind: 'option', answer: 'Drop' }] } }] } } };
  expect(askUserRecord(tool)?.submission.answers).toEqual([{ question_index: 0, answer: { type: 'option', value: { option_index: 1 } } }]);
  if (tool.state.type === 'settled' && tool.state.result.content?.[0].type === 'json') tool.state.result.content[0].value = { cancelled: false, answers: [{ question_index: 0, kind: 'option', answer: 'Missing' }] };
  expect(askUserRecord(tool)).toBeUndefined();
});

it('keeps the approval verdict in its row and the native reason in a structured detail section', () => {
  const ui = render(<InteractionAudit item={{ type: 'interaction_requested', event_id: 'approval-request', interaction_id: 'approval-1', timestamp: '', attempt_id: 'attempt-1', subject: {
    type: 'approval', invocation_id: { caller: 'agent', call_id: 'bash-1' }, tool_id: 'tool-bash', tool_name: 'bash', arguments_digest: 'native-digest', reason: 'Needs approval',
  } }} settlement={{ type: 'interaction_settled', event_id: 'approval-settled', interaction_id: 'approval-1', timestamp: '', attempt_id: 'attempt-1', settlement: { type: 'approved' } }}/>);
  fireEvent.click(screen.getByRole('button', { name: /Approval · bash/ }));
  expect(screen.getAllByText('Allowed')).toHaveLength(1); expect(screen.getAllByText('Needs approval')).toHaveLength(1);
  expect(ui.container.querySelector('[data-approval-record] p')).toBeNull();
});
