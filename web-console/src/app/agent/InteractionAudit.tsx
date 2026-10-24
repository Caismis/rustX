import { useState } from 'react';
import type { RuntimeClientTranscriptEntry } from '../../../../protocol/app-server/v57';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconQuestionOutline14 } from '../../presentation/primitives/icons';
import { ReviewContent, ReviewValue } from '../../presentation/agent/ReviewContent';
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
  const summary = outcome?.type === 'reviewed' ? tx(outcome.response.decision.type === 'accepted' ? 'interactions:audit.accepted' : 'interactions:audit.rejected')
    : outcome ? tx(`interactions:audit.${outcome.type}`) : tx('interactions:audit.requested');
  return <div data-interaction-audit><DisclosureRow icon={<IconQuestionOutline14/>} title={title} open={open} expandable expandOnRowClick onToggle={() => setOpen(value => !value)} collapsedContent={<span> · {summary}</span>}>
    {subject?.type === 'review' && <ReviewContent content={subject.review.subject.content} context={subject.review.context}/>}
    {subject?.type === 'approval' && <><p>{subject.reason}</p></>}
    {subject?.type === 'questionnaire' && subject.questionnaire.questions.map((question, index) => {
      const answer = outcome?.type === 'questionnaire_submitted' ? outcome.submission.answers.find(item => item.question_index === index)?.answer : undefined;
      const options = question.answer.type === 'single_choice' || question.answer.type === 'multi_choice' ? question.answer.options : undefined;
      const value = answer?.type === 'option' ? options?.[answer.value.option_index]?.label ?? answer.value
        : answer?.type === 'options' ? answer.value.option_indices.map(index => options?.[index]?.label ?? index)
          : answer?.type === 'custom' ? answer.value.answer : answer?.type === 'number' ? finiteNumberFromWire(answer.value.value) : answer?.value.value;
      return <div key={index}><p>{question.question}</p>{answer && <ReviewValue value={value}/>}</div>;
    })}
    {!subject && outcome?.type === 'questionnaire_submitted' && <ReviewValue value={outcome.submission.answers}/>}
    <p>{summary}</p>
    {outcome?.type === 'reviewed' && outcome.response.decision.type === 'rejected' && <ReviewValue value={outcome.response.decision.feedback}/>}
    {outcome?.type === 'denied' && <p>{outcome.reason}</p>}
  </DisclosureRow></div>;
}
