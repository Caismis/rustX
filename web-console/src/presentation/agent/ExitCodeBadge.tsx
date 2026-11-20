import { Tag } from '../primitives/Tag';
import css from './ExitCodeBadge.module.css';

export function ExitCodeBadge({ label }: { label: string }) {
  return (
    <Tag tone="neutral" className={css.badge}>
      <span className={css.dot} aria-hidden="true" />
      {label}
    </Tag>
  );
}
