import clsx from 'clsx';
import { useEffect, useLayoutEffect, useMemo, useState, useSyncExternalStore } from 'react';
import type { FileComparison } from '../../../../protocol/app-server/v80';
import type { ChangesReviewRequest } from '../changes-review';
import { useTranslation } from '../../locale/react';
import { DiffBlock, diffCopyText, localDiffHunks } from '../../presentation/primitives/DiffBlock';
import { languageForPath } from '../../presentation/primitives/file-language';
import { grammarLoadCount, highlightLines, subscribeGrammarLoaded } from '../../presentation/markdown/highlight';
import { useCopyFeedback } from '../../presentation/primitives/use-copy-feedback';
import { Button } from '../../presentation/primitives/Button';
import { EnumSelect } from '../../presentation/primitives/EnumSelect';

function PreviewCopy({text}: {text: string}) {
  const tx = useTranslation(), {copied, onCopy} = useCopyFeedback(text);
  return <Button size="sm" className="flex-none min-w-[64px]" disabled={!text} onClick={onCopy}>
    {copied ? tx('artifacts:changes.copied') : tx('artifacts:changes.copy')}
  </Button>;
}

function PreviewDiff({hunks, path}: {hunks: FileComparison['hunks']; path: string}) {
  const language = languageForPath(path);
  const grammarRevision = useSyncExternalStore(subscribeGrammarLoaded, grammarLoadCount, grammarLoadCount);
  const rows = useMemo(() => hunks.map(hunk => localDiffHunks({oldText: hunk.old_text, newText: hunk.new_text}).flatMap(patch => {
    let old = patch.oldStart - 1;
    let next = patch.newStart - 1;
    return patch.lines.map(line => ({
      kind: line[0] === '+' ? 'add' : line[0] === '-' ? 'del' : 'context',
      old: line[0] === '+' ? undefined : old++,
      next: line[0] === '-' ? undefined : next++,
      text: line.slice(1),
    }));
  })), [hunks]);
  const highlights = useMemo(() => hunks.map(hunk => ({
    old: highlightLines(hunk.old_text, language),
    next: highlightLines(hunk.new_text, language),
  })), [hunks, language, grammarRevision]);
  return <div className="grid grid-cols-[minmax(max-content,100%)] content-start min-h-0 overflow-auto overscroll-contain px-0 pt-[8px] pb-[16px] [font:var(--dsw-font-markdown-code-block)]" data-review-view="unified">
    {rows.map((section, index) => <div className="[&+div]:mt-[8px]" key={index}>
      {section.map(({kind, old, next, text}, row) => {
        const spans = old === undefined ? highlights[index]?.next?.[next!] : highlights[index]?.old?.[old];
        const colored = kind === 'add' || kind === 'del';
        const tint = kind === 'add'
          ? { text: '[color:var(--dsw-alias-state-success-primary)]', wash: 'bg-[color-mix(in_srgb,var(--dsw-alias-state-success-primary)_12%,transparent)]', edge: '[box-shadow:inset_3px_0_var(--dsw-alias-state-success-primary)]' }
          : { text: '[color:var(--dsw-alias-state-error-primary)]', wash: 'bg-[color-mix(in_srgb,var(--dsw-alias-state-error-primary)_12%,transparent)]', edge: '[box-shadow:inset_3px_0_var(--dsw-alias-state-error-primary)]' };
        const numCls = colored
          ? clsx('pr-[8px] text-right select-none', tint.text, tint.wash)
          : 'pr-[8px] [color:var(--dsw-alias-label-tertiary)] text-right select-none';
        return <div className={clsx('grid grid-cols-[3.5em_3.5em_1.2em_minmax(0,1fr)] min-h-[22px] leading-[22px] whitespace-pre', colored && tint.wash)} data-diff-line={kind} key={row}>
          <span className={clsx(numCls, colored && tint.edge)}>{old === undefined ? '' : hunks[index]!.old_start + old}</span>
          <span className={numCls}>{next === undefined ? '' : hunks[index]!.new_start + next}</span>
          <span className={clsx('text-center select-none', colored && tint.text)}>{kind === 'add' ? '+' : kind === 'del' ? '-' : ' '}</span>
          <span className="pr-[16px]">{spans?.map((span, i) => <span key={i} style={span.style}>{span.text}</span>) ?? text}</span>
        </div>;
      })}
    </div>)}
  </div>;
}

