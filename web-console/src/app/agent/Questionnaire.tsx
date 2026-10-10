import { useTranslation, useNotice } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Extracted from DeepSeek Harness QuestionComposer QuestionFlow: recommendation
// labels and implicit first-choice draft, mirror-growing answer field, option
// rows with an always-visible custom row, single-select advance, skip/next/submit
// footer, minimize and dismiss header actions, local drafts.
// All Remote, plan-intent, slot-store, countdown, answer-lifetime and
// label-identity semantics replaced by rustX's generated question shapes and
// index-based submissions; dismissing the set is the native decline.
import { useState } from 'react';
import type { ChangeEvent, KeyboardEvent } from 'react';
import clsx from 'clsx';
import type { QuestionSpecification, QuestionnaireSubmission } from '../../../../protocol/app-server/v42';
import { emptyDraft, submission, QuestionnaireValidationError, type QuestionDraft } from '../../bindings/questionnaire';
import { message } from '../../locale/translation';
import { Button } from '../../presentation/primitives/Button';
import { IconCheckOutline14, IconChevronDownOutline14, IconChevronLeftOutline14, IconChevronRightOutline14, IconChevronUpOutline14, IconCloseOutline16, IconEditOutline16 } from '../../presentation/primitives/icons';
import css from '../../presentation/agent/Question.module.css';

export function parseRecommendedLabel(label: string) {
  const suffix = /\s*(?:\((?:recommended|推荐)\)|（(?:recommended|推荐)）)\s*$/i;
  return suffix.test(label) ? { label: label.replace(suffix, ''), recommended: true } : { label, recommended: false };
}
/** Only a marked first choice is an implicit draft; the user still submits it. */
function initialDraft(question: QuestionSpecification): QuestionDraft {
  const shape = question.answer;
  const first = shape.type === 'single_choice' || shape.type === 'multi_choice' ? shape.options[0] : undefined;
  return first && parseRecommendedLabel(first.label).recommended ? { ...emptyDraft(), selected: [0] } : emptyDraft();
}
const answered = (draft: QuestionDraft) => draft.selected.length > 0 || draft.text.trim() !== '' || draft.boolean !== undefined;
/** Return whether a text-field key event belongs to an active IME composition. */
function isComposing(event: KeyboardEvent<HTMLTextAreaElement>) {
  // keyCode 229 is the legacy IME-composition signal engines emit without isComposing.
  return event.nativeEvent.isComposing || Reflect.get(event.nativeEvent, 'keyCode') === 229;
}
function AnswerField({ variant, value, disabled, autoFocus, onChange, onKeyDown, placeholder }: {
  variant: 'inline' | 'block'; value: string; disabled: boolean; autoFocus?: boolean; placeholder: string;
  onChange: (event: ChangeEvent<HTMLTextAreaElement>) => void; onKeyDown: (event: KeyboardEvent<HTMLTextAreaElement>) => void;
}) {
  // The mirror is an invisible layout ruler, not copy: the trailing newline
  // sizes an empty final line, so it never passes through the locale.
  return <div className={clsx(css.field, variant === 'inline' ? css.customInline : css.customBlock)}>
    <div aria-hidden className={css.fieldMirror}>{`${value}\n`}</div>
    <textarea className={css.fieldInput} value={value} disabled={disabled} rows={1} autoFocus={autoFocus} aria-label={placeholder} placeholder={placeholder} onChange={onChange} onKeyDown={onKeyDown} />
  </div>;
}
/** Composer takeover for one pending native questionnaire. `status` is shown
 * only when the response lifecycle has something to say beyond waiting. */
