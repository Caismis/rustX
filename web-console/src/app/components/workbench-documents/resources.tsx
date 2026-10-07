import { useEffect, useState } from 'react';
import type { WorkbenchCall } from '../WorkbenchFiles';
/** Resolve a document-relative reference without admitting a new workspace root. */
export function relativeDocumentPath(document: string, reference: string): string | undefined {
  if (!reference || /^(?:[a-z][a-z0-9+.-]*:|\/|#)/i.test(reference)) return undefined;
  let decoded: string;
  try { decoded=decodeURIComponent(reference.split(/[?#]/)[0]); } catch { return undefined; }
  if (decoded.includes('\0') || decoded.startsWith('/') || decoded.includes('\\')) return undefined;
  const parts=document.split('/').slice(0,-1);
  for(const part of decoded.split('/')) {
    if(part==='..') {if(!parts.length)return undefined;parts.pop();}
    else if(part && part!=='.')parts.push(part);
  }
  return parts.join('/') || undefined;
}
export function WorkspaceImage({path,alt,call}: {path:string;alt:string;call:WorkbenchCall}) {
  const [url,setUrl]=useState('');
  useEffect(()=>{
    const abort=new AbortController();let owned:string|undefined;
    void call({kind:'bytes',path},abort.signal).then(result=>{
      if(abort.signal.aborted||!result.base64)return;
      const bytes=Uint8Array.from(atob(result.base64),c=>c.charCodeAt(0));
      owned=URL.createObjectURL(new Blob([bytes],{type:path.toLowerCase().endsWith('.svg')?'image/svg+xml':`image/${path.split('.').at(-1)}`}));setUrl(owned);
    },()=>{});
    return()=>{abort.abort();if(owned)URL.revokeObjectURL(owned);};
  },[path,call]);
  return url?<img src={url} alt={alt} style={{maxWidth:'100%'}}/>:<span>{alt}</span>;
}
