import { useTranslation } from '../../locale/react';
import css from './ExpressInvalid.module.css';

/** An Express block that failed to compile: say so, and keep the reason and source one click away. */
export function ExpressInvalid({ source, error }: { source: string; error: string }) {
  const tx = useTranslation();
  return <div className={css.invalid} role="note" data-a2ui-invalid="">
    <p className={css.message}>{tx('interactions:express.invalid')}</p>
    <details className={css.details}>
      <summary>{tx('interactions:express.invalid-details')}</summary>
      {error && <pre className={css.error}>{error}</pre>}
      <pre className={css.source}><code>{source.trim()}</code></pre>
    </details>
  </div>;
}
