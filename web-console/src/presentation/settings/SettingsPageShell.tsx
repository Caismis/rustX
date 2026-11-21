import { useTranslation } from '../../locale/react';
import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import clsx from 'clsx';
import { Button } from '../primitives/Button';
import css from './SettingsPageShell.module.css';

/** The shared 960px page skeleton every Settings and resource page uses:
 * a fixed head (title, description and toolbar), one scrolling body on a
 * centered canvas, and a fixed footer that never scrolls away. */
export function SettingsPageShell({ title, description, breadcrumb, toolbar, footer, children, className }: {
  title?: ReactNode; description?: ReactNode; breadcrumb?: ReactNode; toolbar?: ReactNode;
  /** Defaults to the fixed page footer slot; pass null to suppress it. */
  footer?: ReactNode; children: ReactNode; className?: string;
}) {
  return <section className={clsx(css.shell, className)}>
    {(title || toolbar || breadcrumb) && <header className={css.pageHead}><div className={css.headInner}>
      <div className={css.headTitle}>
        {breadcrumb}
        {title && <h1 className={css.title}>{title}</h1>}
        {description && <p className={css.description}>{description}</p>}
      </div>
      {toolbar && <div className={css.headToolbar}>{toolbar}</div>}
    </div></header>}
    <div className={css.pageBody}><div className={css.canvas}>{children}</div></div>
    {footer !== null && (footer ?? <footer className={css.pageFooter} />)}
  </section>;
}

/** The fixed footer of a Settings/resource page: status on the left,
 * Cancel / Save on the right. Both actions stay in place and grey out when
 * the page has nothing to commit, so the layout never shifts. */
export function SettingsPageFooter({ dirty, saving, status, onCancel, onSave, leading, removalOnly }: {
  dirty: boolean; saving?: boolean; status?: ReactNode; leading?: ReactNode;
  /** The pending save is a deletion — style Save as a destructive action. */
  removalOnly?: boolean;
  onCancel: () => void; onSave: () => void;
}) {
  const tx = useTranslation();
  const buttons = useRef<HTMLDivElement>(null);
  // While a save is in flight the clicked button disables itself; if the
  // keyboard sat on it the browser would drop focus to <body>. Keep it inside
  // the footer instead so the user can continue from there.
  useEffect(() => {
    const el = buttons.current;
    if (saving && el && el.contains(document.activeElement)) el.focus();
  }, [saving]);
  return <footer className={css.pageFooter}><div className={css.footerInner}>
    <div className={css.footerStatus}>{leading}{status}</div>
    <div className={css.footerButtons} ref={buttons} tabIndex={-1}>
      <Button variant="outline" disabled={!dirty || !!saving} onClick={onCancel}>{tx('settings:aria.cancel')}</Button>
      <Button variant={removalOnly ? 'danger' : 'primary'} disabled={!dirty || !!saving} aria-busy={!!saving} onClick={onSave}>{tx(saving ? 'settings:catalog.saving' : 'settings:bridge.save')}</Button>
    </div>
  </div></footer>;
}

/** A titled group of settings rows (DSH `.group`). */
export function SettingsGroup({ title, description, action, children }: {
  title?: ReactNode; description?: ReactNode; action?: ReactNode; children: ReactNode;
}) {
  return <section className={css.group}>
    {(title || action) && <div className={css.groupHead}>
      <div>{title && <h2 className={css.groupTitle}>{title}</h2>}
        {description && <p className={css.groupDescription}>{description}</p>}</div>
      {action}
    </div>}
    <div className={css.groupRows}>{children}</div>
  </section>;
}

/** One settings row. The default is horizontal: label and description on the
 * left, the control fixed on the right. `vertical` stacks a full-width body
 * (textarea, lists, complex editors) under the same head. */
export function SettingsRow({ label, description, badge, resetAction, control, vertical = false, children }: {
  label?: ReactNode; description?: ReactNode; badge?: ReactNode; resetAction?: ReactNode;
  control?: ReactNode; vertical?: boolean; children?: ReactNode;
}) {
  return <div className={clsx(css.row, vertical && css.rowVertical)}>
    <div className={css.rowHead}>
      <div className={css.rowText}>
        {label && <div className={css.rowLabel}>{label}{badge}{resetAction}</div>}
        {description && <div className={css.rowDescription}>{description}</div>}
      </div>
      {control && <div className={css.rowControl}>{control}</div>}
    </div>
    {children && (vertical ? <div className={css.rowBody}>{children}</div> : children)}
  </div>;
}
