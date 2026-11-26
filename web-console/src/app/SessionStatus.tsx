import { useTranslation } from '../locale/react';
import type { SessionProductState, SessionRecovery } from '../bindings/session-product';
import { Button } from '../presentation/primitives/Button';
import { StateDot } from '../presentation/primitives/StateDot';

export function SessionStatus({ product, recover }: { product: SessionProductState; recover: (action: SessionRecovery) => void }) {
  const tx = useTranslation();
  if (!product.label) return null;
  const recovery = product.status === 'uncertain' ? { action: 'connection-settings' as const, label: tx('common:session-product.review-outcome') } : product.recovery;
  return <div className="flex-none flex items-baseline gap-[8px] mx-[26px] my-[6px] min-w-0 text-[12px] leading-[20px] [color:var(--dsw-alias-label-secondary)] data-[severity=error]:[color:var(--dsw-alias-state-error-primary)] data-[severity=warning]:[color:var(--dsw-alias-state-warn-label)] max-[600px]:mx-[20px] max-[600px]:flex-wrap [&>:first-child]:self-baseline [&>button]:flex-none [&>button]:min-h-[24px] [&>button]:px-[4px] [&>button]:py-0 [&>button]:[color:var(--dsw-alias-state-business-primary)] [&>button]:bg-transparent [&>button]:border-transparent [&>button]:[font-size:inherit]" role="status" aria-label={tx('common:session-status.session-status')} data-severity={product.severity}>
    <StateDot state={product.status === 'connecting' || product.status === 'stopping' ? 'ongoing' : product.severity === 'error' ? 'error' : product.severity === 'warning' ? 'warning' : 'idle'}/>
    {product.detail ? <details className="min-w-0 [&_summary]:p-0 [&_summary]:[font-size:inherit] [&_summary]:cursor-pointer [&_p]:max-w-[640px] [&_p]:mx-0 [&_p]:my-[4px] [&_p]:[color:var(--dsw-alias-label-secondary)] [&_p]:wrap-anywhere"><summary>{product.label}</summary><p>{product.detail}</p></details> : <span>{product.label}</span>}
    {recovery && <Button size="sm" onClick={() => recover(recovery.action)}>{recovery.label}</Button>}
  </div>;
}
