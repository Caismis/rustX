import type { ReviewSpecification, RuntimeClientSnapshot, RuntimeClientTranscriptEntry } from '../../../protocol/app-server/v69';
import type { Server } from '../fixture';
import { workflowRunsFixture } from './workflow-runs';

/** Two visits of the same review node: one answered, the other cancelled. */
export function workflowReviewHistoryFixture(server: Server, current: RuntimeClientSnapshot) {
  workflowRunsFixture(server, current);
  const [user, calls, answer] = current.transcript.entries!, reviews: RuntimeClientTranscriptEntry[] = [];
  current.workflows!.runs.forEach((run, index) => {
    run.instances[2].node = 'review'; run.instances[2].kind = 'review';
    const review: ReviewSpecification = { instance: { block: run.instances[2].block, node: 'review', visit: 1 }, questionnaire: { questions: [{
      header: 'review', question: '## 审核内容\n\n大狗到底叫不叫！',
      answer: { type: 'single_choice', allow_custom: false, options: [{ label: '叫！！！', description: '' }, { label: '不叫', description: '' }] },
    }] } };
    const common = { attempt_id: 'workflow-attempt', turn_id: 'workflow-turn', interaction_id: `workflow-review-${index}` };
    reviews.push({ cursor: String(reviews.length + 3), item: { ...common, type: 'interaction_requested', event_id: `review-request-${index}`, timestamp: '2026-11-03T06:43:00Z', subject: { type: 'review', review } } });
    reviews.push({ cursor: String(reviews.length + 3), item: { ...common, type: 'interaction_settled', event_id: `review-settle-${index}`, timestamp: '2026-11-03T06:43:01Z', settlement: index
      ? { type: 'cancelled', reason: 'user_requested' }
      : { type: 'reviewed', response: { instance: review.instance, subject_digest: 'native-review-digest', submission: { answers: [{ question_index: 0, answer: { type: 'option', value: { option_index: 0 } } }] } } } } });
  });
  current.transcript.entries = [user, calls, ...reviews, { ...answer, cursor: '7' }];
  server.handlers.set('session/turns', () => ({ type: 'conversation_turns', page: {
    cut: { conversation_id: current.conversation_id, journal: '7', transcript: '7', mutation_revision: '0', ledger_through: '7', surface_revision: '1', transcript_total: '7', pending_through: '0', pending_total: '0' },
    offset: 0, total: '1', turns: [{ id: { conversation_id: current.conversation_id, attempt_id: 'workflow-attempt', attempt_id_source: null }, ordinal: 1, cursor: '2', prompt: '运行两次工作流', response: '两次调用的结果' }],
  } }));
}
