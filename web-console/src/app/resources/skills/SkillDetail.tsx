import {useEffect,useRef,useState} from 'react';
import type {SourceScope,SourceSettings} from '../../../../../protocol/app-server/v78';
import type {AppServerClient} from '../../../client/app-server';
import {useTranslation} from '../../../locale/react';
import {DialogSurface} from '../../../presentation/primitives/DialogSurface';
import {IconCloseOutline16,IconSkillOutline16} from '../../../presentation/primitives/icons';
import modal from '../../../presentation/primitives/Modal.module.css';
import {MarkdownText} from '../../../presentation/markdown/MarkdownText';
import {splitFrontmatter} from '../../components/workbench-documents/frontmatter';
import css from './SkillDetail.module.css';

/** Read-only native instructions. There is intentionally no editing buffer,
 * file-write port, Save or Delete action on a Skill. */
export function SkillDetail({client,source,name,close}:{client:AppServerClient;source:SourceSettings;name:string;close:()=>void}) {
 const tx=useTranslation();
 const [content,setContent]=useState<{text:string;path:string;description:string;owner:SourceScope}>();
 const [error,setError]=useState('');
 const closeButton=useRef<HTMLButtonElement>(null);
 useEffect(()=>{
  setError('');setContent(undefined);
  let alive=true;
  const generation=client.getSnapshot().generation;
  const current=()=>alive&&client.getSnapshot().generation===generation;
  void (async()=>{
   let offset='0',revision:string|undefined,text='';
   const decoder=new TextDecoder();
   do {
    const {content}=await client.request({method:'skills/read',params:{target:source.target,name,kind:'instructions',offset,max_bytes:65536,expected_revision:revision}},'skill_content',undefined,current);
    if(!current())return;
    const bytes=Uint8Array.from(atob(content.data_base64),char=>char.charCodeAt(0));
    text+=decoder.decode(bytes,{stream:content.next_offset!=null});
    revision=content.revision;
    if(content.next_offset==null){
     const {location,description}=content.skill.catalog;
     setContent({text:splitFrontmatter(text)?.body??text,path:location,description,owner:content.skill.provenance.source});
     return;
    }
    offset=content.next_offset;
   } while(current());
  })().catch(error=>{if(current())setError(String(error));});
  return()=>{alive=false;};
 },[client,source,name]);
 return <DialogSurface open title={name} onClose={close} initialFocus={closeButton}
  overlayClassName={`${modal.root} ${modal.scrim} ${css.overlay}`} className={css.dialog}>
  <header className={css.header}>
   <IconSkillOutline16 size={20}/>
   <h2>{name}</h2>
   <button ref={closeButton} type="button" className={modal.close} aria-label={tx('resources:close')} onClick={close}><IconCloseOutline16/></button>
  </header>
  <div className={css.body}>
   {error ? <p role="alert">{error}</p> : content ? <>
    <p className={css.description}>{content.description}</p>
    <div className={css.metadata}>
     <span>{tx(content.owner === 'user' ? 'resources:scope.global' : 'resources:scope.workspace')}</span>
     <code>{content.path}</code>
    </div>
    <section className={css.instructions} aria-label={tx('resources:skills.instructions')}>
     <h3>{tx('resources:skills.instructions')}</h3>
     {content.text.trim() ? <MarkdownText text={content.text}/> : <p>{tx('resources:skills.empty-instructions')}</p>}
    </section>
   </> : <p role="status">{tx('resources:loading')}</p>}
  </div>
 </DialogSurface>;
}
