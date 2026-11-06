/** Decode bounded standard multipart using the platform parser. No Base64 bodies. */
export async function fileResponse(response: Response, maxBytes: number, signal?: AbortSignal): Promise<unknown> {
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
    let target;
    if (result.attachment === 'data' && result.value?.data === null) target = result.value;
    else if (result.attachment === 'preview' && result.value?.preview?.kind === 'pdf' && result.value.preview.data === null) target = result.value.preview;
    else if (result.attachment === 'workspace-preview' && result.value?.preview?.data === null) target = result.value.preview;
    else throw new Error('Invalid file attachment');
    // Consume and validate the physical terminal response even after cancellation,
    // but leave canceled bytes in the parser's Blob instead of copying them.
    if (!signal?.aborted) target.data = new Uint8Array(await body.arrayBuffer());
  } else if (body !== null) throw new Error('Unexpected file body');
  return result;
}
