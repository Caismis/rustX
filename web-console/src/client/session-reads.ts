import { materializeApplication } from '../../../protocol/app-server/application-observation';
import { sourceObservation } from '../../../protocol/app-server/source-observation';
import { materializeFields } from '../../../protocol/app-server/field-observation';
import { materializeEffective } from '../../../protocol/app-server/effective-observation';
import { domainValue, type DomainReader } from '../../../protocol/app-server/domain-observation';
import { materializeSettings } from '../../../protocol/app-server/settings-observation';
import type { AttachmentTarget } from '../../../protocol/app-server/v57';
import type { ConnectionRequest } from './contracts';

/** Bounded, exactly fenced native reads of Session domains, models, settings,
 * configuration and canonical content. Each read materializes its own native
 * cut through the connection's request pipeline and retains nothing. */
export class SessionReads {
  constructor(private readonly request: ConnectionRequest) {}
  domainReader(target: AttachmentTarget, current: () => boolean, agentId?: string): DomainReader {
    return {current,
      page:async(page,offset)=>(await this.request({method:'conversation/checkpointPage',params:{target,agent_id:agentId??null,cut:page.read_cut,domain:page.domain,offset,limit:32}},'checkpoint_page',undefined,current)).page,
      value:async(page,item,offset)=>(await this.request({method:'conversation/checkpointValueRead',params:{target,agent_id:agentId??null,cut:page.read_cut,domain:page.domain,item_offset:Number(item.offset),path:[],offset,max_bytes:65536}},'checkpoint_value_chunk',undefined,current)).chunk};
  }
  async modelRead(target: AttachmentTarget, current: () => boolean) {
    const {page}=await this.request({method:'session/model',params:{target}},'domain_page',undefined,current);
    return domainValue<import('../../../protocol/app-server/v57').SessionModelView>(page,this.domainReader(target,current));
  }
  async modelCatalogRead(target: AttachmentTarget, current: () => boolean, previous?: import('../../../protocol/app-server/v57').CheckpointPage) {
    const reader=this.domainReader(target,current);
    const page=previous?.next_offset!=null ? await reader.page(previous,Number(previous.next_offset)) : (await this.request({method:'session/models',params:{target}},'domain_page',undefined,current)).page;
    if (!current() || page.items.length > 32) throw new Error('Model catalog observation was replaced or exceeded its bound.');
    // A picker retains the finite published headers. Deferred native identities
    // and profile collections remain explicitly readable through their page;
    // opening this menu never downloads every growing catalog row.
    const models = page.items.flatMap(item => {
      const row = item.value as import('../../../protocol/app-server/v57').CatalogModelView | null;
      return row && typeof row.model === 'string' && row.model.length <= 256
        && (row.profiles?.length ?? 0) <= 32
        && (row.profiles ?? []).every(profile => typeof profile.id === 'string' && profile.id.length <= 256)
        && (row.defaultProfile == null || row.defaultProfile.length <= 256) ? [row] : [];
    });
    return {catalog:{models},page};
  }
  async sourceModelCatalogRead(source: import('../../../protocol/app-server/v57').SourceSettings, current: () => boolean, offset = 0) {
    const cut = sourceObservation(source);
    if (!cut || !current()) throw new Error('Model source observation is unavailable.');
    const {page} = await this.request({method:'configuration/sourcePage',params:{target:cut.target,revision:cut.revision,field:'session_models',offset,limit:32}},'source_page',undefined,current);
    if (page.revision !== cut.revision || page.field !== 'session_models' || page.items.length > 32) throw new Error('Model source page cut changed.');
    const models: import('../../../protocol/app-server/v57').CatalogModelView[] = [];
    for (const [index,item] of page.items.entries()) {
      if (Number(item.index) !== offset + index || !current()) throw new Error('Model source page failed to advance.');
      const value = await materializeFields<{model:import('../../../protocol/app-server/v57').CatalogModelView},import('../../../protocol/app-server/v57').SourceFieldRef>(
        {revision:cut.revision,fields:[{name:'model',value:item.value,reference:item.reference}]},
        async(reference,offset)=>(await this.request({method:'configuration/sourceValueRead',params:{target:cut.target,reference,offset,max_bytes:65536}},'source_value_chunk',undefined,current)).chunk,current);
      models.push(value.model);
    }
    if (page.next_offset != null && (page.items.length === 0 || page.next_offset !== offset + page.items.length)) throw new Error('Model source continuation did not advance.');
    return {catalog:{models},page};
  }
  async effectiveConfigurationRead(target: AttachmentTarget, current: () => boolean) {
    const {projection}=await this.request({method:'session/effectiveConfiguration',params:{target}},'effective_configuration',undefined,current);
    return materializeEffective(projection,async(reference,offset)=>(await this.request({method:'session/effectiveConfigurationValueRead',params:{target,reference,offset,max_bytes:65536}},'configuration_value_chunk',undefined,current)).chunk,current);
  }
  async readApplication(id:string,current:()=>boolean) {
    const {projection}=await this.request({method:'session/configuration',params:{session_id:id}},'session_configuration',undefined,current);
    return materializeApplication(projection,async(reference,offset)=>(await this.request({method:'session/configurationValueRead',params:{session_id:id,reference,offset,max_bytes:65536}},'session_application_value_chunk',undefined,current)).chunk,current);
  }
  async readSettings(id: string, current: () => boolean = () => true) {
    const {projection} = await this.request({method:'session/settings',params:{session_id:id}},'settings',undefined,current);
    const settings = await materializeSettings(projection, async(reference,offset)=>(await this.request({method:'session/settingsValueRead',params:{session_id:id,reference,offset,max_bytes:65_536}},'settings_value_chunk',undefined,current)).chunk, current);
    return {revision:projection.revision,settings};
  }
  async readContent(target: AttachmentTarget, agentId: string | undefined, content_ref: import('../../../protocol/app-server/v57').ContentRef, offset: number, current: () => boolean) {
    return (await this.request({ method: 'conversation/contentRead', params: { target, agent_id: agentId ?? null, content_ref, offset, max_bytes: 65_536 } }, 'content_chunk', undefined, current)).chunk;
  }
}
