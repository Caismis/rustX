import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type ReactNode, type UIEvent } from 'react';
import type { FileComparison } from '../../../../protocol/app-server/v80';
import type { ChangesReviewRequest } from '../changes-review';
import { useTranslation } from '../../locale/react';
import { diffCopyText, localDiffHunks } from '../../presentation/primitives/DiffBlock';
import { languageForPath } from '../../presentation/primitives/file-language';
import { grammarLoadCount, highlightLines, subscribeGrammarLoaded, type HighlightSpan } from '../../presentation/markdown/highlight';
import { useCopyFeedback } from '../../presentation/primitives/use-copy-feedback';
import { Button } from '../../presentation/primitives/Button';
import { Menu } from '../../presentation/primitives/Menu';
import { PathLabel } from '../../presentation/primitives/PathLabel';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import {
  IconChevronDownOutline14, IconCompareSplitOutline16, IconInspectOutline12,
  IconNowrapFill16, IconWrapFill16,
} from '../../presentation/primitives/icons';
import css from './ChangesReview.module.css';

const MAX_RENDERED_LINES = 5000;
const GROUPED = new Intl.NumberFormat('en-US');

interface DiffRow {
  kind: 'add' | 'del' | 'context';
  old: number | undefined;
  new: number | undefined;
  text: string;
}

interface SplitRow {
  left?: { no: number; text: string; kind: 'del' | 'context' };
  right?: { no: number; text: string; kind: 'add' | 'context' };
}

interface RenderedPatch {
  header: string;
  rows: DiffRow[];
  oldBase: number;
  newBase: number;
  hunkIndex: number;
}

function splitRows(rows: readonly DiffRow[]): SplitRow[] {
  const result: SplitRow[] = [];
  let dels: NonNullable<SplitRow['left']>[] = [];
  let adds: NonNullable<SplitRow['right']>[] = [];
  const flush = () => {
    for (let at = 0; at < Math.max(dels.length, adds.length); at += 1) {
      const left = dels[at];
      const right = adds[at];
      result.push({ ...(left === undefined ? {} : { left }), ...(right === undefined ? {} : { right }) });
    }
    dels = [];
    adds = [];
  };
  for (const row of rows) {
    if (row.kind === 'del') dels.push({ no: row.old!, text: row.text, kind: 'del' });
    else if (row.kind === 'add') adds.push({ no: row.new!, text: row.text, kind: 'add' });
    else {
      flush();
      result.push({ left: { no: row.old!, text: row.text, kind: 'context' }, right: { no: row.new!, text: row.text, kind: 'context' } });
    }
  }
  flush();
  return result;
}

function splitRowKind(row: SplitRow): DiffRow['kind'] {
  return row.left?.kind === 'del' ? 'del' : row.right?.kind === 'add' ? 'add' : 'context';
}

function buildPatches(hunks: FileComparison['hunks']): { patches: RenderedPatch[]; truncatedByLimit: boolean } {
  let budget = MAX_RENDERED_LINES;
  let truncatedByLimit = false;
  const patches: RenderedPatch[] = [];
  for (let hunkIndex = 0; hunkIndex < hunks.length; hunkIndex += 1) {
    const hunk = hunks[hunkIndex]!;
    for (const patch of localDiffHunks({ oldText: hunk.old_text, newText: hunk.new_text })) {
      const lines = patch.lines.filter(line => !line.startsWith('\\'));
      if (budget === 0) {
        truncatedByLimit = true;
        return { patches, truncatedByLimit };
      }
      const kept = lines.length <= budget ? lines : lines.slice(0, budget);
      if (kept.length < lines.length) truncatedByLimit = true;
      budget -= kept.length;
      const oldStart = hunk.old_start + patch.oldStart - 1;
      const newStart = hunk.new_start + patch.newStart - 1;
      let oldNo = oldStart;
      let newNo = newStart;
      const rows: DiffRow[] = kept.map(line => {
        const text = line.slice(1);
        if (line[0] === '+') return { kind: 'add', old: undefined, new: newNo++, text };
        if (line[0] === '-') return { kind: 'del', old: oldNo++, new: undefined, text };
        return { kind: 'context', old: oldNo++, new: newNo++, text };
      });
      const oldLines = lines.filter(line => !line.startsWith('+')).length;
      const newLines = lines.filter(line => !line.startsWith('-')).length;
      patches.push({
        header: `@@ -${oldStart},${oldLines} +${newStart},${newLines} @@`,
        rows,
        oldBase: hunk.old_start,
        newBase: hunk.new_start,
        hunkIndex,
      });
    }
  }
  return { patches, truncatedByLimit };
}

