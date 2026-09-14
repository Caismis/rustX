// Presentation extracted from DeepSeek Harness ui-conversation/InputBar.
// Native textarea replaces Lexical/attachment/command/queue machines. No retry.
import { useState } from 'react';
import { Button } from './primitives/Button';
import css from './InputBar.module.css';
export function InputBar({ disabled, busy, active, onSend, onCancel }: {
  disabled: boolean; busy: boolean; active: boolean;
  onSend: (text: string, steer: boolean) => Promise<boolean>; onCancel: () => void;
}) {
  const [draft, setDraft] = useState('');
  const submit = async (steer = false) => {
    if (disabled || busy || !draft.trim()) return;
    const submitted = draft;
    if (await onSend(submitted, steer)) setDraft(current => current === submitted ? '' : current);
  };
  return <div className={css.root}>
    <div className={css.card} data-composer-card>
      <div className={css.scroll}><div className={css.grow}>
        <textarea className={css.input} aria-label="Message" placeholder="Give this Session a task…"
          value={draft} disabled={disabled || busy} rows={3} onChange={event => setDraft(event.target.value)}
          onKeyDown={event => {
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) {
              event.preventDefault(); void submit();
            }
          }} />
      </div></div>
      <div className={css.row}>
        <span className="muted">Enter to send · Shift+Enter for newline</span>
        <div className={css.trailing}>
          {active && <Button size="sm" variant="outline" disabled={disabled || busy} onClick={onCancel}>Cancel turn</Button>}
          {active && <Button size="sm" variant="outline" disabled={disabled || busy || !draft.trim()} onClick={() => void submit(true)}>Steer</Button>}
          <Button variant="primary" disabled={disabled || busy || !draft.trim()} onClick={() => void submit()}>{busy ? 'Awaiting acknowledgement…' : 'Send'}</Button>
        </div>
      </div>
    </div>
  </div>;
}
