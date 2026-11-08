import {useEffect,useState} from 'react';
import type {SourceSettings} from '../../../../../protocol/app-server/v77';
import type {AppServerClient} from '../../../client/app-server';
import {useTranslation} from '../../../locale/react';
import {DialogSurface} from '../../../presentation/primitives/DialogSurface';
import {Button} from '../../../presentation/primitives/Button';
import modal from '../../../presentation/primitives/Modal.module.css';
import {MarkdownText} from '../../../presentation/markdown/MarkdownText';
import css from '../ResourceSurface.module.css';

/** Read-only native instructions. There is intentionally no editing buffer,
 * file-write port, Save or Delete action on a Skill. */
export function SkillDetail({client,source,name,close}:{client:AppServerClient;source:SourceSettings;name:string;close:()=>void}) {
 const tx=useTranslation();
 const [content,setContent]=useState<{text:string;path:string}>();
 const [error,setError]=useState('');
 useEffect(()=>{
  setError('');setContent(undefined);
  let alive=true;
  const generation=client.getSnapshot().generation;
  const current=()=>alive&&client.getSnapshot().generation===generation;
  void (async()=>{
   let offset='0',revision:string|undefined,text='',path='';
   const decoder=new TextDecoder();
   do {
    const {content}=await client.request({method:'skills/read',params:{target:source.target,name,kind:'instructions',offset,max_bytes:65536,expected_revision:revision}},'skill_content',undefined,current);
    if(!current())return;
    const bytes=Uint8Array.from(atob(content.data_base64),char=>char.charCodeAt(0));
    text+=decoder.decode(bytes,{stream:content.next_offset!=null});
    revision=content.revision;path=content.skill.catalog.location;
    if(content.next_offset==null)break;
    offset=content.next_offset;
   } while(current());
   if(current())setContent({text,path});
  })().catch(error=>{if(current())setError(String(error));});
  return()=>{alive=false;};
 },[client,source,name]);
 return <DialogSurface open title={name} onClose={close} overlayClassName={`${modal.root} ${modal.scrim}`} className={modal.dialog}><div className={modal.header}><h2>{name}</h2><Button aria-label={tx('resources:close')} onClick={close}>×</Button></div><div className={modal.body}>
  <p className={css.muted}>{tx('resources:skills.read-only')}</p>
  {error?<p role="alert">{error}</p>:content?<><p className={css.path}>{content.path}</p><div className={css.skill} aria-label={tx('resources:skills.instructions')}><MarkdownText text={content.text}/></div></>:<p role="status">{tx('resources:loading')}</p>}
 </div></DialogSurface>;
}
