import { useMemo } from 'react';
import { MarkdownText } from '../../../presentation/markdown/MarkdownText';
import { splitFrontmatter } from './frontmatter';
import { FrontmatterFields } from './frontmatter-fields';
import css from './MarkdownBody.module.css';

export function MarkdownBody({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const split = useMemo(() => splitFrontmatter(text), [text]);
  return <div className={css.document} data-document-markdown>
    {split && <div className={css.frontmatter} data-document-frontmatter><FrontmatterFields source={split.source} fallback={<pre className={css.frontmatterSource}>{split.source}</pre>}/></div>}
    <MarkdownText text={split?.body ?? text} streaming={streaming}/>
  </div>;
}
