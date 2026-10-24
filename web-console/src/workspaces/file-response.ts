/** Decode bounded standard multipart using the platform parser. No Base64 bodies. */
export async function fileResponse(response: Response, maxBytes: number): Promise<unknown> {
  if (!response.headers.get('Content-Type')?.startsWith('multipart/form-data;') || !response.body) throw new Error('Invalid file response');
  let count = 0;
  const bounded = response.body.pipeThrough(new TransformStream<Uint8Array, Uint8Array>({
    transform(chunk, controller) {
      count += chunk.byteLength;
      if (count > maxBytes + 4 * 1024 * 1024) throw new Error('File response exceeds budget');
      controller.enqueue(chunk);
    },
  }));
  const form = await new Response(bounded, { headers: response.headers }).formData();
  const metadata = form.get('metadata');
  if (typeof metadata !== 'string' || metadata.length > 4 * 1024 * 1024
    || form.getAll('metadata').length !== 1 || form.getAll('data').length > 1
    || [...form.keys()].some(key => key !== 'metadata' && key !== 'data')) throw new Error('Invalid file response');
  const result = JSON.parse(metadata);
  if (!result || typeof result.ok !== 'boolean') throw new Error('Invalid file result');
  const body = form.get('data');
  if (result.attachment !== undefined) {
    if (!result.ok || !(body instanceof Blob) || body.size > maxBytes) throw new Error('Invalid file body');
    const data = new Uint8Array(await body.arrayBuffer());
    if (result.attachment === 'data' && result.value?.data === null) result.value.data = data;
    else if (result.attachment === 'preview' && result.value?.preview?.kind === 'pdf' && result.value.preview.data === null) result.value.preview.data = data;
    else throw new Error('Invalid file attachment');
  } else if (body !== null) throw new Error('Unexpected file body');
  return result;
}
