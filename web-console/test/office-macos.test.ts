// @vitest-environment node
import { mkdtemp, realpath, writeFile, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
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
  it.each(['docx','pptx'] as const)('native engine renders %s as a real PDF', async extension => {
    const bytes=await readFile(new URL(`./fixtures/documents/sample.${extension}`,import.meta.url));
    const pdf=await convertOffice(bytes,extension,new AbortController().signal);
    expect(pdf.subarray(0,5).toString()).toBe('%PDF-');
    expect(pdf.length).toBeGreaterThan(1000);
    expect(pdf.toString('latin1')).toMatch(/\/Type\s*\/Page\b/);
  },20000);
});
