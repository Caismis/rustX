import { useMemo } from 'react';
import { MarkdownText } from '../../../presentation/markdown/MarkdownText';
import { splitFrontmatter } from './frontmatter';
import { FrontmatterFields } from './frontmatter-fields';

export function MarkdownBody({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const split = useMemo(() => splitFrontmatter(text), [text]);
  return <div className="min-w-0 px-[12px] py-[10px] [font-family:var(--dsw-font,inherit)] whitespace-normal" data-document-markdown>
    {split && <div className="mt-0 mx-0 mb-[16px] px-[12px] py-[10px] [border:0.5px_solid_var(--dsw-alias-border-l2)] rounded-[var(--dsw-radius-md)] bg-[var(--dsw-alias-bg-layer-1)] [font:var(--dsw-font-markdown-small)] [color:var(--dsw-alias-label-primary)] whitespace-pre-wrap wrap-anywhere empty:hidden" data-document-frontmatter><FrontmatterFields source={split.source} fallback={<pre className="m-0 [font-family:var(--ds-font-family-code)] [white-space:inherit]">{split.source}</pre>}/></div>}
    <MarkdownText text={split?.body ?? text} streaming={streaming}/>
  </div>;
}
