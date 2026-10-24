import { useEffect, useState } from 'react';
import type { DocumentDependencies } from './dependencies';
import { validateRaster } from '../../../client/raster';
import { useTranslation } from '../../../locale/react';
import { documentFailure } from '../documents/errors';
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
export function WorkspaceImage({path,alt,dependencies}: {path:string;alt:string;dependencies:DocumentDependencies}) {
  const tx = useTranslation(), [url,setUrl]=useState(''), [error, setError] = useState<unknown>();
  useEffect(()=>{
    const abort=new AbortController();let owned:string|undefined;
    setUrl(''); setError(undefined);
    void dependencies.read(path,abort.signal).then(result=>{
      if(abort.signal.aborted)return;
      validateRaster(result.data);
      owned=URL.createObjectURL(new Blob([result.data],{type:`image/${path.split('.').at(-1)}`}));setUrl(owned);
    }).catch(error=>{if(!abort.signal.aborted)setError(error);});
    return()=>{abort.abort();if(owned)URL.revokeObjectURL(owned);};
  },[path,dependencies]);
  return url?<img src={url} alt={alt} style={{maxWidth:'100%'}}/>:<span>{alt}{error !== undefined && <span role="alert">: {tx(`artifacts:document.${documentFailure(error)}`)}</span>}</span>;
}
