/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import type {SourceSettings,SourceScope,SourceToolSelection} from '../../../../../protocol/app-server/v79';
import {useTranslation} from '../../../locale/react';
import {UnitForm} from '../../settings/forms/bridge';
import {Selection} from '../../settings/forms/controls';
import {ConfigUnavailable} from '../../settings/forms/ConfigUnavailable';
import {toolSourceId} from '../../settings/capability';
/** Fine-grained tool grants for the root Agent. Permission mode overrides for
 * MCP live on the server definition (`McpPermissionDetail`), not here. */
export function AvailabilityDetail({source,scope,revision,family,name}:{source:SourceSettings;scope:SourceScope;revision?:string;family:'mcp'|'managed_python';name:string}) {
 const tx=useTranslation();
 if(revision===undefined)return <ConfigUnavailable/>;
 const document=(scope==='user'?source.user:source.workspace)?.authored;
 const id=toolSourceId(family,name);
 return <section aria-label={tx('settings:extension-detail.root-agent-availability')}><h4>{tx('settings:extension-detail.availability-to-the-root-agent')}</h4>
  <UnitForm<SourceToolSelection> title={tx('settings:extension-detail.source-value',{p0:id})} authored={document?.agent?.tools?.sources?.[id]??undefined} blank={[]} revision={revision}
   mutation={authored=>({kind:'config',mutation:{unit:'source_tools',id,authored}})}>{(value,change)=><Selection label={id} value={value} change={change}/>}</UnitForm>
 </section>;
}
