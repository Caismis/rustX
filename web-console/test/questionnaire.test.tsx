import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import type { ForegroundToolExecution, QuestionSpecification, QuestionnaireSubmission } from '../../protocol/app-server/v43';
import { Questionnaire } from '../src/app/agent/Questionnaire';
import { askUserRow } from '../src/bindings/ask-user';
import { QuestionRow } from '../src/presentation/agent/QuestionRow';

afterEach(cleanup);

const choice = (header: string, labels: string[], multi = false): QuestionSpecification => ({
  header, question: `${header}?`,
  answer: multi
    ? { type: 'multi_choice', min_selected: 1, max_selected: labels.length, allow_custom: true, options: labels.map(label => ({ label, description: `${label} trade-off` })) }
    : { type: 'single_choice', allow_custom: true, options: labels.map(label => ({ label, description: `${label} trade-off` })) },
});
function flow(questions: QuestionSpecification[]) {
  const result: { submitted?: QuestionnaireSubmission; declined: number } = { declined: 0 };
  render(<Questionnaire questions={questions} disabled={false} onSubmit={value => { result.submitted = value; }} onDecline={() => { result.declined++; }}/>);
  return result;
}
const primary = (name: string) => screen.getByRole('button', { name }) as HTMLButtonElement;

it('a recommended first choice is the implicit draft and a single choice advances to the next question', () => {
  const result = flow([choice('Direction', ['Keep (Recommended)', 'Drop']), choice('Coverage', ['Unit', 'Browser'], true)]);
  expect(screen.getByRole('radio', { name: 'Keep' }).getAttribute('aria-checked')).toBe('true');
  expect(screen.getByText('Recommended')).toBeTruthy();
  fireEvent.click(screen.getByRole('radio', { name: 'Drop' }));
  expect(screen.getByText('Coverage?')).toBeTruthy();
  expect(screen.getByText('2 / 2')).toBeTruthy();
  expect(primary('Submit').disabled).toBe(true);
  fireEvent.click(screen.getByRole('checkbox', { name: 'Unit' }));
  fireEvent.click(screen.getByRole('checkbox', { name: 'Browser' }));
  fireEvent.click(primary('Submit'));
  expect(result.submitted).toEqual({ answers: [
    { question_index: 0, answer: { type: 'option', value: { option_index: 1 } } },
    { question_index: 1, answer: { type: 'options', value: { option_indices: [0, 1] } } },
  ] });
});

it('the always-visible custom row replaces the selection and Enter continues while Shift+Enter breaks a line', () => {
  const result = flow([choice('Direction', ['Keep', 'Drop']), choice('Scope', ['Small', 'Large'])]);
  fireEvent.click(screen.getByRole('radio', { name: 'Keep' }));
  fireEvent.click(screen.getByRole('button', { name: 'Previous question' }));
  const custom = screen.getByPlaceholderText('Type your answer') as HTMLTextAreaElement;
  fireEvent.change(custom, { target: { value: 'Neither' } });
  expect(screen.getByRole('radio', { name: 'Keep' }).getAttribute('aria-checked')).toBe('false');
  fireEvent.keyDown(custom, { key: 'Enter', shiftKey: true });
  expect(screen.getByText('Direction?')).toBeTruthy();
  fireEvent.keyDown(custom, { key: 'Enter' });
  expect(screen.getByText('Scope?')).toBeTruthy();
  fireEvent.click(screen.getByRole('radio', { name: 'Large' }));
  fireEvent.click(primary('Submit'));
  expect(result.submitted).toEqual({ answers: [
    { question_index: 0, answer: { type: 'custom', value: { answer: 'Neither' } } },
    { question_index: 1, answer: { type: 'option', value: { option_index: 1 } } },
  ] });
});

