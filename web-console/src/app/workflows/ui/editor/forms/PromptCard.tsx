import type { ReactNode } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
import { IconCheckOutline16, IconCopyOutline16, IconFullscreenOutline16, IconQuestionOutline14 } from '../../../../../presentation/primitives/icons';
import { Tooltip } from '../../../../../presentation/primitives/Tooltip';
import { useCopyFeedback } from '../../../../../presentation/primitives/use-copy-feedback';
import css from '../Editor.module.css';

/** As Dify's prompt editor: a role header with its help, the text's length,
 * copy and expand, over a borderless body. */
export function PromptCard({ role, hint, text, expanded, onExpand, children }: {
  role: string; hint: string; text?: string; expanded?: boolean; onExpand?: () => void; children: ReactNode;
}) {
  const tx = useTranslation(), copy = useCopyFeedback(text ?? '');
  return <section className={clsx(css.promptCard, expanded && css.promptCardExpanded)} aria-label={role}>
    <header className={css.promptHead}>
      <b>{role}</b>
      <Tooltip label={hint} side="bottom" maxWidth={260}><span className={css.promptHint} tabIndex={0} aria-label={hint}><IconQuestionOutline14 /></span></Tooltip>
      <span className={css.promptTools}>
        {text !== undefined && <>
          <span className={css.promptCount} title={tx('workflows:prompt.length', { p0: text.length })}>{text.length}</span>
          <span className={css.promptDivider} aria-hidden />
          <button type="button" className={css.promptTool} aria-label={tx(copy.copied ? 'workflows:prompt.copied' : 'workflows:prompt.copy')} title={tx(copy.copied ? 'workflows:prompt.copied' : 'workflows:prompt.copy')} disabled={!text} onClick={copy.onCopy}>
            {copy.copied ? <IconCheckOutline16 /> : <IconCopyOutline16 />}
          </button>
        </>}
        {onExpand && <button type="button" className={css.promptTool} aria-pressed={!!expanded} aria-label={tx(expanded ? 'workflows:prompt.collapse' : 'workflows:prompt.expand')} title={tx(expanded ? 'workflows:prompt.collapse' : 'workflows:prompt.expand')} onClick={onExpand}><IconFullscreenOutline16 /></button>}
      </span>
    </header>
    <div className={css.promptBody}>{children}</div>
  </section>;
}
