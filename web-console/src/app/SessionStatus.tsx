import { useTranslation } from '../locale/react';
import type { SessionProductState, SessionRecovery } from '../bindings/session-product';
import { Button } from '../presentation/primitives/Button';
import { StateDot } from '../presentation/primitives/StateDot';
import css from './SessionStatus.module.css';

export function SessionStatus({ product, recover }: { product: SessionProductState; recover: (action: SessionRecovery) => void }) {
  const tx = useTranslation();
  if (!product.label) return null;
  const recovery = product.status === 'uncertain' ? { action: 'connection-settings' as const, label: tx('common:session-product.review-outcome') } : product.recovery;
  return <div className={css.status} role="status" aria-label={tx('common:session-status.session-status')} data-severity={product.severity}>
    <StateDot state={product.status === 'connecting' || product.status === 'stopping' ? 'ongoing' : product.severity === 'error' ? 'error' : product.severity === 'warning' ? 'warning' : 'idle'}/>
    {product.detail ? <details className={css.details}><summary>{product.label}</summary><p>{product.detail}</p></details> : <span>{product.label}</span>}
    {recovery && <Button size="sm" onClick={() => recover(recovery.action)}>{recovery.label}</Button>}
  </div>;
}