it('a skipped question is omitted, an unvisited one blocks submission on its own page, and dismiss declines the set', () => {
  const result = flow([choice('First', ['A', 'B']), choice('Second', ['C', 'D']), choice('Third', ['E', 'F'])]);
  fireEvent.click(screen.getByRole('button', { name: 'Skip' }));
  expect(screen.getByText('Second?')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Next question' }));
  fireEvent.click(screen.getByRole('radio', { name: 'F' }));
  fireEvent.click(primary('Submit'));
  expect(result.submitted).toBeUndefined();
  expect(screen.getByText('Second?')).toBeTruthy();
  expect(screen.getByRole('status').textContent).toBe('Please complete this question first.');
  fireEvent.click(screen.getByRole('radio', { name: 'C' }));
  fireEvent.click(primary('Submit'));
  expect(result.submitted).toEqual({ answers: [
    { question_index: 1, answer: { type: 'option', value: { option_index: 0 } } },
    { question_index: 2, answer: { type: 'option', value: { option_index: 1 } } },
  ] });
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss all questions' }));
  expect(result.declined).toBe(1);
});

it('collapsing keeps the header strip and the draft', () => {
  flow([choice('Direction', ['Keep', 'Drop']), choice('Scope', ['Small', 'Large'])]);
  fireEvent.click(screen.getByRole('radio', { name: 'Keep' }));
  fireEvent.click(screen.getByRole('button', { name: 'Collapse the question card' }));
  expect(screen.getByText('Scope?')).toBeTruthy();
  expect(screen.queryByRole('radio')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Expand the question card' }));
  fireEvent.click(screen.getByRole('button', { name: 'Previous question' }));
  expect(screen.getByRole('radio', { name: 'Keep' }).getAttribute('aria-checked')).toBe('true');
});

const asked = JSON.stringify({ questions: [{ header: 'Direction', question: 'Which direction?', options: [] }, { header: 'Scope', question: 'Which scope?', options: [] }] });
const call = (state: ForegroundToolExecution['state']): ForegroundToolExecution =>
  ({ message_id: 'm', block_index: 0, call_id: 'call-ask', tool_id: 'tool-ask-user', name: 'ask_user', state });
const settled = (status: { type: string }, value?: unknown) => call({ type: 'settled', arguments: asked,
  result: { status, duration_ms: 0, content: value === undefined ? [] : [{ type: 'json', value }] } } as ForegroundToolExecution['state']);

it('the native ask_user record projects its verdict and pairs answers by question index', () => {
  expect(askUserRow(call({ type: 'running', arguments: '{"questions":[' }))).toEqual({ id: 'call-ask', verdict: 'waiting' });
  expect(askUserRow(settled({ type: 'success' }, { cancelled: false, answers: [
    { question_index: 1, question: 'Which scope?', header: 'Scope', kind: 'multiple', selected: ['Unit', 'Browser'] },
  ] }))).toEqual({ id: 'call-ask', verdict: 'answered', answered: 1, total: 2, questions: [
    { question: 'Which direction?', answers: [] }, { question: 'Which scope?', answers: ['Unit', 'Browser'] },
  ] });
  expect(askUserRow(settled({ type: 'success' }, { cancelled: true, answers: [] }))).toEqual({ id: 'call-ask', verdict: 'cancelled', questions: ['Which direction?', 'Which scope?'] });
  expect(askUserRow(settled({ type: 'cancelled' }))).toEqual({ id: 'call-ask', verdict: 'interrupted', questions: ['Which direction?', 'Which scope?'] });
  // Failures and records that cannot be paired fall back to the generic card.
  expect(askUserRow(settled({ type: 'failed' }))).toBeUndefined();
  expect(askUserRow(settled({ type: 'success' }, { cancelled: false, answers: [{ question_index: 2, kind: 'option', answer: 'X' }] }))).toBeUndefined();
  expect(askUserRow(settled({ type: 'success' }, { cancelled: false, answers: [{ question_index: 0, kind: 'option', answer: 'X' }, { question_index: 0, kind: 'custom', answer: 'Y' }] }))).toBeUndefined();
});

it('the question row summarizes the verdict and expands to the read-only record', () => {
  const row = askUserRow(settled({ type: 'success' }, { cancelled: false, answers: [{ question_index: 0, kind: 'custom', answer: 'Fly a plane' }] }))!;
  render(<QuestionRow row={row}/>);
  expect(screen.getByText('Ask question')).toBeTruthy();
  expect(screen.getByText('1/2 answered')).toBeTruthy();
  expect(screen.queryByText('Fly a plane')).toBeNull();
  fireEvent.click(screen.getByText('1/2 answered'));
  expect(screen.getByText('Which direction?')).toBeTruthy();
  expect(screen.getByText('Fly a plane')).toBeTruthy();
  expect(screen.getByText('Not answered')).toBeTruthy();
  cleanup();
  render(<QuestionRow row={askUserRow(call({ type: 'running', arguments: asked }))!}/>);
  expect(screen.getByText('waiting')).toBeTruthy();
});

it('ask_user interaction audits are told by the call row, while other questionnaires keep their audit', async () => {
  const { AgentTranscript } = await import('../src/app/agent/AgentTranscript');
  const { snapshot } = await import('./fixture');
  const s = snapshot();
  const questionnaire = { questions: [{ header: 'Direction', question: 'Which direction?', answer: { type: 'single_choice' as const, allow_custom: true, options: [{ label: 'Keep', description: 'Keep it.' }, { label: 'Drop', description: 'Drop it.' }] } }] };
  const requested = (interaction_id: string, tool_id: string) => ({ type: 'interaction_requested' as const, event_id: `${interaction_id}-requested`, timestamp: '2026-10-06T00:00:00Z', attempt_id: 'a', turn_id: 't', interaction_id,
    subject: { type: 'questionnaire' as const, invocation_id: { caller: 'agent' as const, call_id: `${interaction_id}-call` }, requester: { tool_id, tool_name: tool_id, origin: 'builtin' as const }, questionnaire } });
  const settled = (interaction_id: string) => ({ type: 'interaction_settled' as const, event_id: `${interaction_id}-settled`, timestamp: '2026-10-06T00:00:01Z', attempt_id: 'a', turn_id: 't', interaction_id, settlement: { type: 'questionnaire_declined' as const } });
  s.transcript = { entries: [requested('ask', 'tool-ask-user'), settled('ask'), requested('elicit', 'mcp-elicitation'), settled('elicit')].map((item, index) => ({ cursor: String(index + 1), item })) } as typeof s.transcript;
  const ui = render(<AgentTranscript snapshot={s}/>);
  expect(ui.getAllByText('Historical interaction details')).toHaveLength(2);
  expect(ui.container.textContent).toContain('elicit-requested');
  expect(ui.container.textContent).not.toContain('ask-requested');
});
