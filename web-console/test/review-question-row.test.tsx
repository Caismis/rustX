import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { InteractionSettlement, RuntimeClientTranscriptEntry } from '../../protocol/app-server/v65';
import { InteractionAudit } from '../src/app/agent/InteractionAudit';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { workflowReview } from './workflow-data';
import { snapshot } from './fixture';

afterEach(cleanup);
type Requested = Extract<RuntimeClientTranscriptEntry['item'], { type: 'interaction_requested' }>;
const request = (id = 'review-1'): Requested => ({ type: 'interaction_requested', event_id: `${id}-request`, timestamp: '2026-11-03T06:43:00Z',
  attempt_id: 'review-attempt', turn_id: 'review-turn', interaction_id: id, subject: { type: 'review', review: workflowReview() } });
const settle = (settlement: InteractionSettlement, id = 'review-1') => ({ type: 'interaction_settled' as const, event_id: `${id}-settle`,
  timestamp: '2026-11-03T06:44:00Z', attempt_id: 'review-attempt', turn_id: 'review-turn', interaction_id: id, settlement });
const reviewed = (): InteractionSettlement => ({ type: 'reviewed', response: { instance: workflowReview().instance, subject_digest: 'native-review-digest',
  submission: { answers: [{ question_index: 0, answer: { type: 'option', value: { option_index: 1 } } }] } } });

it('keeps pending review content on the questionnaire surface rather than duplicating it in the waiting row', () => {
  const ui = render(<InteractionAudit item={request()}/>);
  expect(ui.getByText('waiting')).toBeTruthy();
  expect(ui.queryByText('Review report')).toBeNull();
  expect(ui.container.querySelectorAll('[data-tool="review"]')).toHaveLength(1);
  expect(ui.container.querySelector('[data-tool-call-id]')).toBeNull();
  expect(ui.container.querySelector('[aria-expanded]')).toBeNull();
});

it.each([
  [{ type: 'cancelled', reason: 'user_requested' }, 'cancelled'],
  [{ type: 'cancelled', reason: 'parent_cancelled' }, 'interrupted'],
  [{ type: 'deadline_expired', kind: 'hard' }, 'timed out'],
] as const)('uses the question verdict card for native %s', (outcome, verdict) => {
  const ui = render(<InteractionAudit item={request()} settlement={settle(outcome)}/>);
  expect(ui.getByText(verdict)).toBeTruthy(); expect(ui.queryByText('Review report')).toBeNull();
  fireEvent.click(ui.getByRole('button', { name: /Human Input · gate/ }));
  expect(ui.getByRole('heading', { name: 'Review report' })).toBeTruthy();
  expect(ui.queryByText('Revise')).toBeNull();
  expect(ui.container.querySelector('[data-tool="review"] ul')).not.toBeNull();
});

it('a bounded settlement-only read shows the submitted count without invented questions or option labels', () => {
  const ui = render(<InteractionAudit item={settle(reviewed())}/>);
  expect(ui.getByText('1 answered')).toBeTruthy();
  expect(ui.container.textContent).toContain('Human Input · gate');
  expect(ui.container.textContent).not.toMatch(/option_index|subject_digest|undefined|Review report|Revise/);
  expect(ui.container.querySelector('[aria-expanded]')).toBeNull();
});

it('pairs multi-choice answers by native question index and marks omitted questions as unanswered', () => {
  const item = request();
  if (item.subject.type !== 'review') throw new Error('Expected native review request');
  item.subject.review.questionnaire.questions.push({ header: 'checks', question: 'Which checks?', answer: {
    type: 'multi_choice', allow_custom: false, min_selected: 1, max_selected: 2,
    options: [{ label: 'Unit', description: '' }, { label: 'Browser', description: '' }],
  } });
  const outcome = reviewed();
  if (outcome.type !== 'reviewed') throw new Error('Expected native review settlement');
  outcome.response.submission.answers = [{ question_index: 1, answer: { type: 'options', value: { option_indices: [1, 0] } } }];
  const ui = render(<InteractionAudit item={item} settlement={settle(outcome)}/>);
  expect(ui.getByText('1/2 answered')).toBeTruthy();
  fireEvent.click(ui.getByRole('button', { name: /Human Input · gate/ }));
  expect(ui.getAllByText('Not answered')).toHaveLength(1);
  expect(ui.getAllByText('Unit')).toHaveLength(1); expect(ui.getAllByText('Browser')).toHaveLength(1);
  expect(ui.queryByText('Revise')).toBeNull();
  expect(ui.container.textContent).not.toMatch(/option_indices|question_index/);
});

it('keeps two visits of the same node separate and pairs only their exact interaction receipts', () => {
  const current = snapshot();
  const first = reviewed(), second: InteractionSettlement = { type: 'cancelled', reason: 'user_requested' };
  current.transcript.entries = [request(), settle(first), request('review-2'), settle(second, 'review-2')].map((item, i) => ({ cursor: String(i + 1), item }));
  const ui = render(<AgentTranscript snapshot={current}/>);
  expect(ui.container.querySelectorAll('[data-review-interaction]')).toHaveLength(2);
  expect(ui.getByText('1/1 answered')).toBeTruthy(); expect(ui.getByText('cancelled')).toBeTruthy();
  for (const row of ui.container.querySelectorAll('[data-review-interaction]')) fireEvent.click(row.querySelector('[data-disclosure-row]')!);
  expect(ui.getAllByText('Revise')).toHaveLength(1);
  expect(ui.getAllByRole('heading', { name: 'Review report' })).toHaveLength(2);
});
