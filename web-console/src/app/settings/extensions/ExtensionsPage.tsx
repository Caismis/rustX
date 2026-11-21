import {useState, type ReactNode} from 'react';
import {useTranslation} from '../../../locale/react';
import type {SourceScope,SourceSettings} from '../../../../../protocol/app-server/v80';
import {Button} from '../../../presentation/primitives/Button';
import {extensionEntries} from '../../resources/shared/inventory';
import {SettingsNotice} from '../SettingsFeedback';
import {NativeExtensions} from './NativeExtensions';
import {ExtensionDetail} from './ExtensionDetail';
import type {PageFocus} from '../machines/navigation';
import {FilterTabs,Search} from '../primitives/aria';
import {SettingsPageShell,SettingsGroup} from '../../../presentation/settings/SettingsPageShell';
import {PageTransactionFooter} from '../forms/bridge';
import css from '../../../presentation/settings/SettingsContent.module.css';

export type ExtensionFilter='all'|'managed_python'|'native';

/** The Extensions product page.
 *
 * The native extension surface is deliberately thin: managed Python
 * inventories — code the user shipped as declarative resource files — and
 * the native extensions that are semantic units of `rustx.toml`. Native
 * provides exactly these two families; the list below reflects both
 * faithfully. */
export function ExtensionsPage({source,scope,revision,focus,onFocus,filter,onFilter,refresh,refreshing,scopeControl}:{
 source:SourceSettings;scope:SourceScope;revision?:string;
 focus?:PageFocus['extensions'];onFocus:(focus?:PageFocus['extensions'])=>void;
 filter:ExtensionFilter;onFilter:(filter:ExtensionFilter)=>void;
 refresh?:()=>void;refreshing?:boolean;scopeControl?:ReactNode;
}) {
 const tx=useTranslation();
 const [query,setQuery]=useState('');
 if(focus)return <ExtensionDetail source={source} scope={scope} revision={revision} name={focus.name} onFocus={onFocus}/>;
 const entries=extensionEntries(source,scope,'managed_python').filter(row=>row.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()));
 return <SettingsPageShell className={`${css.settings} ${css.page}`} title={tx('settings:extensions-page.extensions')}
  toolbar={<>{scopeControl}<Search label={tx('settings:extensions-page.find-an-extension')} value={query} onChange={setQuery}/>
    <Button variant="toolbar" size="sm" onClick={refresh} disabled={refreshing}>{tx('settings:catalog.refresh')}</Button></>}
  footer={<PageTransactionFooter/>}>
  <FilterTabs label={tx('settings:extensions-page.extension-kinds')} value={filter} onChange={onFilter} options={[
   ['all',tx('settings:copy.all')],['managed_python',tx('settings:copy.python')],['native',tx('settings:copy.native')],
  ]}>
   {filter!=='managed_python'&&<SettingsGroup title={tx('settings:native-extensions.native-extensions')}>
    <NativeExtensions source={source} scope={scope} revision={revision}/>
   </SettingsGroup>}
   {filter!=='native'&&<SettingsGroup title={tx('settings:extensions.managed-python')}>
    {entries.map(entry=><section key={entry.name} className={css.card}>
     <div className={css.rowHead}><strong>{entry.name}</strong>
      <span className={css.badge} data-tone={entry.diagnostics.length?'error':entry.relationship==='inherited'?'success':undefined}>{entry.relationship}</span>
      <Button size="sm" onClick={()=>onFocus({kind:'extension',family:'managed_python',name:entry.name})}>{entry.name} →</Button>
     </div>
     {!!entry.diagnostics.length&&<SettingsNotice text={entry.diagnostics[0]}/>}
    </section>)}
    {!entries.length&&<p className={css.hint}>{tx('settings:catalog.empty')}</p>}
   </SettingsGroup>}
  </FilterTabs>
 </SettingsPageShell>;
}
