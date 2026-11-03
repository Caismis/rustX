import { ToolInspectionButton } from './ToolCard';
import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-tool AskQuestionRow, QuestionToolRow and
// AskQuestionCard: one row whose summary is the interaction verdict and whose
// expansion is the read-only question/answer record. Panel actions are supplied
// by the native conversation composition; Trace navigation owns the Inspect pill.
import { useState, type ReactNode } from 'react';
import { DisclosureRow } from '../primitives/DisclosureRow';
import { IconQuestionOutline14 } from '../primitives/icons';
import css from './Tool.module.css';
import tree from './ToolTree.module.css';
import card from './AskQuestionCard.module.css';

/** One question call's verdict and, once settled, its read-only record. */
export type QuestionRowView =
  | { readonly id?: string; readonly verdict: 'waiting' }
  | { readonly id?: string; readonly verdict: 'answered'; readonly answered: number; readonly total?: number; readonly questions: readonly { readonly question: string; readonly answers: readonly string[] }[] }
  | { readonly id?: string; readonly verdict: 'cancelled' | 'interrupted' | 'timed-out'; readonly questions: readonly string[] };

export function QuestionRow({ row, inspect, title, kind = 'ask_user', renderQuestion, panelAction, disclosure }: {
  row: QuestionRowView; inspect?: () => Promise<void>; title?: string; kind?: 'ask_user' | 'review'; renderQuestion?: (question: string) => ReactNode;
  panelAction?: { label: string; open: () => void }; disclosure?: { open: boolean; toggle: () => void };
}) {
  const tx = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const open = disclosure?.open ?? expanded;
  const toggle = disclosure?.toggle ?? (() => setExpanded(value => !value));
  const questionContent = (question: string) => renderQuestion ? <div className={card.richQuestion}>{renderQuestion(question)}</div> : question;
  const summary = row.verdict === 'waiting' ? tx('tools:ask.waiting')
    : row.verdict === 'answered' ? row.total === undefined ? tx('tools:ask.answered-without-total', { answered: row.answered }) : tx('tools:ask.answered', { answered: row.answered, total: row.total })
    : tx(`tools:ask.${row.verdict}`);
  const expandable = row.verdict !== 'waiting' && row.questions.length > 0;
  // Dismissal is the user's own choice; an interrupt keeps the shared stopped semantics.
  const state = row.verdict === 'waiting' ? 'running' : row.verdict === 'interrupted' || row.verdict === 'timed-out' ? 'stopped' : 'ok';
  return <div className={tree.callRow} data-tool-call-id={row.id} data-tool-name={kind}>
    <div className={css.root} data-state={state} data-tool={kind}>
      <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} chevronClassName={css.chevron}
        icon={<IconQuestionOutline14/>} title={title ?? tx('tools:ask.row-title')} open={open && expandable} expandable={expandable}
        expandOnRowClick={panelAction === undefined} keepContentWhenOpen onToggle={toggle}
        collapsedContent={<><span className={css.sep} aria-hidden/><span className={css.summary}>{summary}</span>
          {panelAction && <button type="button" className={card.panelButton}
            onClick={event => { event.stopPropagation(); panelAction.open(); }}
            onKeyDown={event => { if (event.key === 'Enter' || event.key === ' ') event.stopPropagation(); }}>
            <IconQuestionOutline14/>{panelAction.label}
          </button>}
        </>}>
        {open && expandable && <div className={css.bodyWrap}>
          {row.verdict === 'answered' ? <dl className={card.card}>
            {row.questions.map((item, index) => <div className={card.item} key={index}>
              <dt className={card.question}>{questionContent(item.question)}</dt>
              <dd className={card.answer}>{item.answers.length === 0
                ? <span className={card.skipped}>{tx('tools:ask.skipped')}</span>
                : item.answers.map((answer, line) => <span className={card.answerLine} key={line}>{answer}</span>)}</dd>
            </div>)}
          </dl> : <div className={card.card}>
            <p className={card.verdict}>{tx(`tools:ask.${row.verdict}-detail`)}</p>
            <ul className={card.questionList}>{row.questions.map((question, index) => <li className={card.unansweredQuestion} key={index}>{questionContent(question)}</li>)}</ul>
        </div>}
          {inspect && <ToolInspectionButton inspect={inspect}/>}
        </div>}
      </DisclosureRow>
    </div>
  </div>;
}
