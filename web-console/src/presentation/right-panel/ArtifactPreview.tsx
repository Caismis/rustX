import { useState } from 'react';
import { Button } from '../primitives/Button';
import css from './ArtifactPreview.module.css';
/** Finite artifact presentation only; the app supplies bytes from its native resource API. */
export function ArtifactPreview({ name, text, url, image, error, loading, retry }: { name: string; text?: string; url?: string; image: boolean; error?: string; loading: boolean; retry: () => void }) {
  const [wrap, setWrap] = useState(true);
  return <section className={css.preview} aria-label="Artifact preview"><header className={css.header}><strong title={name}>{name}</strong>{text !== undefined && <Button size="sm" aria-pressed={wrap} onClick={() => setWrap(value => !value)}>Wrap lines</Button>}</header>
    {loading && <p role="status">Reading native artifact…</p>}
    {error && <div role="alert"><p>{error}</p><Button onClick={retry}>Retry preview</Button></div>}
    {text !== undefined && <pre className={css.body} data-wrap={wrap}>{text}</pre>}
    {image && url && <img className={css.image} src={url} alt={name} />}
    {!image && text === undefined && !error && !loading && <p>This artifact has no supported inline viewer.</p>}
    {url && <a href={url} download={name}>Download artifact</a>}
  </section>;
}