export function Questionnaire({ questions, disabled, status, submitting = false, onSubmit, onDecline }: {
  questions: QuestionSpecification[]; disabled: boolean; status?: string; submitting?: boolean;
  onSubmit: (value: QuestionnaireSubmission) => void; onDecline: () => void;
}) {
  const tx = useTranslation();
  const [index, setIndex] = useState(0);
  const [drafts, setDrafts] = useState<QuestionDraft[]>(() => questions.map(initialDraft));
  // Collapsed to the header strip so the conversation above stays readable while the user decides.
  const [minimized, setMinimized] = useState(false);
  const [error, setError] = useNotice();
  const question = questions[index];
  if (!question) return <p>{tx('interactions:questionnaire.invalid-empty-questionnaire-from-server')}</p>;
  const draft = drafts[index];
  const shape = question.answer;
  const hasOptions = shape.type === 'single_choice' || shape.type === 'multi_choice';
  const multi = shape.type === 'multi_choice';
  const last = index === questions.length - 1;
  const titleId = `questionnaire-title-${index}`;
  const move = (next: number) => { setIndex(next); setError(''); };
  const updateDraft = (update: (current: QuestionDraft) => QuestionDraft, next = index) => {
    const values = drafts.map((item, i) => i === index ? update(item) : item);
    setDrafts(values); setIndex(next); setError('');
    return values;
  };
  const submitDrafts = (values: QuestionDraft[]) => {
    const missing = values.findIndex(item => !answered(item) && !item.skipped);
    if (missing >= 0) { setIndex(missing); setError(message('interactions:questionnaire.incomplete')); return; }
    try { onSubmit(submission(tx, questions, values)); } catch (cause) { setError(cause instanceof QuestionnaireValidationError ? cause.notice : String(cause)); }
  };
  // The native custom answer replaces any selection, so choosing and typing are exclusive.
  const choose = (optionIndex: number) => updateDraft(current => ({
    selected: multi ? current.selected.includes(optionIndex) ? current.selected.filter(item => item !== optionIndex) : [...current.selected, optionIndex] : [optionIndex],
    text: '',
  }), !multi && !last ? index + 1 : index);
  const draftText = (event: ChangeEvent<HTMLTextAreaElement>) => updateDraft(() => ({ selected: [], text: event.target.value }));
  const continueFlow = () => {
    if (!answered(draft)) { setError(message('interactions:questionnaire.unanswered')); return; }
    if (!last) { move(index + 1); return; }
    submitDrafts(drafts);
  };
  // Enter continues the flow, Shift+Enter breaks a line.
  const continueFromText = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== 'Enter' || event.shiftKey || isComposing(event)) return;
    event.preventDefault(); continueFlow();
  };
  const skip = () => {
    const values = updateDraft(() => ({ ...emptyDraft(), skipped: true }), last ? index : index + 1);
    if (last) submitDrafts(values);
  };
  return <div className={css.frame}>
    <section className={clsx(css.card, minimized && css.cardMinimized)} aria-label={tx('interactions:questionnaire.questionnaire')}>
      <header className={css.header}>
        <div className={css.headingBlock}>
          <div className={css.eyebrow}>{question.header}</div>
          <h2 className={css.title} id={titleId}>{question.question}</h2>
        </div>
        <div className={css.headerActions}>
          {status && <span className={css.waitStatus}>{status}</span>}
          <button type="button" className={css.iconButton} aria-expanded={!minimized}
            aria-label={tx(minimized ? 'interactions:questionnaire.maximize' : 'interactions:questionnaire.minimize')}
            title={tx(minimized ? 'interactions:questionnaire.maximize' : 'interactions:questionnaire.minimize')}
            onClick={() => setMinimized(current => !current)}>
            {minimized ? <IconChevronUpOutline14/> : <IconChevronDownOutline14/>}
          </button>
          <button type="button" className={css.iconButton} disabled={disabled}
            aria-label={tx('interactions:questionnaire.dismiss')} title={tx('interactions:questionnaire.dismiss')} onClick={onDecline}>
            <IconCloseOutline16/>
          </button>
        </div>
      </header>
      {!minimized && <>
        <div className={css.body} data-question-scroll>
          <div className={css.options} role={shape.type === 'single_choice' ? 'radiogroup' : 'group'} aria-labelledby={titleId}>
            {hasOptions && shape.options.map((option, optionIndex) => {
              const selected = draft.selected.includes(optionIndex);
              const display = parseRecommendedLabel(option.label);
              return <div key={optionIndex}>
                <button type="button" className={clsx(css.option, selected && !multi && css.optionSelected)}
                  role={multi ? 'checkbox' : 'radio'} aria-checked={selected}
                  aria-label={display.label} disabled={disabled} onClick={() => choose(optionIndex)}>
                  {multi ? <span className={clsx(css.checkbox, selected && css.checkboxChecked)} aria-hidden>{selected && <IconCheckOutline14 size={12} />}</span>
                    : <span className={css.number}>{optionIndex + 1}</span>}
                  <span className={css.optionCopy}><span className={css.optionLine}>
                    <span className={css.optionLabel}>{display.label}</span>{display.recommended && <span className={css.badge}>{tx('interactions:questionnaire.recommended')}</span>}
                    <span className={css.description}>{option.description}</span>
                  </span></span>
                </button>
                {option.preview && selected && <pre>{option.preview}</pre>}
              </div>;
            })}
            {shape.type === 'boolean' ? <div className="row">
              <Button variant={draft.boolean === true ? 'primary' : 'outline'} disabled={disabled} onClick={() => updateDraft(() => ({ ...emptyDraft(), boolean: true }))}>{tx('interactions:questionnaire.true')}</Button>
              <Button variant={draft.boolean === false ? 'primary' : 'outline'} disabled={disabled} onClick={() => updateDraft(() => ({ ...emptyDraft(), boolean: false }))}>{tx('interactions:questionnaire.false')}</Button>
            </div> : hasOptions ? shape.allow_custom && <div className={clsx(css.customRow, draft.text !== '' && css.customRowActive)}>
              {multi ? <span className={clsx(css.checkbox, draft.text !== '' && css.checkboxChecked)} aria-hidden>{draft.text !== '' && <IconCheckOutline14 size={12} />}</span>
                : <span className={css.number} aria-hidden><IconEditOutline16 size={12}/></span>}
              <AnswerField variant="inline" value={draft.text} disabled={disabled} placeholder={tx('interactions:questionnaire.placeholder')} onChange={draftText} onKeyDown={continueFromText}/>
            </div> : <AnswerField key={index} variant="block" autoFocus={!disabled} value={draft.text} disabled={disabled} placeholder={tx('interactions:questionnaire.placeholder')} onChange={draftText} onKeyDown={continueFromText}/>}
          </div>
        </div>
        <footer className={css.footer}>
          <div className={css.pager}>
            <button type="button" className={css.iconButton} aria-label={tx('interactions:questionnaire.previous-question')} disabled={index === 0} onClick={() => move(index - 1)}><IconChevronLeftOutline14 /></button>
            <span className={css.progress}>{index + 1} / {questions.length}</span>
            <button type="button" className={css.iconButton} aria-label={tx('interactions:questionnaire.next-question')} disabled={last} onClick={() => move(index + 1)}><IconChevronRightOutline14 /></button>
          </div>
          <div className={css.feedback} role="status">{error}</div>
          <div className={css.footerActions}>
            <Button variant="outline" disabled={disabled} onClick={skip}>{tx('interactions:questionnaire.skip')}</Button>
            <Button variant="primary" disabled={disabled || !answered(draft)} onClick={continueFlow}>
              {submitting ? tx('interactions:questionnaire.submitting') : last ? tx('interactions:questionnaire.submit') : tx('interactions:questionnaire.next')}
            </Button>
          </div>
        </footer>
      </>}
    </section>
  </div>;
}
