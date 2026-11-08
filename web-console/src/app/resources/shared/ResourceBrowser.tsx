import {useState} from 'react';
import type {SourceScope,SourceSettings} from '../../../../../protocol/app-server/v77';
import type {AppServerClient} from '../../../client/app-server';
import {useTranslation} from '../../../locale/react';
import {Button} from '../../../presentation/primitives/Button';
import {DialogSurface} from '../../../presentation/primitives/DialogSurface';
import modal from '../../../presentation/primitives/Modal.module.css';
import {IconAgentPresetOutline16,IconCodeOutline16} from '../../../presentation/primitives/icons';
import {Search} from '../../settings/primitives/aria';
import type {ModelProfiles} from '../../settings/models/ModelsPage';
import {SubagentDetail} from '../subagents/SubagentDetail';
import {SkillDetail} from '../skills/SkillDetail';
import {useSkillCatalog} from '../skills/useSkillCatalog';
import {extensionEntries} from './inventory';
import {ResourceAvailability} from './ResourceAvailability';
import {CollectionDiagnostics} from './CollectionDiagnostics';
import css from '../ResourceSurface.module.css';
import cards from './ResourceCatalog.module.css';

export function ResourceBrowser({family,client,source,scope,revision,models,profiles}:{family:'skill'|'agent';client:AppServerClient;source:SourceSettings;scope:SourceScope;revision?:string;models:string[];profiles:ModelProfiles}) {
 const tx=useTranslation();
 const [query,setQuery]=useState(''),[focus,setFocus]=useState<string>(),[creating,setCreating]=useState(false),[name,setName]=useState('');
 const validName=/^[a-z][a-z0-9_-]{0,63}$/.test(name.trim());
 const skills=useSkillCatalog(client,source,family==='skill');
 const entries=extensionEntries(source,scope,family).map(entry=>({...entry,description:family==='skill'?skills?.skills.find(skill=>skill.catalog.name===entry.name)?.catalog.description??''
  :source.agents.find(agent=>agent.name===entry.name&&agent.scope===entry.owner)?.source.authored?.description??''}));
 // Skills discovery remains available even when rustx.toml cannot be resolved.
 if(family==='skill') for(const skill of skills?.skills??[]) if(!entries.some(row=>row.name===skill.catalog.name)) entries.push({family:'skill',name:skill.catalog.name,owner:skill.provenance.source,
  path:skill.catalog.location,relationship:skill.provenance.source===scope?'authored':'inherited',valid:true,diagnostics:[],capability:{family:'skill',inventory:'native',authoring:'none',selection:'skills',preparation:false,diagnostics:true},description:skill.catalog.description});
 const needle=query.trim().toLocaleLowerCase();
 const matches=entries.filter(row=>`${row.name} ${row.description}`.toLocaleLowerCase().includes(needle));
 const owned=matches.filter(row=>row.owner===scope), inherited=matches.filter(row=>row.owner!==scope);
 if(focus&&family==='agent')return <><Button size="sm" onClick={()=>setFocus(undefined)}>{tx('resources:back')}</Button><h2>{focus}</h2>
  <div className={css.detail}><SubagentDetail source={source} scope={scope} revision={revision} name={focus} models={models} profiles={profiles}/></div></>;
 const list=(rows:typeof entries)=><div role="list" className={cards.list}>{rows.map(row=><div role="listitem" key={row.name} aria-label={row.name} className={cards.card}>
  <button type="button" className={cards.open} aria-label={row.name} onClick={()=>setFocus(row.name)}><span className={cards.icon} aria-hidden="true">{family==='agent'?<IconAgentPresetOutline16/>:<IconCodeOutline16/>}</span>
   <span className={cards.content}><span className={cards.identity}>{row.name}</span>{(row.diagnostics[0]||row.description)&&<span className={cards.description}>{row.diagnostics[0]??row.description}</span>}</span></button>
  {revision!==undefined&&<ResourceAvailability family={family} name={row.name} valid={row.valid} source={source} scope={scope} revision={revision}/>}
 </div>)}</div>;
 return <>
  <div className={css.toolbar}><Search label={tx('resources:search')} placeholder={tx('resources:search')} value={query} onChange={setQuery}/>
   {family==='agent'&&<Button size="sm" variant="primary" onClick={()=>{setName('');setCreating(true);}}>{tx('resources:subagents.new')}</Button>}</div>
  {skills?.error&&<p role="alert">{skills.error}</p>}
  <div role="group" aria-label={tx('resources:scope.owned')} className={cards.group}>
   <div className={cards.groupHeader}><h3 className={cards.groupTitle}>{tx('resources:scope.owned')}</h3><span className={cards.count}>{owned.length}</span></div>
   {owned.length?list(owned):<p className={cards.empty}>{tx(query?'resources:no-results':'resources:empty')}</p>}
  </div>
  {!!inherited.length&&<div role="group" aria-label={tx('resources:scope.inherited')} className={cards.group}><div className={cards.groupHeader}><h3 className={cards.groupTitle}>{tx('resources:scope.inherited')}</h3><span className={cards.count}>{inherited.length}</span></div>{list(inherited)}</div>}
  <CollectionDiagnostics source={source} families={[family]}/>
  {family==='skill'&&focus&&<SkillDetail key={focus} client={client} source={source} name={focus} close={()=>setFocus(undefined)}/>}
  <DialogSurface open={creating} onClose={()=>setCreating(false)} title={tx('resources:subagents.new')} overlayClassName={`${modal.root} ${modal.scrim}`} className={modal.dialog}>
   <div className={modal.header}><h2>{tx('resources:subagents.new')}</h2><Button aria-label={tx('resources:close')} onClick={()=>setCreating(false)}>×</Button></div>
   <div className={modal.body}><label className={css.create}>{tx('resources:subagents.name')}<input autoFocus maxLength={64} value={name} aria-invalid={!!name&&!validName||undefined} onChange={event=>setName(event.target.value)}/></label>
    <p className={css.muted}>{tx('resources:subagents.name-help')}</p>
    {entries.some(row=>row.name===name.trim())&&<p role="alert">{tx('resources:subagents.duplicate')}</p>}</div>
   <div className={modal.footer}><Button variant="outline" onClick={()=>setCreating(false)}>{tx('resources:cancel')}</Button><Button variant="primary" disabled={!validName||entries.some(row=>row.name===name.trim())}
    onClick={()=>{setFocus(name.trim());setCreating(false);}}>{tx('resources:subagents.continue')}</Button></div>
  </DialogSurface>
 </>;
}
