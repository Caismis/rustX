import { useTranslation } from '../../locale/react';
import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import clsx from 'clsx';
import { Button } from '../primitives/Button';
import { IconChevronLeftOutline14, IconChevronRightOutline14 } from '../primitives/icons';
import { settingsPageShellCss } from './settingsPageShellCss';

/** Tactile DSH breadcrumb/back pill button for sub-views and resource detail editors. */
export function SettingsBackButton({
  label,
  current,
  onClick,
  className,
}: {
  label: ReactNode;
  current?: ReactNode;
  onClick: () => void;
  className?: string;
}) {
  return (
    <div className={clsx(settingsPageShellCss.breadcrumbBar, className)}>
      <button type="button" className={settingsPageShellCss.backButton} onClick={onClick}>
        <span className={settingsPageShellCss.backIcon} aria-hidden="true">
          <IconChevronLeftOutline14 />
        </span>
        <span>{label}</span>
      </button>
      {current != null && (
        <>
          <span className={settingsPageShellCss.crumbSep} aria-hidden="true">
            <IconChevronRightOutline14 />
          </span>
          <span className={settingsPageShellCss.crumbCurrent}>{current}</span>
        </>
      )}
    </div>
  );
}

/** The shared 960px page skeleton every Settings and resource page uses:
 * a fixed head (title, description and toolbar), one scrolling body on a
 * centered canvas, and a fixed footer pinned to the bottom of the shell. */
export function SettingsPageShell({
  title,
  description,
  breadcrumb,
  toolbar,
  footer,
  bodyDisabled = false,
  children,
  className,
}: {
  title?: ReactNode;
  description?: ReactNode;
  breadcrumb?: ReactNode;
  toolbar?: ReactNode;
  /** Defaults to omitting the footer unless a footer node is passed. */
  footer?: ReactNode;
  /** Disables interactive form controls inside the page canvas without dimming layout opacity. */
  bodyDisabled?: boolean;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={clsx(settingsPageShellCss.shell, className)}>
      {(title || toolbar || breadcrumb) && (
        <header className={settingsPageShellCss.pageHead}>
          <div className={settingsPageShellCss.headInner}>
            <div className={settingsPageShellCss.headTitle}>
              {breadcrumb}
              {title && <h1 className={settingsPageShellCss.titleCls}>{title}</h1>}
              {description && <p className={settingsPageShellCss.descriptionCls}>{description}</p>}
            </div>
            {toolbar && <div className={settingsPageShellCss.headToolbar}>{toolbar}</div>}
          </div>
        </header>
      )}
      <div className={settingsPageShellCss.pageBody}>
        <fieldset disabled={bodyDisabled} className={settingsPageShellCss.bodyFieldset}>
          <div className={settingsPageShellCss.canvas}>{children}</div>
        </fieldset>
      </div>
      {footer != null && footer}
    </section>
  );
}

/** The fixed footer of a Settings/resource page: status on the left,
 * Cancel / Save on the right, flush at the bottom of SettingsPageShell. */
export function SettingsPageFooter({
  dirty,
  cancelEnabled = false,
  saving,
  status,
  onCancel,
  onSave,
  leading,
}: {
  dirty: boolean;
  /** Enables Cancel even when nothing has changed yet (e.g. aborting creation of a new resource). */
  cancelEnabled?: boolean;
  saving?: boolean;
  status?: ReactNode;
  leading?: ReactNode;
  onCancel: () => void;
  onSave: () => void;
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
  return (
    <footer data-settings-footer="" className={settingsPageShellCss.pageFooter}>
      <div className={settingsPageShellCss.footerInner}>
        <div className={settingsPageShellCss.footerStatus}>
          {leading}
          {status}
        </div>
        <div className={settingsPageShellCss.footerButtons} ref={buttons} tabIndex={-1}>
          <Button
            variant="outline"
            disabled={(!dirty && !cancelEnabled) || !!saving}
            onClick={onCancel}
          >
            {tx('settings:aria.cancel')}
          </Button>
          <Button
            variant="primary"
            disabled={!dirty || !!saving}
            aria-busy={!!saving}
            onClick={onSave}
          >
            {tx(saving ? 'settings:catalog.saving' : 'settings:bridge.save')}
          </Button>
        </div>
      </div>
    </footer>
  );
}

/** A titled group of settings rows (DSH `.group`), optionally framed in a card surface. */
export function SettingsGroup({
  title,
  description,
  action,
  plain = false,
  children,
}: {
  title?: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  plain?: boolean;
  children: ReactNode;
}) {
  return (
    <section className={settingsPageShellCss.group}>
      {(title || description || action) && (
        <div className={settingsPageShellCss.groupHead}>
          <div className={settingsPageShellCss.groupCopy}>
            {title && <h2 className={settingsPageShellCss.groupTitle}>{title}</h2>}
            {description && <p className={settingsPageShellCss.groupDescription}>{description}</p>}
          </div>
          {action}
        </div>
      )}
      <div className={plain ? settingsPageShellCss.groupPlain : settingsPageShellCss.groupCard}>{children}</div>
    </section>
  );
}

/** One settings row. The default is horizontal: label and description on the
 * left, the control fixed on the right. `vertical` stacks a full-width body
 * (textarea, lists, complex editors) under the same head. */
export function SettingsRow({
  label,
  description,
  badge,
  resetAction,
  control,
  vertical = false,
  children,
}: {
  label?: ReactNode;
  description?: ReactNode;
  badge?: ReactNode;
  resetAction?: ReactNode;
  control?: ReactNode;
  vertical?: boolean;
  children?: ReactNode;
}) {
  return (
    <div className={clsx(settingsPageShellCss.row, vertical && settingsPageShellCss.rowVertical)}>
      <div className={settingsPageShellCss.rowHead}>
        <div className={settingsPageShellCss.rowText}>
          {label && (
            <div className={settingsPageShellCss.rowLabel}>
              {label}
              {badge}
              {resetAction}
            </div>
          )}
          {description && <div className={settingsPageShellCss.rowDescription}>{description}</div>}
        </div>
        {control && <div className={settingsPageShellCss.rowControl}>{control}</div>}
      </div>
      {children && (vertical ? <div className={settingsPageShellCss.rowBody}>{children}</div> : children)}
    </div>
  );
}
