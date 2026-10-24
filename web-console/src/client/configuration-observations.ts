import { materializeApplication } from '../../../protocol/app-server/application-observation';
import { materializeFields } from '../../../protocol/app-server/field-observation';
import type { BoundedSessionApplication, ConfigurationApplication, SourceFieldRef } from '../../../protocol/app-server/v57';
import type { ClientView, ConnectionRequest } from './contracts';

/** What the configuration observations read from and write to their client. */
export interface ConfigurationObservationPort {
  request: ConnectionRequest;
  state(): Readonly<ClientView>;
  current(generation: number): boolean;
  publish(patch: Partial<Pick<ClientView, 'configuration' | 'configurationObservation'>>): void;
}

/** The connection's finite cache of native configuration applications, one
 * per scope, advanced only by newer `configuration/changed` cuts and loaded
 * by bounded reads when a cut defers its value. */
export class ConfigurationObservations {
  constructor(private readonly port: ConfigurationObservationPort) {}
  private readonly cuts=new Map<string,BoundedSessionApplication>();
  private readonly reads=new Map<string,Promise<void>>();
  private readonly weights=new Map<string,number>();
  private retainConfiguration(application:ConfigurationApplication) {
    // Each source read is bounded; the aggregate cache must also stay finite.
    const bytes=new TextEncoder().encode(JSON.stringify(application)).byteLength;
    if(bytes>16*1024*1024)throw new Error('Configuration observation exceeds the local cache budget.');
    this.weights.delete(application.scope);this.weights.set(application.scope,bytes);
    const diagnostics={...this.port.state().configurationObservation};delete diagnostics[application.scope];
    this.port.publish({configuration:{...this.port.state().configuration,[application.scope]:application},configurationObservation:diagnostics});
    this.pruneConfiguration();
  }
  private pruneConfiguration() {
    const retired=new Set<string>();
    while(this.cuts.size>32){const scope=this.cuts.keys().next().value!;this.cuts.delete(scope);retired.add(scope);this.weights.delete(scope);}
    let bytes=[...this.weights.values()].reduce((sum,value)=>sum+value,0);
    while(this.weights.size>32||bytes>16*1024*1024){const scope=this.weights.keys().next().value!;bytes-=this.weights.get(scope)!;this.weights.delete(scope);this.cuts.delete(scope);retired.add(scope);}
    if(!retired.size)return;
    const configuration={...this.port.state().configuration},diagnostics={...this.port.state().configurationObservation};
    for(const scope of retired){delete configuration[scope];delete diagnostics[scope];}
    this.port.publish({configuration,configurationObservation:diagnostics});
  }

  private loadApplication(cut:BoundedSessionApplication,generation:number) {
    if(this.reads.has(cut.scope))return;
    const current=()=>this.port.current(generation)&&this.cuts.get(cut.scope)===cut;
    const load=async()=>{
      if(this.port.state().views[cut.scope])return materializeApplication(cut,async(reference,offset)=>(await this.port.request({method:'session/configurationValueRead',params:{session_id:cut.scope,reference,offset,max_bytes:65536}},'session_application_value_chunk',undefined,current)).chunk,current);
      const target=cut.sources[0];if(!target)throw new Error('Configuration source is unavailable.');
      const {projection}=await this.port.request({method:'configuration/sourcesRead',params:{target}},'source_settings',undefined,current);
      const application=projection.inline?.application ?? (await materializeFields<{application:ConfigurationApplication},SourceFieldRef>({revision:projection.revision,fields:projection.fields.filter(field=>field.name==='application')},async(reference,offset)=>(await this.port.request({method:'configuration/sourceValueRead',params:{target,reference,offset,max_bytes:65536}},'source_value_chunk',undefined,current)).chunk,current)).application;
      if(application?.version!==cut.version||application.scope!==cut.scope)throw new Error('Configuration source cut changed.');return application;
    };
    this.port.publish({configurationObservation:{...this.port.state().configurationObservation,[cut.scope]:{version:cut.version,phase:'loading'}}});
    const work=load().then(application=>{if(current()&&application){const diagnostics={...this.port.state().configurationObservation};delete diagnostics[cut.scope];this.port.publish({configurationObservation:diagnostics});this.retainConfiguration(application);}}).catch(error=>{if(current())this.port.publish({configurationObservation:{...this.port.state().configurationObservation,[cut.scope]:{version:cut.version,phase:'unavailable',error:String(error).slice(0,512)}}});}).finally(()=>{
      if(this.reads.get(cut.scope)!==work)return;this.reads.delete(cut.scope);
      const latest=this.cuts.get(cut.scope);if(latest&&latest!==cut)this.loadApplication(latest,generation);
    });
    this.reads.set(cut.scope,work);
  }
  /** A new connection observes every scope afresh. */
  connect() { this.cuts.clear(); this.weights.clear(); }
  /** One native `configuration/changed` cut; an older or equal version is ignored. */
  changed(cut: BoundedSessionApplication, generation: number) {
    const previous=this.cuts.get(cut.scope);
    if(previous&&BigInt(cut.version)<=BigInt(previous.version))return;
    this.cuts.set(cut.scope,cut);
    this.pruneConfiguration();
    if(cut.inline)this.retainConfiguration(cut.inline);
    else {
      const configuration={...this.port.state().configuration};delete configuration[cut.scope];this.port.publish({configuration,configurationObservation:{...this.port.state().configurationObservation,[cut.scope]:{version:cut.version,phase:'deferred'}}});
      this.loadApplication(cut,generation);
    }
  }
}
