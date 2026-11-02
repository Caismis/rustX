import { useState } from 'react';
import type { QuestionnaireAnswerEntry, QuestionSpecification, RuntimeClientTranscriptEntry } from '../../../../protocol/app-server/v62';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconQuestionOutline14 } from '../../presentation/primitives/icons';
import { ReviewValue } from '../../presentation/agent/ReviewContent';
import { finiteNumberFromWire } from '../../../../protocol/app-server/number';

type Item = RuntimeClientTranscriptEntry['item'];
type Requested = Extract<Item, { type: 'interaction_requested' }>;
type Settled = Extract<Item, { type: 'interaction_settled' }>;
/** Durable request/settlement receipts share only their exact native identity. */
export function InteractionAudit({ item, settlement }: { item: Requested | Settled; settlement?: Settled }) {
  const tx = useTranslation(), [open, setOpen] = useState(false);
  const requested = item.type === 'interaction_requested' ? item : undefined;
  const outcome = item.type === 'interaction_settled' ? item.settlement : settlement?.settlement;
  const subject = requested?.subject;
  const title = subject?.type === 'review' ? `${tx('interactions:review.title')} · ${subject.review.instance.node}`
    : subject?.type === 'approval' ? `${tx('interactions:audit.approval')} · ${subject.tool_name}`
      : subject?.type === 'questionnaire' ? `${tx('interactions:audit.questionnaire')} · ${subject.requester.tool_name}`
        : outcome?.type === 'reviewed' ? `${tx('interactions:review.title')} · ${outcome.response.instance.node}`
          : tx('interactions:audit.interaction');
  // A Human Input's answer is the label of the action chosen, when its question is at hand.
  const questions = subject?.type === 'review' ? subject.review.questionnaire.questions : subject?.type === 'questionnaire' ? subject.questionnaire.questions : undefined;
  const answers = outcome?.type === 'reviewed' ? outcome.response.submission.answers : outcome?.type === 'questionnaire_submitted' ? outcome.submission.answers : undefined;
  const action = outcome?.type === 'reviewed' ? label(questions, answers?.[0]) : undefined;
  const summary = typeof action === 'string' ? action : outcome ? tx(`interactions:audit.${outcome.type}`) : tx('interactions:audit.requested');
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
/** An answer as the person saw it: option labels rather than indices. */
function label(questions: QuestionSpecification[] | undefined, entry: QuestionnaireAnswerEntry | undefined): unknown {
  if (!entry) return undefined;
  const { answer } = entry, shape = questions?.[entry.question_index]?.answer;
  const options = shape?.type === 'single_choice' || shape?.type === 'multi_choice' ? shape.options : undefined;
  return answer.type === 'option' ? options?.[answer.value.option_index]?.label ?? answer.value
    : answer.type === 'options' ? answer.value.option_indices.map(index => options?.[index]?.label ?? index)
      : answer.type === 'custom' ? answer.value.answer : answer.type === 'number' ? finiteNumberFromWire(answer.value.value) : answer.value.value;
}
