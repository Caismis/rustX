import { useTranslation } from '../../locale/react';
import { useState } from 'react';
import { Button } from '../primitives/Button';
import css from './ArtifactPreview.module.css';
/** Finite artifact presentation only; the app supplies bytes from its native resource API. */
export function ArtifactPreview({ name, text, url, image, error, loading, retry }: { name: string; text?: string; url?: string; image: boolean; error?: string; loading: boolean; retry: () => void }) {
  const tx = useTranslation();
  const [wrap, setWrap] = useState(true);
  return <section className={css.preview} aria-label={tx('artifacts:artifact-preview.artifact-preview')}><header className={css.header}><strong title={name}>{name}</strong>{text !== undefined && <Button size="sm" aria-pressed={wrap} onClick={() => setWrap(value => !value)}>{tx('artifacts:artifact-preview.wrap-lines')}</Button>}</header>
    {loading && <p role="status">{tx('artifacts:artifact-preview.reading-native-artifact')}</p>}
    {error && <div role="alert"><p>{error}</p><Button onClick={retry}>{tx('artifacts:artifact-preview.retry-preview')}</Button></div>}
    {text !== undefined && <pre className={css.body} data-wrap={wrap}>{text}</pre>}
    {image && url && <img className={css.image} src={url} alt={name} />}
    {!image && text === undefined && !error && !loading && <p>{tx('artifacts:artifact-preview.this-artifact-has-no-supported-inline-viewer')}</p>}
    {url && <a href={url} download={name}>{tx('artifacts:artifact-preview.download-artifact')}</a>}
  </section>;
}
