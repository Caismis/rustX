import type { EditorLocator, EditorPage, UserInputBlock } from './v57.ts';

/** Read a known committed lineage input; never retry the lineage mutation. */
export async function readEditor(locator: EditorLocator, read: (block: number, offset: number) => Promise<EditorPage>, current: () => boolean): Promise<UserInputBlock[]> {
  const content: UserInputBlock[] = [];
  let block = 0, bytes = 0;
  while (current()) {
    let offset = 0, text = '', decoder = new TextDecoder('utf-8',{ignoreBOM:true});
    let nextBlock: number | undefined;
    do {
      const page = await read(block, offset);
      if (!current()) throw new Error('Editor view was replaced.');
      if (JSON.stringify(page.locator) !== JSON.stringify(locator) || page.block_index !== block) throw new Error('Editor source identity mismatch.');
      nextBlock = page.next_block ?? undefined;
      if (page.upload) { content.push({ type: 'upload', ...page.upload }); break; }
      if (!page.chunk) break;
      const chunk=page.chunk;
      if(chunk.data_base64.length>87_384 || chunk.data_base64.length%4!==0 || chunk.data_base64.length/4*3-(chunk.data_base64.endsWith('==')?2:chunk.data_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
      const data = Uint8Array.from(atob(chunk.data_base64), value => value.charCodeAt(0));
      if (chunk.offset !== offset || chunk.end_offset !== offset + data.length || data.length > 65_536 || !data.length && !chunk.complete) throw new Error('Invalid editor content range.');
      bytes += data.length;
      if (bytes > 786_432 || content.length >= 32) throw new Error('Editor content is deferred because it exceeds the input budget. Read the original conversation before editing.');
      text += decoder.decode(data, { stream: !chunk.complete });
      if (chunk.complete) { content.push({ type: 'text', text }); break; }
      if (chunk.next_offset == null || chunk.next_offset <= offset) throw new Error('Editor content failed to advance.');
      offset = chunk.next_offset;
    } while (current());
    if (nextBlock === undefined) return content;
    if (nextBlock <= block) throw new Error('Editor block page failed to advance.');
    block = nextBlock;
  }
  throw new Error('Editor view was replaced.');
}