function DiffText({ text, spans }: { text: string; spans: readonly HighlightSpan[] | undefined }) {
  return <span className={css.text} data-diff-code={spans === undefined ? undefined : ''}>
    {spans === undefined ? text : spans.map((span, index) => <span key={index} style={span.style}>{span.text}</span>)}
  </span>;
}

function SplitColumns({ patches, highlights }: {
  patches: readonly RenderedPatch[];
  highlights: readonly { old?: HighlightSpan[][]; next?: HighlightSpan[][] }[];
}) {
  const paired = useMemo(() => patches.map(patch => ({ ...patch, split: splitRows(patch.rows) })), [patches]);
  const columns = useRef<Record<'left' | 'right', HTMLDivElement | null>>({ left: null, right: null });
  const offsets = useRef({ left: { scrollLeft: 0, scrollTop: 0 }, right: { scrollLeft: 0, scrollTop: 0 } });
  const follow = (side: 'left' | 'right') => (event: UIEvent<HTMLDivElement>) => {
    const peer = side === 'left' ? 'right' : 'left';
    const other = columns.current[peer];
    if (other === null) return;
    for (const axis of ['scrollLeft', 'scrollTop'] as const) {
      const value = event.currentTarget[axis];
      if (offsets.current[side][axis] === value) continue;
      offsets.current[side][axis] = value;
      other[axis] = value;
      offsets.current[peer][axis] = other[axis];
    }
  };
  return <div className={css.columns}>
    {(['left', 'right'] as const).map(side => (
      <div key={side} className={css.column} data-diff-side={side}
        ref={element => { columns.current[side] = element; }} onScroll={follow(side)}>
        {paired.map((patch, position) => (
          <section key={position} className={css.hunk}>
            <div className={css.hunkHeader} data-diff-hunk-header>{patch.header}</div>
            {patch.split.map((row, at) => {
              const cell = row[side];
              const spans = cell === undefined
                ? undefined
                : side === 'left'
                  ? highlights[patch.hunkIndex]?.old?.[cell.no - patch.oldBase]
                  : highlights[patch.hunkIndex]?.next?.[cell.no - patch.newBase];
              return <div key={at} className={`${css.sideLine} ${cell === undefined ? css.empty : css[cell.kind]}`} data-diff-line={splitRowKind(row)}>
                <span className={css.number}>{cell?.no ?? ''}</span>
                <DiffText text={cell?.text ?? ''} spans={spans}/>
              </div>;
            })}
          </section>
        ))}
      </div>
    ))}
  </div>;
}

function FileDiffBody({ hunks, path, split, wrap, truncated }: {
  hunks: FileComparison['hunks']; path: string; split: boolean; wrap: boolean; truncated: boolean;
}) {
  const tx = useTranslation();
  const language = languageForPath(path);
  const grammarRevision = useSyncExternalStore(subscribeGrammarLoaded, grammarLoadCount, grammarLoadCount);
  const { patches, truncatedByLimit } = useMemo(() => buildPatches(hunks), [hunks]);
  const highlights = useMemo(() => hunks.map(hunk => ({
    old: highlightLines(hunk.old_text, language),
    next: highlightLines(hunk.new_text, language),
  })), [hunks, language, grammarRevision]);
  const hasAdditions = useMemo(() => patches.some(patch => patch.rows.some(row => row.kind === 'add')), [patches]);
  const hasDeletions = useMemo(() => patches.some(patch => patch.rows.some(row => row.kind === 'del')), [patches]);
  const effectiveSplit = split && hasAdditions && hasDeletions;
  return <div className={css.body} data-review-view={effectiveSplit ? 'split' : 'unified'} data-review-wrap={wrap || undefined}>
    {!patches.length && !truncated && <p className={css.note}>{tx('artifacts:changes.unchanged')}</p>}
    {truncated && <p className={css.note}>{tx('artifacts:changes.truncated')}</p>}
    {truncatedByLimit && <p className={css.note}>{tx('artifacts:changes.truncated-lines', { count: String(MAX_RENDERED_LINES) })}</p>}
    {effectiveSplit && !wrap
      ? <SplitColumns patches={patches} highlights={highlights}/>
      : patches.map((patch, position) => (
        <section key={position} className={css.hunk}>
          <div className={css.hunkHeader} data-diff-hunk-header>{patch.header}</div>
          {effectiveSplit
            ? splitRows(patch.rows).map((row, at) => (
              <div key={at} className={css.splitLine} data-diff-line={splitRowKind(row)}>
                <span className={`${css.cell} ${row.left === undefined ? css.empty : css[row.left.kind]}`}>
                  <span className={css.number}>{row.left?.no ?? ''}</span>
                  <DiffText text={row.left?.text ?? ''} spans={row.left === undefined ? undefined : highlights[patch.hunkIndex]?.old?.[row.left.no - patch.oldBase]}/>
                </span>
                <span className={`${css.cell} ${row.right === undefined ? css.empty : css[row.right.kind]}`}>
                  <span className={css.number}>{row.right?.no ?? ''}</span>
                  <DiffText text={row.right?.text ?? ''} spans={row.right === undefined ? undefined : highlights[patch.hunkIndex]?.next?.[row.right.no - patch.newBase]}/>
                </span>
              </div>
            ))
            : patch.rows.map((row, at) => {
              const spans = row.kind === 'add'
                ? highlights[patch.hunkIndex]?.next?.[row.new! - patch.newBase]
                : highlights[patch.hunkIndex]?.old?.[row.old! - patch.oldBase];
              return <div key={at} className={`${css.line} ${css[row.kind]}`} data-diff-line={row.kind}>
                <span className={css.number}>{row.old ?? ''}</span>
                <span className={css.number}>{row.new ?? ''}</span>
                <span className={css.sign}>{row.kind === 'add' ? '+' : row.kind === 'del' ? '-' : ' '}</span>
                <DiffText text={row.text} spans={spans}/>
              </div>;
            })}
        </section>
      ))}
  </div>;
}

