import { expect, it, vi } from 'vitest';
import type { GeometrySourceConsumer } from '../shared/geometry-source';
import { PreviewContentOwner, type PreviewReader } from '../src/client/preview-content';
const flush = async () => { for (let turn=0;turn<30;turn++)await Promise.resolve(); };
function fixture() {
  let finish!:()=>void;
  const geometry=vi.fn(async (signal:AbortSignal,consumer:GeometrySourceConsumer)=>{await consumer.start({bytes:3,version:'one'});await consumer.chunk(new Uint8Array([1,2,3]),0);await new Promise<void>((resolve,reject)=>{finish=resolve;signal.addEventListener('abort',()=>reject(signal.reason),{once:true});});return {version:'one'};});
  const probe=vi.fn(async()=> 'newer');
  const owner=new PreviewContentOwner({current:()=>true,immutable:false,geometry,probe,page:vi.fn(),bytes:vi.fn(),document:vi.fn()} as PreviewReader);
  owner.activate();owner.ensure('geometry');return {owner,geometry,probe,finish:()=>finish()};
}
it('opens no source until a decoder attaches and keeps completed live source identity across version hints', async()=>{
  const f=fixture(),source=f.owner.getSnapshot().source!;expect(f.geometry).not.toHaveBeenCalled();
  const pending=source.consume({start:()=>{},chunk:()=>{}},new AbortController().signal);await flush();expect(f.geometry).toHaveBeenCalledOnce();
  f.finish();await pending;await flush();expect(f.owner.getSnapshot().source).toBe(source);expect(f.owner.getSnapshot().version).toBe('one');
  f.owner.invalidate();await flush();expect(f.probe).toHaveBeenCalledOnce();expect(f.geometry).toHaveBeenCalledOnce();expect(f.owner.getSnapshot().source).toBe(source);
  f.owner.deactivate();expect(f.owner.getSnapshot().source).toBe(source);expect(f.owner.getSnapshot().data).toEqual(new Uint8Array([1,2,3]));f.owner.dispose();
});
it('actual body unmount cancels an unattached stream without opening any physical read', async()=>{
  const f=fixture();f.owner.deactivate();await flush();expect(f.geometry).not.toHaveBeenCalled();expect(f.owner.getSnapshot().source).toBeUndefined();f.owner.dispose();
});
it('a source hint retires an unfinished consumed stream and restarts only after its read settles', async()=>{
  const f=fixture(),source=f.owner.getSnapshot().source!;
  const pending=source.consume({start:()=>{},chunk:()=>{}},new AbortController().signal);const rejected=expect(pending).rejects.toBeDefined();await flush();
  f.owner.invalidate();await rejected;await flush();expect(f.geometry).toHaveBeenCalledOnce();expect(f.owner.getSnapshot().source).not.toBe(source);expect(f.owner.getSnapshot().revision).toBeGreaterThan(1);f.owner.dispose();
});
