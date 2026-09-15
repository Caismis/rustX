/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Extracted from DeepSeek Harness QuestionComposer: recommendation labels,
// mirror-growing answer field, option rows, page navigation, local drafts.
// All Remote, plan-intent, slot-store, answer-lifetime and label-identity semantics
// replaced by rustX's generated question shapes and index-based submissions.
import { useState } from 'react';
import type { ChangeEvent } from 'react';
import clsx from 'clsx';
import type { QuestionSpecification, QuestionnaireSubmission } from '../../../../protocol/app-server/v4';
import { emptyDraft, submission, type QuestionDraft } from '../../bindings/questionnaire';
import { Button } from '../../presentation/primitives/Button';
import { IconCheckOutline14, IconChevronLeftOutline14, IconChevronRightOutline14 } from '../../presentation/primitives/icons';
import css from './QuestionComposer.module.css';

export function parseRecommendedLabel(label: string) {
  const suffix = /\s*(?:\((?:recommended|推荐)\)|（(?:recommended|推荐)）)\s*$/i;
  return suffix.test(label) ? { label: label.replace(suffix, ''), recommended: true } : { label, recommended: false };
}
function AnswerField({ value, disabled, onChange, label }: {
  value: string; disabled: boolean; onChange: (event: ChangeEvent<HTMLTextAreaElement>) => void; label: string;
}) {
  return <div className={clsx(css.field, css.customBlock)}>
    <div aria-hidden className={css.fieldMirror}>{`${value}\n`}</div>
    <textarea className={css.fieldInput} value={value} disabled={disabled} rows={1} aria-label={label} placeholder={label} onChange={onChange} />
  </div>;
}
export function QuestionComposer({ questions, disabled, status, onSubmit, onDecline }: {
  questions: QuestionSpecification[]; disabled: boolean; status: string;
  onSubmit: (value: QuestionnaireSubmission) => void; onDecline: () => void;
}) {
  const [index, setIndex] = useState(0);
  const [drafts, setDrafts] = useState<QuestionDraft[]>(() => questions.map(emptyDraft));
  const [error, setError] = useState('');
  const question = questions[index];
  if (!question) return <p>Invalid empty Questionnaire from server.</p>;
  const draft = drafts[index];
  const shape = question.answer;
  const hasOptions = shape.type === 'single_choice' || shape.type === 'multi_choice';
  const update = (value: Partial<QuestionDraft>) => {
    setDrafts(current => current.map((item, i) => i === index ? { ...item, ...value } : item)); setError('');
  };
  const choose = (optionIndex: number) => {
    const selected = shape.type === 'multi_choice'
      ? draft.selected.includes(optionIndex) ? draft.selected.filter(item => item !== optionIndex) : [...draft.selected, optionIndex]
      : [optionIndex];
    update({ selected, text: '' });
  };
  const submit = () => {
    try { onSubmit(submission(questions, drafts)); } catch (cause) { setError(String(cause)); }
  };
  return <section className={css.frame} aria-label="Questionnaire"><div className={css.card}>
    <header className={css.header}><div className={css.headingBlock}>
      <div className={css.eyebrow}>{question.header} · {status}</div><h2 className={css.title}>{question.question}</h2>
    </div></header>
    <div className={css.body} data-question-scroll>
      <div className={css.options} role={shape.type === 'single_choice' ? 'radiogroup' : 'group'} aria-label={question.header}>
        {hasOptions && shape.options.map((option, optionIndex) => {
          const selected = draft.selected.includes(optionIndex);
          const display = parseRecommendedLabel(option.label);
          return <div key={optionIndex}>
            <button type="button" className={clsx(css.option, selected && css.optionSelected)}
              role={shape.type === 'multi_choice' ? 'checkbox' : 'radio'} aria-checked={selected}
              aria-label={display.label} disabled={disabled} onClick={() => choose(optionIndex)}>
              {shape.type === 'multi_choice' ? <span className={clsx(css.checkbox, selected && css.checkboxChecked)} aria-hidden>{selected && <IconCheckOutline14 size={12} />}</span>
                : <span className={css.number}>{optionIndex + 1}</span>}
              <span className={css.optionCopy}><span className={css.optionLine}>
                <span className={css.optionLabel}>{display.label}</span>{display.recommended && <span className={css.badge}>Recommended</span>}
                <span className={css.description}>{option.description}</span>
              </span></span>
            </button>
            {option.preview && selected && <pre>{option.preview}</pre>}
          </div>;
        })}
        {shape.type === 'boolean' ? <div className="row">
          <Button variant={draft.boolean === true ? 'primary' : 'outline'} disabled={disabled} onClick={() => update({ boolean: true })}>True</Button>
          <Button variant={draft.boolean === false ? 'primary' : 'outline'} disabled={disabled} onClick={() => update({ boolean: false })}>False</Button>
        </div> : (!hasOptions || shape.allow_custom) && <AnswerField value={draft.text} disabled={disabled}
          label={hasOptions ? 'Custom answer' : `Answer (${shape.type})`} onChange={event => update({ text: event.target.value, selected: [] })} />}
        <small className="muted">{hasOptions ? shape.type === 'multi_choice' ? `Choose ${shape.min_selected}–${shape.max_selected}; custom replaces the selection when allowed.` : 'Choose one option.' : JSON.stringify(shape)}</small>
      </div>
    </div>
    <footer className={css.footer}>
      <div className={css.pager}>
        <button type="button" className={css.iconButton} aria-label="Previous question" disabled={index === 0} onClick={() => setIndex(index - 1)}><IconChevronLeftOutline14 /></button>
        <span className={css.progress}>{index + 1} / {questions.length}</span>
        <button type="button" className={css.iconButton} aria-label="Next question" disabled={index === questions.length - 1} onClick={() => setIndex(index + 1)}><IconChevronRightOutline14 /></button>
      </div>
      <div className={css.feedback} role="status">{error}</div>
      <div className={css.footerActions}>
        <Button variant="outline" disabled={disabled} onClick={onDecline}>Decline</Button>
        <Button variant="primary" disabled={disabled} onClick={submit}>Submit answers</Button>
      </div>
    </footer>
    <p className="muted inset">Unanswered questions are omitted. The server validates and settles the response.</p>
  </div></section>;
}