function PreviewCopy({ text }: { text: string }) {
  const tx = useTranslation();
  const { copied, onCopy } = useCopyFeedback(text);
  return <Button size="sm" className={css.copy} disabled={!text} onClick={onCopy}>
    {copied ? tx('artifacts:changes.copied') : tx('artifacts:changes.copy')}
  </Button>;
}

function FileCounts({ file }: { file: ChangesReviewRequest['summary']['files'][number] }) {
  const tx = useTranslation();
  if (file.binary) return <span className={css.label}>{tx('artifacts:changes.binary')}</span>;
  if (file.oversized) return <span className={css.label}>{tx('artifacts:changes.oversized')}</span>;
  return <>
    <span className={css.added}>+{GROUPED.format(file.added)}</span>
    <span className={css.deleted}>-{GROUPED.format(file.deleted)}</span>
  </>;
}

export function ChangeComparison({ request, index, visible = true, preview = false, split = false, wrap = false }: {
  request: ChangesReviewRequest; index: number; visible?: boolean; preview?: boolean; split?: boolean; wrap?: boolean;
}) {
  const tx = useTranslation();
  const current = useSyncExternalStore(request.client.subscribe, request.owned, request.owned);
  const identity = JSON.stringify([request.target, request.attemptId, index]);
  const [read, setRead] = useState<{ identity: string; comparison?: FileComparison | null; error?: boolean }>();
  const [retry, setRetry] = useState(0);
  const state: { comparison?: FileComparison | null; error?: boolean } = read?.identity === identity ? read : {};
  useEffect(() => {
    if (!current || !visible) return;
    let live = true;
    const owned = () => live && request.owned();
    setRead({ identity });
    void request.client.request({ method: 'conversation/changeDiff', params: { target: request.target, attempt_id: request.attemptId, index } }, 'workspace_change_diff', undefined, owned)
      .then(value => { if (owned()) setRead({ identity, comparison: value.comparison }); }, () => { if (owned()) setRead({ identity, error: true }); });
    return () => { live = false; };
  }, [request.client, request.owned, identity, current, visible, retry]);
  const comparison = state.comparison;
  const file = request.summary.files[index];
  const diffs = useMemo(() => comparison && !comparison.binary && !comparison.oversized && file
    ? comparison.hunks.map(hunk => ({ path: `${file.display}:${hunk.new_start}`, oldText: hunk.old_text, newText: hunk.new_text })) : [], [comparison, file]);
  const copyText = useMemo(() => preview ? diffCopyText(diffs) : '', [preview, diffs]);
  if (!current) return <p className={css.status}>{tx('artifacts:changes.missing')}</p>;
  if (!file) return <p className={css.status}>{tx('artifacts:changes.missing')}</p>;
  return <div className={preview ? css.preview : css.comparison} data-change-comparison data-changes-hover-preview={preview || undefined}>
    {preview && <div className={css.previewHeader}><span className={css.path} data-changes-preview-path>{file.path}</span><PreviewCopy key={JSON.stringify([identity, copyText])} text={copyText}/></div>}
    {state.error
      ? <div className={css.status} role="alert"><span>{tx('artifacts:changes.error')}</span><Button size="sm" onClick={() => setRetry(value => value + 1)}>{tx('artifacts:changes.retry')}</Button></div>
      : comparison === undefined
        ? <p className={css.status} role="status">{tx('artifacts:changes.loading')}</p>
        : comparison === null
          ? <p className={css.status}>{tx('artifacts:changes.missing')}</p>
          : comparison.binary
            ? <p className={css.status}>{tx('artifacts:changes.binary-diff')}</p>
            : comparison.oversized
              ? <p className={css.status}>{tx('artifacts:changes.oversized-diff')}</p>
              : <FileDiffBody hunks={comparison.hunks} path={file.path} split={preview ? false : split} wrap={preview ? false : wrap} truncated={comparison.truncated}/>}
  </div>;
}

