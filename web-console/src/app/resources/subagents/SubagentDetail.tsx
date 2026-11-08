import type {SourceScope,SourceSettings} from '../../../../../protocol/app-server/v77';
import {useTranslation} from '../../../locale/react';
import type {ModelProfiles} from '../../settings/models/ModelsPage';
import {AgentEditor} from './AgentEditor';
import {ResourceAvailability} from '../shared/ResourceAvailability';
import {extensionEntries} from '../shared/inventory';
export function SubagentDetail(props:{source:SourceSettings;scope:SourceScope;revision?:string;name:string;models:string[];profiles:ModelProfiles}) {
 const tx=useTranslation();
 const entry=extensionEntries(props.source,props.scope,'agent').find(row=>row.name===props.name);
 return <section aria-label={tx('settings:extension-detail.agent-value',{p0:props.name})}>
  {entry?.diagnostics.map((message,index)=><p role="alert" key={index}>{message}</p>)}<AgentEditor {...props}/>
  {props.revision!==undefined&&entry&&<ResourceAvailability family="agent" source={props.source} scope={props.scope} revision={props.revision} name={props.name} valid={entry.valid}/>}
 </section>;
}
