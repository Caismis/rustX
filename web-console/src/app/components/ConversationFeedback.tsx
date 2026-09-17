import type { ReactNode } from 'react';
import { StateDot } from '../../presentation/primitives/StateDot';
import css from './ConversationFeedback.module.css';
export function Feedback({ kind, title, children }: { kind: 'loading' | 'empty' | 'error'; title: string; children?: ReactNode }) {
  return <div className={css.card} role={kind === 'error' ? 'alert' : 'status'}>
    <div className={css.heading}>{kind !== 'empty' && <StateDot state={kind === 'loading' ? 'ongoing' : 'error'} />}<strong>{title}</strong></div>
    {children}
  </div>;
}