export function ChangesReview({ request, visible, onOpenFile, fileActions }: {
  request: ChangesReviewRequest;
  visible: boolean;
  onOpenFile?: (path: string) => void;
  fileActions?: (path: string) => ReactNode;
}) {
  const tx = useTranslation();
  const [index, setIndex] = useState(request.index);
  const [menuOpen, setMenuOpen] = useState(false);
  const [split, setSplit] = useState(false);
  const [wrap, setWrap] = useState(false);
  useLayoutEffect(() => setIndex(request.index), [request]);
  const files = request.summary.files;
  const safeIndex = files[index] !== undefined ? index : 0;
  const file = files[safeIndex];
  return <section className={css.root} aria-label={tx('artifacts:changes.open-review')} data-changes-review>
    <div className={css.header}>
      {file === undefined
        ? <span className={css.selectorLabel}>{request.turn != null ? tx('artifacts:changes.review-title', { turn: request.turn }) : tx('artifacts:changes.title', { count: request.summary.total })}</span>
        : <Menu className={css.selector} open={menuOpen} autoFocus align="start" dense onClose={() => setMenuOpen(false)}
          anchor={<button type="button" className={css.selectorButton} aria-haspopup="menu" aria-expanded={menuOpen}
            aria-label={tx('artifacts:changes.select-file')} title={file.display} data-review-file={file.path}
            onClick={() => setMenuOpen(value => !value)}>
            <PathLabel path={file.display}/>
            <IconChevronDownOutline14 size={12}/>
          </button>}
          items={files.map((entry, at) => ({
            id: String(at),
            label: <span className={css.item}>
              <span className={css.itemPath}>{entry.display}</span>
              <span className={css.itemCounts}><FileCounts file={entry}/></span>
            </span>,
          }))}
          selectedId={String(safeIndex)}
          onSelect={id => { setIndex(Number(id)); setMenuOpen(false); }}/>}
      {file !== undefined && <span className={css.counts}><FileCounts file={file}/></span>}
      <span className={css.tools}>
        <Tooltip label={tx(split ? 'artifacts:changes.unified' : 'artifacts:changes.split')} side="bottom" delayMs={500}>
          <button type="button" className={css.tool} aria-pressed={split} aria-label={tx('artifacts:changes.split-aria')} data-review-tool="split"
            onClick={() => setSplit(value => !value)}>
            <IconCompareSplitOutline16 className={css.compareIcon}/>
          </button>
        </Tooltip>
        <Tooltip label={tx(wrap ? 'artifacts:changes.nowrap' : 'artifacts:changes.wrap')} side="bottom" delayMs={500}>
          <button type="button" className={css.tool} aria-pressed={wrap} aria-label={tx('artifacts:changes.wrap-aria')} data-review-tool="wrap"
            onClick={() => setWrap(value => !value)}>
            {wrap ? <IconNowrapFill16/> : <IconWrapFill16/>}
          </button>
        </Tooltip>
        {file !== undefined && onOpenFile && <Tooltip label={tx('artifacts:changes.open-file')} side="bottom" delayMs={500}>
          <button type="button" className={css.tool} aria-label={tx('artifacts:changes.open-file-aria', { name: file.display })} data-review-tool="open-file"
            onClick={() => onOpenFile(file.path)}>
            <IconInspectOutline12 size={15}/>
          </button>
        </Tooltip>}
        {file !== undefined && fileActions?.(file.path)}
      </span>
    </div>
    <ChangeComparison request={request} index={safeIndex} visible={visible} split={split} wrap={wrap}/>
  </section>;
}
