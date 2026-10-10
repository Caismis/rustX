import { useRef } from 'react';
import { useTranslation } from '../../locale/react';
import { IconSendOutline14, IconStopFill16 } from '../../presentation/primitives/icons';
import css from '../../presentation/agent/Composer.module.css';
import agentCss from './Subagents.module.css';

/** The child uses the same input surface and gesture as the root conversation.
 * Child admission stays agent/sendMessage; root model and upload controls do
 * not masquerade as child capabilities. */
export function SubagentComposer({ name, value, onChange, send, disabled, pending, running, interrupt, interruptDisabled }: {
  name: string; value: string; onChange: (value: string) => void; send: () => void;
  disabled: boolean; pending: boolean;
  running: boolean; interrupt: () => void; interruptDisabled: boolean;
}) {
  const tx = useTranslation(), form = useRef<HTMLFormElement>(null);
  const ready = !disabled && !!value.trim();
  const stop = (primary: boolean) => <button className={primary ? css.primary : agentCss.interrupt} type="button" disabled={interruptDisabled} onClick={interrupt} aria-label={tx('common:activity.interrupt')} title={tx('common:activity.interrupt')}><IconStopFill16/></button>;
  return <form ref={form} className={css.root} onSubmit={event => { event.preventDefault(); if (ready) send(); }}>
    <div className={css.card} data-composer-card="">
      <div className={css.scroll}><textarea className={css.input} rows={1}
        aria-label={tx('common:activity.message-label', { name })}
        placeholder={tx('agent:agent-composer.describe-what-you-want-to-do')}
        value={value} onChange={event => onChange(event.target.value)} disabled={disabled && !pending} readOnly={pending}
        onKeyDown={event => {
          if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
          event.preventDefault(); if (ready) form.current?.requestSubmit();
        }}/></div>
      <div className={`${css.row} ${agentCss.toolbar}`}>
        <div className={css.trailing}>{running && value.trim() && stop(false)}{running && !value.trim() ? stop(true) : <button className={css.primary} type="submit" disabled={!ready} aria-label={tx('common:activity.send')} title={tx('common:activity.send')}><IconSendOutline14/></button>}</div>
      </div>
    </div>
  </form>;
}
