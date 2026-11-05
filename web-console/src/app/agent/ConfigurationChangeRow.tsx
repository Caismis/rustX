/* Copyright (c) 2026 DeepSeek. MIT. Tool-change presentation adapted from Harness ContextInjectionRow; see PROVENANCE.md. */
import { useState } from 'react';
import type { BoundedNames, ConfigurationChange } from '../../../../protocol/app-server/v70';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconBrowseOutline16 } from '../../presentation/primitives/icons';
import css from './ConfigurationChangeRow.module.css';

/** Whether a committed configuration change has a Chat row. Chat shows only
 * Tool additions and removals; instruction, Skill, policy and same-name Tool
 * definition changes stay in Trace and inspection, as in Harness. */
export function hasConfigurationChangeRow(change: ConfigurationChange): boolean {
  return change.tools.added.total > 0 || change.tools.removed.total > 0;
}

/** One committed Session configuration change, rendered with the Tool-change
 * disclosure chrome. A single addition or removal is one compact row with no
 * pointless expansion; several show their counts while collapsed and the names,
 * grouped by addition and removal, when expanded. */
export function ConfigurationChangeRow({ change }: { change: ConfigurationChange }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const { added, removed } = change.tools;
  const single = added.total + removed.total === 1;
  if (!hasConfigurationChangeRow(change)) return null;
  const names = (list: BoundedNames) => list.total > list.names.length
    ? tx('agent:tools.more', { names: list.names.join(', '), count: list.total - list.names.length })
    : list.names.join(', ');
  const title = single
    ? tx(added.total ? 'agent:tools.added-one' : 'agent:tools.removed-one', { name: (added.total ? added : removed).names[0] ?? '' })
    : tx('agent:tools.updated');
  const summary = single ? null
    : added.total && removed.total ? tx('agent:tools.changed-counts', { added: added.total, removed: removed.total })
      : added.total ? tx('agent:tools.added-count', { count: added.total })
        : tx('agent:tools.removed-count', { count: removed.total });
  return <div data-configuration-change data-binding-revision={change.binding_revision}>
    <DisclosureRow icon={<IconBrowseOutline16 size={14}/>} title={title} open={open && !single} expandable={!single} expandOnRowClick keepContentWhenOpen
      onToggle={() => setOpen(value => !value)}
      collapsedContent={summary && <><span className={css.sep} aria-hidden/><span className={css.summary}>{summary}</span></>}>
      <div className={css.body}>
        <div className={css.toolChanges}>
          {added.total > 0 && <div>{tx('agent:tools.added', { names: names(added) })}</div>}
          {removed.total > 0 && <div>{tx('agent:tools.removed', { names: names(removed) })}</div>}
        </div>
      </div>
    </DisclosureRow>
  </div>;
}
