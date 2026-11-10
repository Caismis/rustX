import { useEffect, useLayoutEffect, useMemo, useState, useSyncExternalStore } from 'react';
import type { FileComparison } from '../../../../protocol/app-server/v78';
import type { ChangesReviewRequest } from '../changes-review';
import { useTranslation } from '../../locale/react';
import { DiffBlock, localDiffHunks } from '../../presentation/primitives/DiffBlock';
import { languageForPath } from '../../presentation/primitives/file-language';
import { grammarLoadCount, highlightLines, subscribeGrammarLoaded } from '../../presentation/markdown/highlight';
import { Button } from '../../presentation/primitives/Button';
import css from './ChangesReview.module.css';

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
  return <div className={css.previewRows} data-review-view="unified">
    {rows.map((section, index) => <div className={css.previewHunk} key={index}>
      {section.map(({kind, old, next, text}, row) => {
        const spans = old === undefined ? highlights[index]?.next?.[next!] : highlights[index]?.old?.[old];
        return <div className={css.previewLine} data-diff-line={kind} key={row}>
          <span className={css.number}>{old === undefined ? '' : hunks[index]!.old_start + old}</span>
          <span className={css.number}>{next === undefined ? '' : hunks[index]!.new_start + next}</span>
          <span className={css.sign}>{kind === 'add' ? '+' : kind === 'del' ? '-' : ' '}</span>
          <span className={css.code}>{spans?.map((span, i) => <span key={i} style={span.style}>{span.text}</span>) ?? text}</span>
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
  if(!current)return <p>{tx('artifacts:changes.missing')}</p>;
  return <div className={preview ? css.preview : css.comparison} data-change-comparison data-changes-hover-preview={preview || undefined}>
    {preview && <div className={css.previewHeader}><span className={css.path} data-changes-preview-path>{file.path}</span></div>}
    {state.error ? <p role="alert">{tx('artifacts:changes.error')} <Button size="sm" onClick={()=>setRetry(value=>value+1)}>{tx('artifacts:changes.retry')}</Button></p>
      : comparison===undefined ? <p role="status">{tx('artifacts:changes.loading')}</p>
      : comparison===null ? <p>{tx('artifacts:changes.missing')}</p>
      : comparison.binary ? <p>{tx('artifacts:changes.binary-diff')}</p>
      : comparison.oversized ? <p>{tx('artifacts:changes.oversized-diff')}</p>
      : preview ? comparison.hunks.length > 0 && <PreviewDiff hunks={comparison.hunks} path={file.path}/>
      : <DiffBlock diffs={comparison.hunks.map(hunk=>({path:`${file.display}:${hunk.new_start}`,oldText:hunk.old_text,newText:hunk.new_text}))} maxLines={500} labels={{
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
  return <section className={css.root} aria-label={tx('artifacts:changes.open-review')} data-changes-review>
    <label className={css.selector}>{tx('artifacts:changes.select-file')}<select value={index} onChange={event=>setIndex(Number(event.currentTarget.value))}>{request.summary.files.map((file,i)=><option key={i} value={i}>{file.display} (+{file.added} -{file.deleted})</option>)}</select></label>
    <div className={css.body}><ChangeComparison request={request} index={index} visible={visible}/></div>
  </section>;
}
