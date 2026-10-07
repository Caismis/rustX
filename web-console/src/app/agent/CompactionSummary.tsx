import { useMemo, useState, type ReactNode } from 'react';
import type { Root, RootContent } from 'mdast';
import { parseGfm } from '../../presentation/markdown/parse';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconCompactOutline16 } from '../../presentation/primitives/icons';
import css from './CompactionSummary.module.css';

function textOf(node: Root | RootContent): string {
  if (node.type === 'definition' || node.type === 'html') return '';
  if ('children' in node) return node.children.map(textOf).join(node.type === 'root' || node.type === 'list' ? ' ' : '');
  if ('value' in node) return node.value;
  if ('alt' in node) return node.alt ?? '';
  return ' ';
}

/** A compact reading surface for the canonical summary, never a new message. */
export function CompactionSummary({ preview, children }: { preview: string; children: ReactNode }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const plainPreview = useMemo(() => textOf(parseGfm(preview.slice(0, 2000))).replace(/\s+/gu, ' ').trim(), [preview]);
  return <section className={css.card} aria-label={tx('agent:compaction.summary')}>
    <DisclosureRow icon={<IconCompactOutline16 />} title={tx('agent:compaction.summary')}
      open={open} expandable expandOnRowClick onToggle={() => setOpen(value => !value)}
      rowClassName={css.header} collapsedContent={<span className={css.preview}>{plainPreview}</span>}>
      <div className={css.body}><p className={css.description}>{tx('agent:compaction.description')}</p>{children}</div>
    </DisclosureRow>
  </section>;
}