export function ChangeComparison({request,index,visible=true,preview=false}: {request:ChangesReviewRequest;index:number;visible?:boolean;preview?:boolean}) {
  const tx=useTranslation(), current=useSyncExternalStore(request.client.subscribe,request.owned,request.owned);
  const identity=JSON.stringify([request.target,request.attemptId,index]);
  const [read,setRead]=useState<{identity:string;comparison?:FileComparison|null;error?:boolean}>(), [retry,setRetry]=useState(0);
  const state: {comparison?:FileComparison|null;error?:boolean}=read?.identity===identity?read:{};
  useEffect(()=>{
    if(!current||!visible)return;
    let live=true; const owned=()=>live&&request.owned();
    setRead({identity});
    void request.client.request({method:'conversation/changeDiff',params:{target:request.target,attempt_id:request.attemptId,index}},'workspace_change_diff',undefined,owned)
      .then(value=>{if(owned())setRead({identity,comparison:value.comparison});},()=>{if(owned())setRead({identity,error:true});});
    return()=>{live=false;};
  },[request.client,request.owned,identity,current,visible,retry]);
  const comparison=state.comparison, file=request.summary.files[index];
  const diffs = useMemo(() => comparison && !comparison.binary && !comparison.oversized
    ? comparison.hunks.map(hunk => ({path: `${file.display}:${hunk.new_start}`, oldText: hunk.old_text, newText: hunk.new_text})) : [], [comparison, file.display]);
  const copyText = useMemo(() => preview ? diffCopyText(diffs) : '', [preview, diffs]);
  if(!current)return <p>{tx('artifacts:changes.missing')}</p>;
  return <div className={preview ? 'flex flex-col min-h-0 min-w-0 overflow-hidden [&>p]:m-0 [&>p]:px-[16px] [&>p]:py-[12px] [&>p]:[color:var(--dsw-alias-label-secondary)] [&>p]:text-[13px]' : 'min-w-0 [color:var(--dsw-alias-label-secondary)] text-[12px]'} data-change-comparison data-changes-hover-preview={preview || undefined}>
    {preview && <div className="flex items-center min-h-[38px] gap-[8px] flex-none px-[8px] py-0 [border-bottom:0.5px_solid_var(--dsw-alias-border-l3)]"><span className="flex-1 min-w-0 overflow-x-auto overflow-y-hidden [color:var(--dsw-alias-label-tertiary)] [font:12px/20px_var(--ds-font-family-code)] whitespace-nowrap" data-changes-preview-path>{file.path}</span><PreviewCopy key={JSON.stringify([identity,copyText])} text={copyText}/></div>}
    {state.error ? <p role="alert">{tx('artifacts:changes.error')} <Button size="sm" onClick={()=>setRetry(value=>value+1)}>{tx('artifacts:changes.retry')}</Button></p>
      : comparison===undefined ? <p role="status">{tx('artifacts:changes.loading')}</p>
      : comparison===null ? <p>{tx('artifacts:changes.missing')}</p>
      : comparison.binary ? <p>{tx('artifacts:changes.binary-diff')}</p>
      : comparison.oversized ? <p>{tx('artifacts:changes.oversized-diff')}</p>
      : preview ? comparison.hunks.length > 0 && <PreviewDiff hunks={comparison.hunks} path={file.path}/>
      : <DiffBlock diffs={diffs} maxLines={500} labels={{
        copy:tx('artifacts:changes.copy'),copied:tx('artifacts:changes.copied'),collapseAria:tx('artifacts:changes.collapse'),collapse:tx('artifacts:changes.collapse'),
        expandAria:n=>tx('artifacts:changes.more-lines',{count:n}),expand:n=>tx('artifacts:changes.more-lines',{count:n}),files:()=>tx('artifacts:changes.title',{count:1}),
      }}/>}
    {comparison && !comparison.binary && !comparison.oversized && !comparison.hunks.length && !comparison.truncated && <p>{tx('artifacts:changes.unchanged')}</p>}
    {comparison?.truncated && <p>{tx('artifacts:changes.truncated')}</p>}
    </div>;
}

export function ChangesReview({request,visible}: {request:ChangesReviewRequest;visible:boolean}) {
  const tx=useTranslation(), [index,setIndex]=useState(request.index);
  useLayoutEffect(()=>setIndex(request.index),[request]);
  return <section className="flex flex-1 flex-col min-w-0 min-h-0" aria-label={tx('artifacts:changes.open-review')} data-changes-review>
    <div className="flex flex-col gap-[6px] p-[12px] text-[12px] [color:var(--dsw-alias-label-secondary)]"><span>{tx('artifacts:changes.select-file')}</span><EnumSelect label={tx('artifacts:changes.select-file')} value={String(index)} options={request.summary.files.map((file,i)=>({value:String(i),label:`${file.display} (+${file.added} -${file.deleted})`}))} onChange={value=>setIndex(Number(value))}/></div>
    <div className="flex-1 min-h-0 overflow-auto px-[12px] pt-0 pb-[12px]"><ChangeComparison request={request} index={index} visible={visible}/></div>
  </section>;
}
