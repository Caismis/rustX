import { Fragment, useMemo } from 'react';
import { MarkdownText } from './MarkdownText';
import { parseExpress, type ExpressSurface } from './a2ui-express';
import { ExpressCard } from './ExpressCard';

type Part = { start: number; text: string; tree?: ExpressSurface };

function expressParts(source: string): Part[] {
  if (!source.includes('<a2ui>') && !source.includes('a2ui')) return [{ start: 0, text: source }];
  const parts: Part[] = [];
  let consumed = 0;
  let fence: { marker: string; start: number; body: number; language: string } | undefined;
  let tag: number | undefined;
  function append(start: number, end: number, body: string) {
    const tree = parseExpress(body);
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
  if (consumed < source.length || !parts.length) parts.push({ start: consumed, text: source.slice(consumed) });
  return parts;
}

export function AssistantMarkdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const parts = useMemo(() => expressParts(text), [text]);
  return <>{parts.map((part, index) => <Fragment key={part.start}>
    {part.tree ? <ExpressCard key={part.text} surface={part.tree} streaming={streaming}/> : <MarkdownText text={part.text} streaming={streaming && index === parts.length - 1}/>}
  </Fragment>)}</>;
}
