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
  const questionContent = (question: string) => renderQuestion ? <div className="whitespace-normal">{renderQuestion(question)}</div> : question;
  const summary = row.verdict === 'waiting' ? tx('tools:ask.waiting')
    : row.verdict === 'answered' ? row.total === undefined ? tx('tools:ask.answered-without-total', { answered: row.answered }) : tx('tools:ask.answered', { answered: row.answered, total: row.total })
    : tx(`tools:ask.${row.verdict}`);
  const expandable = row.verdict !== 'waiting' && row.questions.length > 0;
  // Dismissal is the user's own choice; an interrupt keeps the shared stopped semantics.
  const state = row.verdict === 'waiting' ? 'running' : row.verdict === 'interrupted' || row.verdict === 'timed-out' ? 'stopped' : 'ok';
  return <div className="rounded-[var(--dsw-radius-sm)]" data-tool-call-id={row.id} data-tool-name={kind}>
    <div className={css.root} data-state={state} data-tool={kind}>
      <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} chevronClassName={css.chevron}
        icon={<IconQuestionOutline14/>} title={title ?? tx('tools:ask.row-title')} open={open && expandable} expandable={expandable}
        expandOnRowClick={panelAction === undefined} keepContentWhenOpen onToggle={toggle}
        collapsedContent={<><span className={css.sep} aria-hidden/><span className={css.summary}>{summary}</span>
          {panelAction && <button type="button" className="flex-none inline-flex items-center gap-[4px] ml-[8px] px-[10px] py-0 h-[calc(22px+var(--dsh-content-font-delta,0px))] border-none rounded-full bg-[var(--dsw-alias-button-floating-fill)] [color:var(--dsw-alias-label-primary)] [font-family:inherit] text-[calc(var(--dsh-content-font-size-secondary,13px)-1px)] leading-none whitespace-nowrap cursor-pointer transition-colors duration-100 hover:bg-[var(--dsw-alias-button-floating-hover)] [&_svg]:w-[13px] [&_svg]:h-[13px]"
            onClick={event => { event.stopPropagation(); panelAction.open(); }}
            onKeyDown={event => { if (event.key === 'Enter' || event.key === ' ') event.stopPropagation(); }}>
            <IconQuestionOutline14/>{panelAction.label}
          </button>}
        </>}>
        {open && expandable && <div className={css.bodyWrap}>
          {row.verdict === 'answered' ? <dl className="flex flex-col gap-[16px] max-h-[360px] mt-[4px] mr-0 mb-[4px] ml-[4px] px-[20px] py-[16px] overflow-y-auto [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-[16px] bg-[var(--dsw-alias-bg-base)]">
            {row.questions.map((item, index) => <div className="flex flex-col gap-[2px] min-w-0" key={index}>
              <dt className="m-0 whitespace-pre-wrap wrap-anywhere text-[var(--dsh-content-font-size,14px)] leading-[calc(24px+var(--dsh-content-font-delta,0px))] [color:var(--dsw-alias-label-tertiary)]">{questionContent(item.question)}</dt>
              <dd className="m-0 whitespace-pre-wrap wrap-anywhere text-[var(--dsh-content-font-size,14px)] leading-[calc(24px+var(--dsh-content-font-delta,0px))] [color:var(--dsw-alias-label-primary)]">{item.answers.length === 0
                ? <span className="[color:var(--dsw-alias-label-tertiary)]">{tx('tools:ask.skipped')}</span>
                : item.answers.map((answer, line) => <span className="block" key={line}>{answer}</span>)}</dd>
            </div>)}
          </dl> : <div className="flex flex-col gap-[16px] max-h-[360px] mt-[4px] mr-0 mb-[4px] ml-[4px] px-[20px] py-[16px] overflow-y-auto [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-[16px] bg-[var(--dsw-alias-bg-base)]">
            <p className="m-0 [color:var(--dsw-alias-label-primary)] text-[var(--dsh-content-font-size,14px)] leading-[calc(24px+var(--dsh-content-font-delta,0px))]">{tx(`tools:ask.${row.verdict}-detail`)}</p>
            <ul className="flex flex-col gap-[8px] m-0 pl-[20px]">{row.questions.map((question, index) => <li className="[color:var(--dsw-alias-label-tertiary)] whitespace-pre-wrap wrap-anywhere text-[var(--dsh-content-font-size,14px)] leading-[calc(24px+var(--dsh-content-font-delta,0px))]" key={index}>{questionContent(question)}</li>)}</ul>
        </div>}
          {inspect && <ToolInspectionButton inspect={inspect}/>}
        </div>}
      </DisclosureRow>
    </div>
  </div>;
}
