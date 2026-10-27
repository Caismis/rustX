import {describe,it,expect} from 'vitest';
import {readFileSync} from 'node:fs';
import {convertXlsx} from '../src/app/components/workbench-documents/excel/xlsx';
import {viewersForPath} from '../src/app/components/workbench-documents/definitions';
import {relativeDocumentPath} from '../src/app/components/workbench-documents/resources';
import {createBasicHtmlDocument} from '../src/app/components/workbench-documents/basic-document';
import {splitFrontmatter} from '../src/app/components/workbench-documents/frontmatter';
import {readSidebarLayout,writeSidebarLayout} from '../src/app/components/workbench-persistence';
import {DockController} from '../src/presentation/dockkit/engine/controller';
describe('Harness document defaults',()=>{
 it.each([['note.MD','markdown'],['note.markdown','markdown'],['page.htm','html'],['photo.png','image'],['photo.svg','code'],['slides.pdf','pdf'],['report.docx','office'],['book.xlsx','excel'],['data.csv','excel'],['main.py','code'],['README','text']])('%s selects %s',(path,viewer)=>expect(viewersForPath(path)[0]).toBe(viewer));
 it.each(['book.xls','old.doc','old.ppt','video.mp4','archive.zip','icon.ico'])('%s is unsupported before acquiring content', path => expect(viewersForPath(path)).toEqual([]));
 it('offers source only for text documents',()=>{
   expect(viewersForPath('image.svg')).toContain('text');
   expect(viewersForPath('note.md')).toEqual(['markdown','code','text']);
   for(const path of ['image.png','file.pdf','book.xlsx','file.docx'])expect(viewersForPath(path)).not.toContain('text');
 });
 it('splits only complete leading frontmatter',()=>{
   expect(splitFrontmatter('---\ntitle: Example\n---\n# Body')).toEqual({source:'title: Example',body:'# Body'});
   expect(splitFrontmatter('text\n---\n# Body')).toBeUndefined();
   expect(splitFrontmatter('---\ntitle: incomplete')).toBeUndefined();
 });
 it('resolves document resources without escaping the workspace',()=>{
   expect(relativeDocumentPath('docs/readme.md','../images/example.svg')).toBe('images/example.svg');
   for(const ref of ['../../secret','/%2e%2e/secret','file:///secret','https://example.com/a','%00','%2fetc/passwd'])expect(relativeDocumentPath('docs/readme.md',ref)).toBeUndefined();
 });
 it('keeps HTML styles while forbidding script, network, frames and base changes',()=>{
   const result=createBasicHtmlDocument(new TextEncoder().encode('<style>h1{color:red}</style><h1>Title</h1><script>alert(1)</script><base href="https://example.com"><iframe src="https://example.com"></iframe>'));
   expect(result).toContain('h1{color:red}');expect(result).toContain("script-src 'none'");
   expect(result).not.toContain('<script');expect(result).not.toContain('<base');expect(result).not.toContain('<iframe');
 });
 it('renders workbook defaults as finite geometry and preserves cached formula values',async()=>{
   const bytes=Uint8Array.from(readFileSync('test/fixtures/documents/sample.xlsx'));
   const preview=await convertXlsx(bytes,{maxBytes:16*1024*1024,maxCells:200000,timeoutMs:15000});
   for(const sheet of preview.sheets) {
     expect(sheet.defaultRowHeight === undefined || Number.isFinite(sheet.defaultRowHeight)).toBe(true);
     expect(sheet.celldata?.length).toBeGreaterThan(0);
   }
   expect(preview.sheets[0].celldata?.some(cell=>cell.v?.v===99)).toBe(true);
 });
 it.each([{mode:['push']},{expanded:'true'},{floats:['pane0']},{nodes:null}])('rejects malformed persisted layout shapes: %j',patch=>{
   const layout=new DockController().getSnapshot().state;
   localStorage.setItem('rustx.workbench-layout.v1.invalid',JSON.stringify({bySession:{invalid:{layout:{...layout,...patch},minted:100}}}));
   expect(readSidebarLayout('invalid')).toBeUndefined();
   expect(localStorage.getItem('rustx.workbench-layout.v1.invalid')).toBeNull();
 });
 it('restores validated layout metadata and rejects dangling or cyclic references',()=>{
   const controller=new DockController();controller.openContent({kind:'file',title:'docs/readme.md',contentId:'file:docs/readme.md'});
   const layout=controller.getSnapshot().state;
   writeSidebarLayout('test',{layout,minted:100,history:{entries:[],cursor:0}});
   expect(readSidebarLayout('test')?.layout).toEqual(layout);
   localStorage.setItem('rustx.workbench-layout.v1.test',JSON.stringify({bySession:{test:{layout:{...layout,rootId:'pane999'},minted:100}}}));
   expect(readSidebarLayout('test')).toBeUndefined();expect(localStorage.getItem('rustx.workbench-layout.v1.test')).toBeNull();
 });
 it('persists ordinary tabs while omitting runtime-bound changed-file reviews and their floating seats',()=>{
   const controller=new DockController();
   controller.openContent({kind:'file',title:'docs/readme.md',contentId:'file:docs/readme.md'});
   controller.openContent({kind:'changes',title:'Edited files',contentId:'changes:runtime:attempt'});
   const review=Object.values(controller.getSnapshot().state.tabs).find(tab=>tab.kind==='changes')!;
   controller.floatTab(review.id);
   const layout=controller.getSnapshot().state;
   writeSidebarLayout('review',{layout,minted:100,history:{entries:[],cursor:0}});
   const restored=readSidebarLayout('review')!.layout;
   expect(Object.values(restored.tabs).map(tab=>tab.kind)).toContain('file');
   expect(Object.values(restored.tabs).some(tab=>tab.kind==='changes')).toBe(false);
   expect(restored.floats).toHaveLength(0);
   expect(layout.tabs[review.id]).toBe(review);
 });
 it('restores task tabs using exact native identities without persisting task instructions',()=>{
   const controller=new DockController();
   const identity=JSON.stringify(['original-session','original-node','original-conversation','9007199254740993']);
   controller.openContent({kind:'schedule',title:'Review repository changes',contentId:identity});
   writeSidebarLayout('task',{layout:controller.getSnapshot().state,minted:100,history:{entries:[],cursor:0}});
   const restored=Object.values(readSidebarLayout('task')!.layout.tabs);
   expect(restored).toHaveLength(1); expect(restored[0]).toMatchObject({kind:'schedule',contentId:identity,title:'Review repository changes'});
   expect(Object.keys(restored[0]).sort()).toEqual(['contentId','id','kind','title']);
 });
});
