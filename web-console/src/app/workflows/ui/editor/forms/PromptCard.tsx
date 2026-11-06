import type { ReactNode } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
import { IconCheckOutline16, IconCopyOutline16, IconFullscreenOutline16, IconQuestionOutline14, IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { Tooltip } from '../../../../../presentation/primitives/Tooltip';
import { useCopyFeedback } from '../../../../../presentation/primitives/use-copy-feedback';
import { useInspectorExpansion } from './useInspectorExpansion';
import css from '../Editor.module.css';

/** As Dify's prompt editor: a role header with its help, the text's length,
 * the card's own tools, copy, remove and expand, over a borderless body. */
export function PromptCard({ label, role, hint, text, tools, expanded, onExpand, onRemove, removeLabel, children }: {
  label: string; role: ReactNode; hint?: string; text: string; tools?: ReactNode; expanded?: boolean; onExpand: () => void; onRemove?: () => void; removeLabel?: string; children: ReactNode;
}) {
  const tx = useTranslation(), copy = useCopyFeedback(text);
  const expand = useInspectorExpansion({expanded:!!expanded,onChange:onExpand});
  return <div ref={expand.anchor} style={expanded?{height:expand.collapsedHeight}:undefined}>
  <section className={clsx(css.promptCard, expanded && css.promptCardExpanded)} style={expanded?expand.bounds:undefined} data-prompt-expanded={expanded||undefined} aria-label={label}
    onKeyDown={event=>{if(expanded&&event.key==='Escape'&&!event.defaultPrevented&&!event.nativeEvent.isComposing){event.preventDefault();event.stopPropagation();onExpand();}}}>
    <header className={css.promptHead}>
      {role}
      {hint && <Tooltip label={hint} side="bottom" maxWidth={260}><span className={css.promptHint} tabIndex={0} aria-label={hint}><IconQuestionOutline14 /></span></Tooltip>}
      <span className={css.promptTools}>
        <span className={css.promptCount} title={tx('workflows:prompt.length', { p0: text.length })}>{text.length}</span>
        <span className={css.promptDivider} aria-hidden />
        {tools}
        {onRemove && <button type="button" className={css.promptTool} aria-label={removeLabel ?? tx('workflows:prompt.remove-message')} title={removeLabel ?? tx('workflows:prompt.remove-message')} onClick={onRemove}><IconTrashOutline16 /></button>}
        <button type="button" className={css.promptTool} aria-label={tx(copy.copied ? 'workflows:prompt.copied' : 'workflows:prompt.copy')} title={tx(copy.copied ? 'workflows:prompt.copied' : 'workflows:prompt.copy')} disabled={!text} onClick={copy.onCopy}>
          {copy.copied ? <IconCheckOutline16 /> : <IconCopyOutline16 />}
        </button>
        <button type="button" className={css.promptTool} aria-pressed={!!expanded} aria-label={tx(expanded ? 'workflows:prompt.collapse' : 'workflows:prompt.expand')} title={tx(expanded ? 'workflows:prompt.collapse' : 'workflows:prompt.expand')} onClick={expand.toggle}><IconFullscreenOutline16 /></button>
      </span>
    </header>
    <div className={css.promptBody}>{children}</div>
  </section></div>;
}
