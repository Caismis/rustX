import {useState,type ReactNode} from 'react';
import type {SourceScope,SourceSettings} from '../../../../../protocol/app-server/v77';
import {useTranslation} from '../../../locale/react';
import {Button} from '../../../presentation/primitives/Button';
import {FilterTabs,Search} from '../primitives/aria';
import type {PageFocus} from '../machines/navigation';
import {extensionEntries} from '../../resources/shared/inventory';
import {ExtensionDetail} from './ExtensionDetail';
import {NativeExtensions} from './NativeExtensions';
import css from '../../resources/shared/ResourceCatalog.module.css';
export type ExtensionFilter='all'|'managed_python'|'native';
/** Runtime extensions left in Settings; skills, agents, MCP and workflows each
 * have their own application module. */
export function ExtensionsPage({source,scope,revision,focus,onFocus,filter,onFilter,scopeControl,refresh,refreshing}:{
 source:SourceSettings;scope:SourceScope;revision?:string;focus?:PageFocus['extensions'];
 onFocus:(focus?:PageFocus['extensions'])=>void;filter:ExtensionFilter;onFilter:(filter:ExtensionFilter)=>void;scopeControl:ReactNode;refresh:()=>void;refreshing:boolean;
}) {
 const tx=useTranslation();const [query,setQuery]=useState('');
 if(focus)return <ExtensionDetail source={source} scope={scope} revision={revision} name={focus.name} onFocus={onFocus}/>;
 const entries=extensionEntries(source,scope,'managed_python').filter(row=>row.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
 return <section className={css.catalog} aria-label={tx('settings:extensions-page.extensions')}>
  <h3 className={css.title}>{tx('settings:extensions-page.extensions')}</h3>
  <div className={css.header}>{scopeControl}<div className={css.search}><Search label={tx('settings:extensions-page.find-an-extension')} value={query} onChange={setQuery}/></div><Button size="sm" variant="outline" onClick={refresh} disabled={refreshing}>{tx('settings:catalog.refresh')}</Button></div>
  <FilterTabs label={tx('settings:extensions-page.extension-kinds')} value={filter} onChange={onFilter} options={[
   ['all',tx('settings:copy.all')],['managed_python',tx('settings:copy.python')],['native',tx('settings:copy.native')],
  ]}>
   {filter!=='managed_python'&&<NativeExtensions source={source} scope={scope} revision={revision}/>}
   {filter!=='native'&&<><div role="list" className={css.list}>{entries.map(entry=><div role="listitem" aria-label={entry.name} key={entry.name}><button type="button" className={css.card} onClick={()=>onFocus({kind:'extension',family:'managed_python',name:entry.name})}>{entry.name}</button></div>)}</div>
    {!entries.length&&<p className={css.empty}>{tx('settings:catalog.empty')}</p>}</>}
  </FilterTabs>
 </section>;
}
