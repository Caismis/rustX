import { useState } from 'react';
import type { InteractionSettlement, QuestionnaireAnswerEntry, QuestionSpecification, RuntimeClientTranscriptEntry } from '../../../../protocol/app-server/v63';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconQuestionOutline14 } from '../../presentation/primitives/icons';
import { ReviewValue } from '../../presentation/agent/ReviewContent';
import { finiteNumberFromWire } from '../../../../protocol/app-server/number';
import { QuestionRow, type QuestionRowView } from '../../presentation/agent/QuestionRow';
import type { Translate } from '../../locale/translation';

type Item = RuntimeClientTranscriptEntry['item'];
type Requested = Extract<Item, { type: 'interaction_requested' }>;
type Settled = Extract<Item, { type: 'interaction_settled' }>;
/** Durable request/settlement receipts share only their exact native identity. */
export function InteractionAudit({ item, settlement }: { item: Requested | Settled; settlement?: Settled }) {
  const tx = useTranslation(), [open, setOpen] = useState(false);
  const requested = item.type === 'interaction_requested' ? item : undefined;
  const outcome = item.type === 'interaction_settled' ? item.settlement : settlement?.settlement;
  const subject = requested?.subject;
  const review = subject?.type === 'review' ? subject.review : undefined;
  const row = (review || outcome?.type === 'reviewed') && reviewQuestionRow(review?.questionnaire.questions, outcome, tx);
  if (row) {
    const node = review?.instance.node ?? (outcome?.type === 'reviewed' ? outcome.response.instance.node : '');
    return <div data-interaction-audit data-review-interaction={item.interaction_id}>
      <QuestionRow kind="review" title={`${tx('interactions:review.title')} · ${node}`} row={row} renderQuestion={question => <ReviewValue value={question}/>}/>
    </div>;
  }
  const title = subject?.type === 'approval' ? `${tx('interactions:audit.approval')} · ${subject.tool_name}`
      : subject?.type === 'questionnaire' ? `${tx('interactions:audit.questionnaire')} · ${subject.requester.tool_name}`
          : tx('interactions:audit.interaction');
  const questions = subject?.type === 'questionnaire' ? subject.questionnaire.questions : undefined;
  const answers = outcome?.type === 'questionnaire_submitted' ? outcome.submission.answers : undefined;
  const summary = outcome ? tx(`interactions:audit.${outcome.type}`) : tx('interactions:audit.requested');
  return <div data-interaction-audit><DisclosureRow icon={<IconQuestionOutline14/>} title={title} open={open} expandable expandOnRowClick onToggle={() => setOpen(value => !value)} collapsedContent={<span> · {summary}</span>}>
    {subject?.type === 'approval' && <><p>{subject.reason}</p></>}
    {questions?.map((question, index) => {
      const answer = answers?.find(item => item.question_index === index);
      return <div key={index}><ReviewValue value={question.question}/>{answer && <ReviewValue value={label(questions, answer)}/>}</div>;
    })}
    {!questions && answers && <ReviewValue value={answers}/>}
    <p>{summary}</p>
    {outcome?.type === 'denied' && <p>{outcome.reason}</p>}
  </DisclosureRow></div>;
}
/** Reviews use the same read-only verdict/card as ask_user, from native receipts. */
function reviewQuestionRow(questions: QuestionSpecification[] | undefined, outcome: InteractionSettlement | undefined, tx: Translate): QuestionRowView | undefined {
  if (!outcome) return { verdict: 'waiting' };
  if (outcome.type === 'reviewed') {
    const answers = outcome.response.submission.answers;
    const display = (value: unknown): string => typeof value === 'string' ? value
      : typeof value === 'boolean' ? tx(value ? 'interactions:questionnaire.true' : 'interactions:questionnaire.false') : JSON.stringify(value) ?? '';
    return { verdict: 'answered', answered: answers.length, total: questions?.length,
      questions: questions?.map((question, index) => {
        const value = label(questions, answers.find(answer => answer.question_index === index));
        return { question: question.question, answers: value === undefined ? [] : Array.isArray(value) ? value.map(display) : [display(value)] };
      }) ?? [] };
  }
  if (outcome.type === 'cancelled' || outcome.type === 'deadline_expired') return {
    verdict: outcome.type === 'deadline_expired' ? 'timed-out' : outcome.reason === 'user_requested' ? 'cancelled' : 'interrupted',
    questions: questions?.map(question => question.question) ?? [],
  };
  return undefined;
}
/** An answer as the person saw it: option labels rather than indices. */
function label(questions: QuestionSpecification[] | undefined, entry: QuestionnaireAnswerEntry | undefined): unknown {
  if (!entry) return undefined;
  const { answer } = entry, shape = questions?.[entry.question_index]?.answer;
  const options = shape?.type === 'single_choice' || shape?.type === 'multi_choice' ? shape.options : undefined;
  return answer.type === 'option' ? options?.[answer.value.option_index]?.label ?? answer.value
    : answer.type === 'options' ? answer.value.option_indices.map(index => options?.[index]?.label ?? index)
      : answer.type === 'custom' ? answer.value.answer : answer.type === 'number' ? finiteNumberFromWire(answer.value.value) : answer.value.value;
}
