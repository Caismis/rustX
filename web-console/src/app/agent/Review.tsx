/* Copyright (c) 2026 DeepSeek. MIT. QuestionComposer takeover adapted; see PROVENANCE.md. */
import { useState } from 'react';
import type { ReviewSpecification } from '../../../../protocol/app-server/v57';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { IconChevronDownOutline14, IconChevronUpOutline14, IconCloseOutline16 } from '../../presentation/primitives/icons';
import { ReviewContent } from '../../presentation/agent/ReviewContent';
import css from '../../presentation/agent/Question.module.css';
import detailCss from '../../presentation/agent/ReviewContent.module.css';

/** DSH question-card interaction, backed by rustX's in-workflow review decision. */
export function Review({ title, review, disabled, submitting, accept, reject, cancel }: {
  title: string; review: ReviewSpecification; disabled: boolean; submitting: boolean; accept: () => void; reject: (feedback: string) => void; cancel: () => void;
}) {
  const tx = useTranslation();
  const [feedback, setFeedback] = useState(''), [decision, setDecision] = useState<'accepted' | 'rejected'>('accepted'), [minimized, setMinimized] = useState(false);
  return <div className={css.frame}><section className={`${css.card} ${minimized ? css.cardMinimized : ''}`} aria-label={tx('interactions:review.title')}>
    <header className={css.header}>
      <div className={css.headingBlock}><div className={css.eyebrow}>{review.instance.block.definition.workflow_id} · {review.instance.node}</div><h2 className={css.title}>{tx('interactions:review.title')}</h2>{minimized && <span className={detailCss.status} role="status">{title}</span>}</div>
      <div className={css.headerActions}><button type="button" className={css.iconButton} aria-expanded={!minimized} aria-label={tx(minimized ? 'interactions:questionnaire.maximize' : 'interactions:questionnaire.minimize')} onClick={() => setMinimized(value => !value)}>{minimized ? <IconChevronUpOutline14/> : <IconChevronDownOutline14/>}</button>
        <button type="button" className={css.iconButton} disabled={disabled} aria-label={tx('interactions:interactions.cancel-interaction')} onClick={cancel}><IconCloseOutline16/></button></div>
    </header>
    {!minimized && <><div className={css.body} data-question-scroll><div className={detailCss.detail}><ReviewContent content={review.subject.content} context={review.context}/></div>
      <div className={css.options} role="radiogroup" aria-label={tx('interactions:review.decision')}>
        {(['accepted', 'rejected'] as const).map((value, index) => <button key={value} type="button" role="radio" aria-label={tx(value === 'accepted' ? 'interactions:interactions.accept-review' : 'interactions:interactions.reject-with-feedback')} aria-checked={decision === value} className={`${css.option} ${decision === value ? css.optionSelected : ''}`} disabled={disabled} onClick={() => setDecision(value)}>
          <span className={css.number}>{index + 1}</span><span className={css.optionCopy}><span className={css.optionLine}><span className={css.optionLabel}>{tx(value === 'accepted' ? 'interactions:interactions.accept-review' : 'interactions:interactions.reject-with-feedback')}</span></span></span>
        </button>)}
      </div>
      {decision === 'rejected' && <div className={detailCss.feedback}><textarea aria-label={tx('interactions:interactions.review-feedback')} placeholder={tx('interactions:interactions.review-feedback')} disabled={disabled} value={feedback} onChange={event => setFeedback(event.target.value)}/></div>}
    </div><footer className={css.footer}><span className={detailCss.status} role="status">{title}</span><div className={css.footerActions}>
      <Button disabled={disabled} variant="outline" onClick={cancel}>{tx('interactions:interactions.cancel-interaction')}</Button>
      <Button disabled={disabled || decision === 'rejected' && !feedback.trim()} variant="primary" onClick={() => decision === 'accepted' ? accept() : reject(feedback)}>{tx(submitting ? 'interactions:questionnaire.submitting' : 'interactions:questionnaire.submit')}</Button>
    </div></footer></>}
  </section></div>;
}
