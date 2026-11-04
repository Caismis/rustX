import { useLayoutEffect, useRef, useState, type CSSProperties } from 'react';

/** Expand an editor inside the inspector while retaining its DOM and scroll slot. */
export function useInspectorExpansion() {
  const anchor=useRef<HTMLDivElement>(null),[expanded,setExpanded]=useState(false),[collapsedHeight,setCollapsedHeight]=useState(0),[bounds,setBounds]=useState<CSSProperties>();
  useLayoutEffect(()=>{
    if(!expanded)return;
    const body=anchor.current?.closest<HTMLElement>('[data-workflow-inspector-body]');if(!body)return;
    const measure=()=>{const box=body.getBoundingClientRect(),viewport=window.visualViewport,bottom=viewport?viewport.offsetTop+viewport.height:window.innerHeight;setBounds({top:box.top,left:box.left,width:box.width,height:Math.max(64,Math.min(box.bottom,bottom)-box.top)});};
    measure();const observer=new ResizeObserver(measure);observer.observe(body);
    window.addEventListener('resize',measure);window.visualViewport?.addEventListener('resize',measure);
    return()=>{observer.disconnect();window.removeEventListener('resize',measure);window.visualViewport?.removeEventListener('resize',measure);};
  },[expanded]);
  const toggle=()=>{if(!expanded)setCollapsedHeight(anchor.current?.getBoundingClientRect().height??0);setExpanded(value=>!value);};
  return {anchor,expanded,setExpanded,toggle,collapsedHeight,bounds};
}
