import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-tool AskQuestionRow, QuestionToolRow and
// AskQuestionCard: one row whose summary is the interaction verdict and whose
// expansion is the read-only question/answer record. Harness's timed-question
// panel actions and the trajectory Inspect pill have no rustX counterpart.
import { useState } from 'react';
import { DisclosureRow } from '../primitives/DisclosureRow';
import { IconQuestionOutline14 } from '../primitives/icons';
import css from './Tool.module.css';
import tree from './ToolTree.module.css';
import card from './AskQuestionCard.module.css';

/** One question call's verdict and, once settled, its read-only record. */
export type QuestionRowView =
  | { readonly id: string; readonly verdict: 'waiting' }
  | { readonly id: string; readonly verdict: 'answered'; readonly answered: number; readonly total: number; readonly questions: readonly { readonly question: string; readonly answers: readonly string[] }[] }
  | { readonly id: string; readonly verdict: 'cancelled' | 'interrupted'; readonly questions: readonly string[] };

export function QuestionRow({ row }: { row: QuestionRowView }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const summary = row.verdict === 'waiting' ? tx('tools:ask.waiting')
    : row.verdict === 'answered' ? tx('tools:ask.answered', { answered: row.answered, total: row.total })
    : tx(`tools:ask.${row.verdict}`);
  const expandable = row.verdict !== 'waiting';
  // Dismissal is the user's own choice; an interrupt keeps the shared stopped semantics.
  const state = row.verdict === 'waiting' ? 'running' : row.verdict === 'interrupted' ? 'stopped' : 'ok';
  return <div className={tree.callRow} data-tool-call-id={row.id} data-tool-name="ask_user">
    <div className={css.root} data-state={state} data-tool="ask_user">
      <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} chevronClassName={css.chevron}
        icon={<IconQuestionOutline14/>} title={tx('tools:ask.row-title')} open={open && expandable} expandable={expandable}
        expandOnRowClick keepContentWhenOpen onToggle={() => setOpen(value => !value)}
        collapsedContent={<><span className={css.sep} aria-hidden/><span className={css.summary}>{summary}</span></>}>
        {open && expandable && <div className={css.bodyWrap}>
          {row.verdict === 'answered' ? <dl className={card.card}>
            {row.questions.map((item, index) => <div className={card.item} key={index}>
              <dt className={card.question}>{item.question}</dt>
              <dd className={card.answer}>{item.answers.length === 0
                ? <span className={card.skipped}>{tx('tools:ask.skipped')}</span>
                : item.answers.map((answer, line) => <span className={card.answerLine} key={line}>{answer}</span>)}</dd>
            </div>)}
          </dl> : <div className={card.card}>
            <p className={card.verdict}>{tx(`tools:ask.${row.verdict}-detail`)}</p>
            <ul className={card.questionList}>{row.questions.map((question, index) => <li className={card.unansweredQuestion} key={index}>{question}</li>)}</ul>
          </div>}
        </div>}
      </DisclosureRow>
    </div>
  </div>;
}
