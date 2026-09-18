import type { SessionProductState, SessionRecovery } from '../bindings/session-product';
import { Button } from '../presentation/primitives/Button';

export function SessionStatus({ product, recover }: { product: SessionProductState; recover: (action: SessionRecovery) => void }) {
  if (!product.label) return null;
  return <div className={`session-status ${product.severity === 'quiet' ? '' : 'notice'}`} role="status" aria-label="Session status" data-severity={product.severity}>
    <span>{product.label}</span>
    {product.detail && <p>{product.detail}</p>}
    {product.recovery && <Button size="sm" onClick={() => recover(product.recovery!.action)}>{product.recovery.label}</Button>}
  </div>;
}
