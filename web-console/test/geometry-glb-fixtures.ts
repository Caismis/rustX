/** Original allocation-plan fixtures: distinct views deliberately overlap one BIN. */
export function encodeGlb(json: unknown, bin: Uint8Array<ArrayBuffer>): Uint8Array<ArrayBuffer> {
  const text = new TextEncoder().encode(JSON.stringify(json)), jsonLength = Math.ceil(text.length / 4) * 4, binLength = Math.ceil(bin.length / 4) * 4;
  const bytes = new Uint8Array(28 + jsonLength + binLength), header = new DataView(bytes.buffer);
  header.setUint32(0, 0x46546c67, true); header.setUint32(4, 2, true); header.setUint32(8, bytes.length, true);
  header.setUint32(12, jsonLength, true); header.setUint32(16, 0x4e4f534a, true);
  bytes.fill(32, 20, 20 + jsonLength); bytes.set(text, 20);
  header.setUint32(20 + jsonLength, binLength, true); header.setUint32(24 + jsonLength, 0x004e4942, true); bytes.set(bin, 28 + jsonLength);
  return bytes;
}
export function paddedGlbViews(viewCount: number, viewBytes: number) {
  const bin = new Uint8Array(viewBytes); new Float32Array(bin.buffer, 0, 9).set([0,0,0, 1,0,0, 0,1,0]);
  const json = { asset: { version: '2.0' }, buffers: [{ byteLength: viewBytes }],
    bufferViews: Array.from({ length: viewCount }, () => ({ buffer: 0, byteOffset: 0, byteLength: viewBytes })),
    accessors: Array.from({ length: viewCount }, (_, bufferView) => ({ bufferView, componentType: 5126, count: 3, type: 'VEC3', min: [0,0,0], max: [1,1,0] })),
    meshes: [{ primitives: Array.from({ length: viewCount }, (_, POSITION) => ({ attributes: { POSITION } })) }],
    nodes: [{ mesh: 0 }], scenes: [{ nodes: [0] }], scene: 0 };
  return { json, bin };
}
