import {useEffect,useRef,useState,useSyncExternalStore} from 'react';
import {useSelector} from '@xstate/react';
import {Tabs,TabList,Tab,TabPanel} from 'react-aria-components';
import type {AppServerClient} from '../../client/app-server';
import type {ProductHostWorkspaces} from '../../workspaces/host';
import type {WorkspaceAuthority} from '../../workspaces/authority';
import type {WorkspaceAssociations} from '../../workspaces/associations';
import {useTranslation} from '../../locale/react';
import {Button} from '../../presentation/primitives/Button';
import {Menu} from '../../presentation/primitives/Menu';
import {IconChevronDownOutline14,IconFolderClose16} from '../../presentation/primitives/icons';
import {SettingsActorContext,useSettingsTarget} from '../settings/machines/react';
import {SourceContext} from '../settings/source-context';
import {catalogIdentities,catalogProfiles,configAuthoring,settingsTargetKey,userSettingsTarget,workspaceSettingsTarget} from '../settings/projection';
import {McpPage} from './mcp/McpPage';
import {useMcpProbes} from './mcp/useMcpProbes';
import {ResourceBrowser} from './shared/ResourceBrowser';
import type {ResourceModule,McpFocus} from './types';
import settings from '../../presentation/settings/SettingsContent.module.css';
import css from './ResourceSurface.module.css';

interface Props {module:ResourceModule;visible:boolean;client:AppServerClient;host:ProductHostWorkspaces;authority:WorkspaceAuthority;associations:WorkspaceAssociations;initialWorkspace?:string}
/** Module presentation is retained; configuration transactions belong to the
 * shared source actor and settle even after their module or scope is left. */
export function ResourceSurface(props:Props) {
 const [visited,setVisited]=useState(props.visible);
 if(props.visible&&!visited)setVisited(true);
 return (visited||props.visible)&&<ResourcePage {...props}/>;
}
function ResourcePage({module,visible,client,host,authority,associations,initialWorkspace}:Props) {
 const tx=useTranslation();
 const catalog=useSyncExternalStore(authority.subscribe,authority.getCatalog);
 useEffect(()=>{if(visible)void authority.observe().catch(()=>{});},[authority,visible]);
 const [tab,setTab]=useState<'workspace'|'global'>(initialWorkspace?'workspace':'global');
 const [workspaceId,setWorkspace]=useState(initialWorkspace);
 const chosen=useRef(!!initialWorkspace);
 const workspaces=catalog?.workspaces??[];
 useEffect(()=>{if(!chosen.current&&workspaces.length){chosen.current=true;setWorkspace(workspaces[0].id);}},[workspaces]);
 const workspace=workspaces.find(row=>row.id===workspaceId);
 const target=tab==='workspace'&&workspaceId?workspaceSettingsTarget(workspaceId,workspace?.displayName??workspaceId):userSettingsTarget;
 const available=tab==='global'||!!workspace;
 const {actor,transport}=useSettingsTarget(client,target,host,visible&&available);
 const observation=useSelector(actor,state=>state.context.observation);
 const source=useSelector(actor,state=>state.context.observation??state.context.staleObservation);
 const readError=useSelector(actor,state=>state.context.readError);
 const maintenanceError=useSelector(actor,state=>state.context.maintenanceError);
 const convergenceError=useSelector(actor,state=>state.context.convergenceError);
 const busy=useSelector(actor,state=>state.matches({mutation:'submitting'})||state.matches({maintenance:'reconciling'}));
 const [menu,setMenu]=useState(false);
 const [focus,setFocus]=useState<McpFocus>();
 const probes=useMcpProbes(client,host,target,observation,visible&&available&&module==='mcp'&&!focus,associations);
 const scope=target.kind;
 const config=configAuthoring(source,scope);
 const refresh=()=>actor.send({type:'REFRESH'});
 const scopeKey=settingsTargetKey(target);
 const title=tx(`resources:${module}.title`);
 const scopeLabel=tab==='global'?tx('resources:scope.global'):workspace?.displayName;
 return <section className={`primary-page ${css.surface}`} aria-label={title} data-resource-module={module} data-lifecycle={available&&observation&&!readError?'ready':readError?'failed':'loading'} hidden={!visible} inert={!visible}>
  <div className={css.inner}>
   <header className={css.head}><h1>{title}</h1><p>{tx(`resources:${module}.description`)}</p></header>
   <Tabs selectedKey={tab} onSelectionChange={key=>{setFocus(undefined);setTab(key as typeof tab);}}>
   <TabList className={css.tabs} aria-label={tx('resources:scope.label')}>
    {(['workspace','global'] as const).map(value=><Tab key={value} id={value} className={css.tab}>{tx(`resources:scope.${value}`)}</Tab>)}
   </TabList>
   <TabPanel id={tab}>
   <div className={css.scope}>
    {tab==='workspace'?<Menu open={menu} autoFocus onClose={()=>setMenu(false)} selectedId={workspaceId}
     items={workspaces.map(row=>({id:row.id,label:row.displayName,icon:<IconFolderClose16/>}))}
     onSelect={id=>{chosen.current=true;setWorkspace(id);setFocus(undefined);setMenu(false);}}
     anchor={<Button size="sm" variant="outline" aria-label={tx('resources:scope.choose')} aria-haspopup="menu" aria-expanded={menu} onClick={()=>setMenu(value=>!value)}><IconFolderClose16/>{workspace?.displayName??tx('resources:scope.choose')}<IconChevronDownOutline14/></Button>}/>:<span className={css.muted}>{tx('resources:scope.global')}</span>}
    <Button size="sm" variant="outline" disabled={!available||busy||transport.connection!=='connected'} onClick={refresh}>{tx('resources:refresh')}</Button>
   </div>
   {!available?<p className={css.state} role="status">{tx(workspaceId?'resources:scope.unavailable':'resources:scope.empty')}</p>:<>
    {[readError,maintenanceError,convergenceError].filter(Boolean).map((error,index)=><p role="alert" key={index}>{error}</p>)}
    {!source?<p className={css.state} role="status">{tx('resources:loading')}</p>:<SettingsActorContext value={actor}><SourceContext value={source}>
     <div className={`${settings.settings} ${css.body}`} key={`${transport.endpoint}|${transport.authorityRevision}|${scopeKey}`}>
      <fieldset className={css.editor} disabled={!observation||!!readError||transport.connection!=='connected'}>
       {module==='mcp'?<McpPage source={source} scope={scope} revision={config.state==='structured'?config.revision:undefined} focus={focus} onFocus={setFocus}
        scopeControl={<span>{scopeLabel}</span>} probeStates={probes.states} testConnection={probes.testConnection} probeAvailable={probes.available}/>
        :<ResourceBrowser family={module==='skills'?'skill':'agent'} client={client} source={source} scope={scope} revision={config.state==='structured'?config.revision:undefined}
         models={catalogIdentities(source,scope,'models')} profiles={catalogProfiles(source,scope)}/>}
      </fieldset>
     </div>
    </SourceContext></SettingsActorContext>}
   </>}
   </TabPanel></Tabs>
  </div>
 </section>;
}
