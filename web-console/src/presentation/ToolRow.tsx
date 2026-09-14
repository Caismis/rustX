// Generic disclosure and IN/OUT card extracted from DeepSeek Harness ui-tool/ToolRow.
// Tool lifecycle labels are supplied verbatim from rustX; file/Remote slots removed.
import { useState } from 'react';
import { DisclosureRow } from './primitives/DisclosureRow';
import { StateDot } from './primitives/StateDot';
import css from './ToolRow.module.css';
export function ToolRow({ title, summary, input, output, running = false }: {
  title: string; summary: string; input?: string; output?: string; running?: boolean;
}) {
  const [expanded, setExpanded] = useState(false);
  const expandable = input !== undefined || output !== undefined;
  return <div className={css.root} data-state={running ? 'running' : undefined}>
    <DisclosureRow rowClassName={css.row} leadingClassName={css.leading} titleClassName={css.title} chevronClassName={css.chevron}
      icon={<StateDot state={running ? 'ongoing' : 'idle'} />} title={title} open={expanded} expandable={expandable}
      expandOnRowClick keepContentWhenOpen onToggle={() => setExpanded(value => !value)}
      collapsedContent={<><span className={css.sep} aria-hidden /><span className={css.summary}>{summary}</span></>}>
      <div className={css.bodyWrap}><div className={css.ioCard}>
        {input !== undefined && <div className={css.ioSection}><span className={css.ioLabel}>IN</span><pre className={css.ioText}>{input}</pre></div>}
        {input !== undefined && output !== undefined && <span className={css.ioDivider} aria-hidden />}
        {output !== undefined && <div className={css.ioSection}><span className={css.ioLabel}>OUT</span><pre className={css.ioText}>{output}</pre></div>}
      </div></div>
    </DisclosureRow>
  </div>;
}
