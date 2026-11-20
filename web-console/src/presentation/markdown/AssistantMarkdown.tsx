import { Fragment, lazy, useMemo, useRef } from 'react';
import { MarkdownText } from './MarkdownText';
import { parseExpress, type ExpressNode, type ExpressSurface } from './a2ui-express';
import { ExpressCard } from './ExpressCard';
import { ExpressGeneration } from './ExpressGeneration';

const PlaygroundExpress = lazy(() => import('./PlaygroundExpress'));
type Part = { start: number; text: string; tree?: ExpressSurface; source?: string; draft?: string };

function needsPlayground(node: ExpressNode): boolean {
  return node.kind === 'Unsupported' || ['AreaChart', 'CandlestickChart', 'StockTicker'].includes(node.kind)
    || ((node.kind === 'LineChart' || node.kind === 'BarChart') && Object.hasOwn(node.props, 'data'))
    || node.children.some(needsPlayground);
}

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
    if (consumed < start) parts.push({ start: consumed, text: source.slice(consumed, start) });
    parts.push(!tree || needsPlayground(tree.root)
      ? { start, text: source.slice(start, end), source: body, tree: tree ?? undefined }
      : { start, text: body, tree });
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
    const body = tag === undefined ? source.slice(fence?.body ?? pending) : source.slice(tag + '<a2ui>'.length);
    parts.push({ start: pending, text: '', draft: body.replace(/^\s*<a2ui>\s*/, '') });
    return parts;
  }
  if (consumed < source.length || !parts.length) parts.push({ start: consumed, text: source.slice(consumed) });
  return parts;
}

export function AssistantMarkdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const cache = useRef(new Map<string, ExpressSurface | null>());
  const parts = useMemo(() => expressParts(text, streaming, cache.current), [text, streaming]);
  return <>{parts.map((part, index) => <Fragment key={part.start}>
    {part.draft !== undefined || part.source || part.tree
      ? <ExpressGeneration live={streaming} draft={part.draft}>
        {part.source
          ? <PlaygroundExpress key={part.source} source={part.source} fallback={part.text} tree={part.tree} streaming={streaming}/>
          : part.tree && <ExpressCard key={part.text} surface={part.tree} streaming={streaming}/>}
      </ExpressGeneration>
      : <MarkdownText text={part.text} streaming={streaming && index === parts.length - 1}/>}
  </Fragment>)}</>;
}
