/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import {useTranslation} from '../../../locale/react';
import type {SourceScope,SourceSettings} from '../../../../../protocol/app-server/v77';
import {Button} from '../../../presentation/primitives/Button';
import {AvailabilityDetail} from '../../resources/shared/AvailabilityDetail';
import type {PageFocus} from '../machines/navigation';
import {extensionEntries} from '../../resources/shared/inventory';
import css from '../../../presentation/settings/SettingsContent.module.css';
export function ExtensionDetail({source,scope,revision,name,onFocus}: {
 source:SourceSettings;scope:SourceScope;revision?:string;name:string;
 onFocus:(focus?:PageFocus['extensions'])=>void;
}) {
 const tx=useTranslation();
 const entry=extensionEntries(source,scope,'managed_python').find(row=>row.name===name);
 return <section><Button size="sm" onClick={()=>onFocus(undefined)}>{tx('settings:extension-detail.extensions')}</Button><h3>{name}</h3>
  <p>{tx('settings:extensions.manage-files')}</p>{entry?.path&&<p className={css.hint}>{entry.path}</p>}
  {entry?.diagnostics.map((reason,index)=><p key={index} role="alert">{reason}</p>)}
  {source.prospective_resources?.sources[`python:${name}`]?.status==='unavailable'&&<p role="alert">{tx('settings:extensions.unavailable')}</p>}
  <AvailabilityDetail source={source} scope={scope} revision={revision} family="managed_python" name={name}/>
 </section>;
}
