import { useLayoutEffect, useMemo, useRef } from 'react';
import type { GeometryRoughPose, GeometryRoughPreview as RoughData } from '../../../client/geometry-rough';
import css from './GeometryRoughPreview.module.css';

type Pose = { yaw: number; pitch: number; zoom: number; x: number; y: number };
const initial = (): Pose => ({ yaw: Math.PI / 4, pitch: Math.asin(1 / Math.sqrt(3)), zoom: 1, x: 0, y: 0 });
// A projected coordinate envelope is deliberately separate from exact CAD
// geometry. Normalized coordinates keep this tiny paint/input path independent
// of the full 3D engine and all source magnitudes within finite arithmetic.
function envelope(points: number[][], pose: Pose) {
  const cy = Math.cos(pose.yaw), sy = Math.sin(pose.yaw), cp = Math.cos(pose.pitch), sp = Math.sin(pose.pitch);
  const projected = points.map(([x,y,z]) => [(x*cy-z*sy)*pose.zoom+pose.x, -(-x*sp*sy+y*cp-z*sp*cy)*pose.zoom+pose.y]).sort((a,b) => a[0]-b[0] || a[1]-b[1]);
  const cross = (a:number[],b:number[],c:number[]) => (b[0]-a[0])*(c[1]-a[1])-(b[1]-a[1])*(c[0]-a[0]);
  const chain = (input:number[][]) => { const output:number[][]=[]; for(const point of input){while(output.length>1&&cross(output.at(-2)!,output.at(-1)!,point)<=0)output.pop();output.push(point);}return output; };
  const lower=chain(projected),upper=chain([...projected].reverse());lower.pop();upper.pop();const hull=lower.concat(upper);
  return hull.length>=3 ? hull.map((p,i)=>`${i?'L':'M'}${p[0]},${p[1]}`).join(' ')+' Z' : '';
}
export function GeometryRoughPreview({ preview, copyId, current, signal, pose, interaction, labels }: { preview: RoughData; copyId: string; current:()=>boolean; signal?:AbortSignal; pose:{current:GeometryRoughPose|undefined}; interaction?:(active:boolean)=>void; labels:{rough:string;roughLoading:string;canvas:string} }) {
  const svg=useRef<SVGSVGElement>(null),path=useRef<SVGPathElement>(null),camera=useRef(initial()),key=useRef(copyId),latest=useRef(current);latest.current=current;
  const activity=useRef(interaction);activity.current=interaction;
  const drag=useRef<{x:number;y:number;button:number}>(undefined);
  if(key.current!==copyId){key.current=copyId;camera.current=initial();pose.current=undefined;drag.current=undefined;}
  const points=useMemo(()=>{
    if(preview.positions.length<9||preview.positions.length>1920||preview.positions.length%3)return [];
    const min=[Infinity,Infinity,Infinity],max=[-Infinity,-Infinity,-Infinity],values=preview.positions;
    for(let i=0;i<values.length;i++){if(!Number.isFinite(values[i])||Math.abs(values[i])>1e12)return [];const c=i%3;min[c]=Math.min(min[c],values[i]);max[c]=Math.max(max[c],values[i]);}
    const size=Math.max(...max.map((value,i)=>value-min[i]));if(!(size>1e-12))return [];
    return Array.from({length:values.length/3},(_,i)=>[0,1,2].map(c=>(values[i*3+c]-(min[c]+max[c])/2)/size*1.25));
  },[preview.positions]);
  const shape=envelope(points,camera.current);
  useLayoutEffect(()=>{
    const element=svg.current;if(!element||!path.current)return;let serial=0;
    const draw=(input=false)=>{
      if(!latest.current())return;
      const value=camera.current,cy=Math.cos(value.yaw),sy=Math.sin(value.yaw),cp=Math.cos(value.pitch),sp=Math.sin(value.pitch);
      if(input)pose.current={direction:[sy*cp,sp,cy*cp],target:[(-value.x*cy-value.y*sp*sy)/1.25,value.y*cp/1.25,(value.x*sy-value.y*sp*cy)/1.25],zoom:value.zoom};
      path.current!.setAttribute('d',envelope(points,value));element.dataset.geometryRoughRender=String(++serial);
    };
    const start=(event:PointerEvent)=>{if(!latest.current())return;drag.current={x:event.clientX,y:event.clientY,button:event.button};activity.current?.(true);element.setPointerCapture(event.pointerId);element.focus();};
    const move=(event:PointerEvent)=>{const down=drag.current;if(!down||!latest.current())return;const width=Math.max(1,element.clientWidth),height=Math.max(1,element.clientHeight),dx=event.clientX-down.x,dy=event.clientY-down.y;down.x=event.clientX;down.y=event.clientY;
      if(down.button===2){camera.current.x+=dx/width*2;camera.current.y+=dy/height*2;}
      else{camera.current.yaw-=dx/width*6;camera.current.pitch=Math.max(-1.4,Math.min(1.4,camera.current.pitch+dy/height*6));}draw(true);};
    const end=()=>{if(!drag.current)return;drag.current=undefined;activity.current?.(false);};const wheel=(event:WheelEvent)=>{if(!latest.current())return;event.preventDefault();activity.current?.(!!drag.current);camera.current.zoom=Math.max(.05,Math.min(20,camera.current.zoom*Math.exp(-event.deltaY*.001)));draw(true);};
    const context=(event:Event)=>event.preventDefault();
    const keyboard=(event:KeyboardEvent)=>{if(!latest.current())return;const directions:Record<string,[number,number]>={ArrowLeft:[.04,0],ArrowRight:[-.04,0],ArrowUp:[0,.04],ArrowDown:[0,-.04]};const delta=directions[event.code];if(!delta)return;event.preventDefault();activity.current?.(!!drag.current);camera.current.x+=delta[0];camera.current.y+=delta[1];draw(true);};
    const abort=()=>{element.style.display='none';drag.current=undefined;};signal?.addEventListener('abort',abort,{once:true});element.style.display='';if(signal?.aborted)abort();
    element.addEventListener('pointerdown',start);element.addEventListener('pointermove',move);element.addEventListener('pointerup',end);element.addEventListener('pointercancel',end);element.addEventListener('lostpointercapture',end);element.addEventListener('wheel',wheel,{passive:false});element.addEventListener('contextmenu',context);element.addEventListener('keydown',keyboard);draw();
    return()=>{signal?.removeEventListener('abort',abort);element.removeEventListener('pointerdown',start);element.removeEventListener('pointermove',move);element.removeEventListener('pointerup',end);element.removeEventListener('pointercancel',end);element.removeEventListener('lostpointercapture',end);element.removeEventListener('wheel',wheel);element.removeEventListener('contextmenu',context);element.removeEventListener('keydown',keyboard);};
  },[points,copyId,pose,signal]);
  if(!shape||!current()||signal?.aborted)return null;
  return <div className={css.root}><svg ref={svg} role="img" aria-label={labels.canvas} tabIndex={0} viewBox="-1 -1 2 2" data-geometry-rough={preview.version}><path ref={path} d={shape} className={css.surface} vectorEffect="non-scaling-stroke"/></svg><p className={css.status} role="status">{labels.rough}<span>{labels.roughLoading}</span></p></div>;
}
