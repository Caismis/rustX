/* Copyright (c) 2026 DeepSeek. MIT. Tool-change presentation adapted from Harness ContextInjectionRow; see PROVENANCE.md. */
import { useState } from 'react';
import type { BoundedNames, ConfigurationChange } from '../../../../protocol/app-server/v80';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconBrowseOutline16 } from '../../presentation/primitives/icons';

/** Whether a committed configuration change has a Chat row. Chat shows only
 * Tool additions and removals; instruction, Skill, policy and same-name Tool
 * definition changes stay in Trace and inspection, as in Harness. */
export function hasConfigurationChangeRow(change: ConfigurationChange): boolean {
  return change.tools.added.total > 0 || change.tools.removed.total > 0;
}

/** One committed Session configuration change, rendered with the Tool-change
 * disclosure chrome. A single addition or removal whose name is retained is one
 * compact row with no pointless expansion; otherwise the counts show while
 * collapsed and any retained names, grouped by addition and removal, when
 * expanded. A name the native byte bound omitted is never filled in here. */
export function ConfigurationChangeRow({ change }: { change: ConfigurationChange }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const { added, removed } = change.tools;
  const only = added.total + removed.total === 1 ? (added.total ? added : removed).names[0] : undefined;
  const single = only !== undefined;
  const expandable = !single && added.names.length + removed.names.length > 0;
  if (!hasConfigurationChangeRow(change)) return null;
  const names = (list: BoundedNames) => list.total > list.names.length
    ? tx('agent:tools.more', { names: list.names.join(', '), count: list.total - list.names.length })
    : list.names.join(', ');
  const title = single
    ? tx(added.total ? 'agent:tools.added-one' : 'agent:tools.removed-one', { name: only })
    : tx('agent:tools.updated');
  const summary = single ? null
    : added.total && removed.total ? tx('agent:tools.changed-counts', { added: added.total, removed: removed.total })
      : added.total ? tx('agent:tools.added-count', { count: added.total })
        : tx('agent:tools.removed-count', { count: removed.total });
  return <div data-configuration-change data-binding-revision={change.binding_revision}>
    <DisclosureRow icon={<IconBrowseOutline16 size={14}/>} title={title} open={open && expandable} expandable={expandable} expandOnRowClick keepContentWhenOpen
      onToggle={() => setOpen(value => !value)}
      collapsedContent={summary && <><span className="flex-none w-[2px] h-[2px] mx-[8px] my-0 rounded-[var(--dsw-radius-xs)] bg-[var(--dsw-alias-label-caption)]" aria-hidden/><span className="flex-auto min-w-0 overflow-hidden [color:var(--dsw-alias-label-tertiary)] [font-size:var(--dsh-content-font-size-secondary,13px)] leading-[calc(24px+var(--dsh-content-font-delta,0px))] text-ellipsis whitespace-nowrap">{summary}</span></>}>
      <div className="box-border w-[calc(100%-22px-var(--dsh-content-font-delta,0px))] max-h-[141px] mt-[4px] mr-0 mb-0 ml-[calc(22px+var(--dsh-content-font-delta,0px))] overflow-auto pt-[10px] pr-[16px] pb-[12px] pl-[12px] rounded-[var(--dsw-radius-md)] bg-[var(--dsw-alias-markdown-code-block)] [color:var(--dsw-alias-label-tertiary)] [font:400_11px/16px_var(--ds-font-family-code)]">
        <div className="wrap-anywhere">
          {added.total > 0 && <div>{tx('agent:tools.added', { names: names(added) })}</div>}
          {removed.total > 0 && <div>{tx('agent:tools.removed', { names: names(removed) })}</div>}
        </div>
      </div>
    </DisclosureRow>
  </div>;
}
