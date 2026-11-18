import type { ReactNode } from 'react';
import type { SessionMessageVersion, SessionMessageVersionFamily } from '../../../../protocol/app-server/v80';
import { useTranslation } from '../../locale/react';
import { BundledIcon } from '../../presentation/primitives/BundledIcon';
import { Button } from '../../presentation/primitives/Button';
import { IconChevronLeftOutline14, IconChevronRightOutline14, IconEditOutline16 } from '../../presentation/primitives/icons';
import { StateDot } from '../../presentation/primitives/StateDot';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import actionCss from './TurnTail.module.css';
import css from './UserMessageVersions.module.css';

export interface UserMessageVersionLabels {
  edit: string;
  retry: string;
  cancel: string;
  input: string;
  loading: string;
}

export interface UserMessageEditorProps {
  labels: Pick<UserMessageVersionLabels, 'retry' | 'cancel' | 'input' | 'loading'>;
  editing: {
    text: string;
    onText: (text: string) => void;
    hasAttachments: boolean;
  };
  busy?: boolean;
  disabled?: boolean;
  onRetry: () => void;
  onCancel: () => void;
}

export interface UserMessageVersionsProps {
  labels: UserMessageVersionLabels;
  family?: SessionMessageVersionFamily;
  busy?: boolean;
  disabled?: boolean;
  onEdit: () => void;
  onSwitch: (version: SessionMessageVersion) => void;
}

function VersionAction({ label, tooltip = label, disabled, busy, onClick, children }: {
  label: string;
  tooltip?: string;
  disabled?: boolean;
  busy?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Tooltip label={tooltip} side="bottom" delayMs={300}>
      <button type="button" className={actionCss.action} aria-label={label}
        aria-busy={busy || undefined} disabled={disabled} onClick={onClick}>
        {children}
      </button>
    </Tooltip>
  );
}

export function MessageVersionPager({ family, onSwitch, disabled, compact }: {
  family: SessionMessageVersionFamily;
  onSwitch: (version: SessionMessageVersion) => void;
  disabled?: boolean;
  compact?: boolean;
}) {
  const tx = useTranslation();
  const { previous, next, current_index, total } = family;
  if (total < 2) return null;
  return (
    <span className={css.pager} data-compact={compact || undefined}
      onClick={event => event.stopPropagation()} onDoubleClick={event => event.stopPropagation()}>
      <VersionAction label={tx('agent:message-version.previous')} disabled={disabled || !previous} onClick={() => { if (previous) onSwitch(previous); }}>
        <BundledIcon name="lucide:chevron-left" size={14} fallback={<IconChevronLeftOutline14 />} />
      </VersionAction>
      <span className={css.position} role="status">{tx('agent:message-version.position', { current: current_index + 1, total })}</span>
      <VersionAction label={tx('agent:message-version.next')} disabled={disabled || !next} onClick={() => { if (next) onSwitch(next); }}>
        <BundledIcon name="lucide:chevron-right" size={14} fallback={<IconChevronRightOutline14 />} />
      </VersionAction>
    </span>
  );
}

export function UserMessageVersions({
  labels, family, busy = false, disabled = false, onEdit, onSwitch,
}: UserMessageVersionsProps) {
  const unavailable = disabled || busy;
  return (
    <span className={css.actions}>
      {family && <MessageVersionPager family={family} disabled={unavailable} onSwitch={onSwitch} />}
      <VersionAction label={labels.edit} tooltip={busy ? labels.loading : labels.edit}
        disabled={unavailable} busy={busy} onClick={onEdit}>
        {busy ? <StateDot state="ongoing" /> :
          <BundledIcon name="lucide:pencil" size={15} fallback={<IconEditOutline16 />} />}
      </VersionAction>
      <span className={css.visuallyHidden} role="status">{busy ? labels.loading : ''}</span>
    </span>
  );
}

export function UserMessageEditor({
  labels, editing, busy = false, disabled = false, onRetry, onCancel,
}: UserMessageEditorProps) {
  const unavailable = disabled || busy;
  const canRetry = !unavailable && (editing.text.trim() !== '' || editing.hasAttachments);
  return (
    <form className={css.editor} aria-busy={busy || undefined} onSubmit={event => {
      event.preventDefault();
      if (canRetry) onRetry();
    }} onKeyDown={event => {
      if (event.defaultPrevented || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        if (!unavailable) onCancel();
      } else if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
        event.preventDefault();
        event.stopPropagation();
        if (canRetry && !event.repeat) event.currentTarget.requestSubmit();
      }
    }}>
      <textarea className={css.input} aria-label={labels.input} value={editing.text}
        rows={Math.max(3, editing.text.split('\n', 10).length)}
        disabled={unavailable} autoFocus onChange={event => editing.onText(event.target.value)} />
      <div className={css.editorActions}>
        <Button variant="outline" size="sm" disabled={unavailable} onClick={onCancel} aria-keyshortcuts="Escape">
          {labels.cancel}
        </Button>
        <Button type="submit" variant="primary" size="sm" disabled={!canRetry} aria-keyshortcuts="Control+Enter Meta+Enter">
          {busy ? labels.loading : labels.retry}
        </Button>
      </div>
    </form>
  );
}
