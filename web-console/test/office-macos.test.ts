// @vitest-environment node
import { watch } from 'node:fs';
import { mkdtemp, realpath, writeFile, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { unzipSync, zipSync, strFromU8, strToU8 } from 'fflate';
import { describe, it, expect } from 'vitest';
import { runMacOfficeSandbox } from '../host/documents/office-macos';
import { convertOffice } from '../host/documents/converter';

describe.skipIf(process.platform !== 'darwin')('real macOS engine and Seatbelt boundary', () => {
  it('denies outside content, network and inherited Host secrets', async () => {
    const root=await realpath(await mkdtemp(join(tmpdir(),'rustx-mac-probe-')));
    const outside=await realpath(await mkdtemp(join(tmpdir(),'rustx-mac-secret-')));
    const node=await realpath(process.execPath);
    const previous=process.env.RUSTX_OFFICE_SECRET;
    process.env.RUSTX_OFFICE_SECRET='must-not-cross';
    try {
      await writeFile(join(outside,'secret'),'private');
      const script=join(root,'probe.mjs');
      await writeFile(script,`import fs from 'node:fs';import net from 'node:net';
let denied=false;try{fs.readFileSync(${JSON.stringify(join(outside,'secret'))})}catch{denied=true}
const network=await new Promise(r=>{const s=net.createServer();s.once('error',()=>r(true));s.listen(0,'127.0.0.1',()=>s.close(()=>r(false)))});
process.stdout.write(JSON.stringify({denied,network,secret:process.env.RUSTX_OFFICE_SECRET??null}));`);
      const result=await runMacOfficeSandbox(root,[node,script],[resolve(dirname(node),'..')],new AbortController().signal);
      expect(JSON.parse(result.toString())).toEqual({denied:true,network:true,secret:null});
    } finally {
      if(previous===undefined)delete process.env.RUSTX_OFFICE_SECRET;else process.env.RUSTX_OFFICE_SECRET=previous;
      await rm(root,{recursive:true,force:true});await rm(outside,{recursive:true,force:true});
    }
  });
  it('cancellation closes the entire process group before settling its inherited output pipe', async () => {
    const root=await realpath(await mkdtemp(join(tmpdir(),'rustx-mac-cancel-')));
    const node=await realpath(process.execPath);
    const controller=new AbortController();
    let ready!:()=>void;
    const admitted=new Promise<void>(resolve=>{ready=resolve});
    const watcher=watch(root,(_event,name)=>{if(name==='ready')ready()});
    try {
      await writeFile(join(root,'child.mjs'), `import fs from 'node:fs';fs.writeFileSync('pending','ready');fs.renameSync('pending','ready');setInterval(()=>{},1000);`);
      await writeFile(join(root,'parent.mjs'), `import {spawn} from 'node:child_process';spawn(process.execPath,['child.mjs'],{stdio:'inherit'});setInterval(()=>{},1000);`);
      const work=runMacOfficeSandbox(root,[node,join(root,'parent.mjs')],[resolve(dirname(node),'..')],controller.signal);
      const outcome=work.then(()=>({error:new Error('Unexpected success')}),error=>({error}));
      await Promise.race([admitted, outcome.then(({error})=>{throw error})]);
      controller.abort();
      // close cannot fire while either process retains the inherited stdout pipe.
      expect((await outcome).error.message).toBe('obsolete');
    } finally { watcher.close();controller.abort();await rm(root,{recursive:true,force:true}); }
  });
  it.each(['docx','pptx'] as const)('native engine renders %s as a real PDF', async extension => {
    let bytes=await readFile(new URL(`./fixtures/documents/sample.${extension}`,import.meta.url));
    if(extension==='docx') {
      const archive=unzipSync(bytes);
      archive['word/document.xml']=strToU8(strFromU8(archive['word/document.xml']).replace('rustX document preview','rustX 中文文档预览'));
      bytes=Buffer.from(zipSync(archive));
    }
    const pdf=await convertOffice(bytes,extension,new AbortController().signal);
    expect(pdf.subarray(0,5).toString()).toBe('%PDF-');
    expect(pdf.length).toBeGreaterThan(1000);
    const { getDocument } = await import('pdfjs-dist/legacy/build/pdf.mjs');
    const task=getDocument({data:new Uint8Array(pdf),useSystemFonts:false});
    try {
      const document=await task.promise;
      const content=await(await document.getPage(1)).getTextContent();
      const text=content.items.map(item=>'str' in item?item.str:'').join(' ');
      expect(text).toContain(extension==='docx'?'rustX 中文文档预览':'rustX presentation preview');
    } finally { await task.destroy(); }
  },65000);
});
