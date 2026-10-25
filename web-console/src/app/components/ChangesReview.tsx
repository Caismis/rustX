import { useEffect, useLayoutEffect, useState, useSyncExternalStore } from 'react';
import type { FileComparison } from '../../../../protocol/app-server/v59';
import type { ChangesReviewRequest } from '../changes-review';
import { useTranslation } from '../../locale/react';
import { DiffBlock } from '../../presentation/primitives/DiffBlock';
import { Button } from '../../presentation/primitives/Button';
import css from './ChangesReview.module.css';

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
  return <div className={css.comparison} data-change-comparison>
    {preview && <p className={css.path}>{file.display}</p>}
    {state.error ? <p role="alert">{tx('artifacts:changes.error')} <Button size="sm" onClick={()=>setRetry(value=>value+1)}>{tx('artifacts:changes.retry')}</Button></p>
      : comparison===undefined ? <p role="status">{tx('artifacts:changes.loading')}</p>
      : comparison===null ? <p>{tx('artifacts:changes.missing')}</p>
      : comparison.binary ? <p>{tx('artifacts:changes.binary-diff')}</p>
      : comparison.oversized ? <p>{tx('artifacts:changes.oversized-diff')}</p>
      : <DiffBlock diffs={comparison.hunks.map(hunk=>({path:`${file.display}:${hunk.new_start}`,oldText:hunk.old_text,newText:hunk.new_text}))} maxLines={preview?16:500} labels={{
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
    <label className={css.selector}>{tx('artifacts:changes.select-file')}<select value={index} onChange={event=>setIndex(Number(event.currentTarget.value))}>{request.summary.files.map((file,i)=><option key={file.path} value={i}>{file.display} (+{file.added} -{file.deleted})</option>)}</select></label>
    <div className={css.body}><ChangeComparison request={request} index={index} visible={visible}/></div>
  </section>;
}
