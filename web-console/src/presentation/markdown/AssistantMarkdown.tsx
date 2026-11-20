import { Fragment, useMemo, useRef } from 'react';
import { useTranslation } from '../../locale/react';
import { MarkdownText } from './MarkdownText';
import { parseExpress, type ExpressSurface } from './a2ui-express';
import { ExpressCard } from './ExpressCard';
import css from './ExpressCard.module.css';

type Part = { start: number; text: string; tree?: ExpressSurface; pending?: boolean };

function expressParts(source: string, streaming: boolean, cache: Map<string, ExpressSurface | null>): Part[] {
  if (!source.includes('<a2ui>') && !source.includes('a2ui')) return [{ start: 0, text: source }];
  const parts: Part[] = [];
  let consumed = 0;
  let fence: { marker: string; start: number; body: number; language: string } | undefined;
  let tag: number | undefined;
  function append(start: number, end: number, body: string) {
    let tree = cache.get(body);
    if (tree === undefined) {
      tree = parseExpress(body);
      if (cache.size >= 8) cache.clear();
      cache.set(body, tree);
    }
    if (!tree) return;
    if (consumed < start) parts.push({ start: consumed, text: source.slice(consumed, start) });
    parts.push({ start, text: body, tree });
    consumed = end;
  }
  for (const line of source.matchAll(/^.*(?:\r?\n|$)/gm)) {
    const text = line[0].trimEnd();
    const end = line.index + line[0].length;
    if (tag !== undefined) {
      if (text.trim() === '</a2ui>') {
        append(tag, end, source.slice(tag, end));
        tag = undefined;
      }
      continue;
    }
    const marker = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(text);
    if (fence) {
      if (marker && marker[1][0] === fence.marker[0] && marker[1].length >= fence.marker.length && !marker[2].trim()) {
        const body = source.slice(fence.body, line.index).trim();
        if (fence.language === 'a2ui' || (body.startsWith('<a2ui>') && body.endsWith('</a2ui>'))) append(fence.start, end, body);
        fence = undefined;
      }
    } else if (marker) {
      fence = { marker: marker[1], language: marker[2].trim(), start: line.index, body: end };
    } else if (/^ {0,3}<a2ui>$/.test(text)) {
      tag = line.index;
    }
  }
  const openFence = fence && (fence.language === 'a2ui' || source.slice(fence.body).trimStart().startsWith('<a2ui>'));
  const pending = tag ?? (openFence ? fence?.start : undefined);
  if (streaming && pending !== undefined) {
    if (consumed < pending) parts.push({ start: consumed, text: source.slice(consumed, pending) });
    parts.push({ start: pending, text: '', pending: true });
    return parts;
  }
  if (consumed < source.length || !parts.length) parts.push({ start: consumed, text: source.slice(consumed) });
  return parts;
}

export function AssistantMarkdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const tx = useTranslation();
  const cache = useRef(new Map<string, ExpressSurface | null>());
  const parts = useMemo(() => expressParts(text, streaming, cache.current), [text, streaming]);
  return <>{parts.map((part, index) => <Fragment key={part.start}>
    {part.pending ? <div className={css.pending} role="status">{tx('interactions:express.preparing')}</div>
      : part.tree ? <ExpressCard key={part.text} surface={part.tree} streaming={streaming}/>
      : <MarkdownText text={part.text} streaming={streaming && index === parts.length - 1}/>}
  </Fragment>)}</>;
}
