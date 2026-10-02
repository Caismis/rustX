import { imageSize } from 'image-size';
/** Inspect dimensions before a browser decoder allocates pixels. Keep the
 * parser library's Uint8Array path; its filesystem API never enters the Web. */
export const RASTER_MAX_DIMENSION = 4096;
export const RASTER_MAX_PIXELS = 4_194_304;
export function validateRaster(bytes: Uint8Array): void {
  const size = imageSize(bytes);
  if (!['png', 'jpg', 'gif', 'webp', 'bmp', 'avif'].includes(size.type ?? '')) throw new Error('Unsupported raster image');
  for (const image of [size, ...(size.images ?? [])]) {
    if (!Number.isInteger(image.width) || !Number.isInteger(image.height) || image.width < 1 || image.height < 1
      || image.width > RASTER_MAX_DIMENSION || image.height > RASTER_MAX_DIMENSION || image.width * image.height > RASTER_MAX_PIXELS) throw new Error('Image dimensions exceed preview policy');
  }
  const data = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const tag = (at: number, count: number) => new TextDecoder('latin1').decode(bytes.subarray(at, at + count));
  // Animation can retain many decoded frames; only a static raster is inline.
  if (size.type === 'png') {
    for (let at = 8; at + 12 <= bytes.length;) {
      const length = data.getUint32(at);
      if (tag(at + 4, 4) === 'acTL') throw new Error('Animated images are not supported inline');
      if (length > bytes.length - at - 12) throw new Error('Invalid PNG');
      at += length + 12;
    }
  }
  if (size.type === 'webp' && tag(12, 4) === 'VP8X' && bytes[20] & 2) throw new Error('Animated images are not supported inline');
  if (size.type === 'avif' && tag(8, data.getUint32(0) - 8).includes('avis')) throw new Error('Animated images are not supported inline');
  if (size.type === 'gif') {
    let at = 13 + (bytes[10] & 128 ? 3 * (1 << ((bytes[10] & 7) + 1)) : 0), frames = 0;
    const blocks = () => { while (at < bytes.length) { const length = bytes[at++]; if (!length) return; at += length; } throw new Error('Invalid GIF'); };
    while (at < bytes.length) {
      const kind = bytes[at++];
      if (kind === 59) { if (frames !== 1) throw new Error('Invalid GIF'); return; }
      if (kind === 33) { at++; blocks(); }
      else if (kind === 44) {
        if (++frames > 1 || at + 9 > bytes.length) throw new Error('Animated images are not supported inline');
        const width = data.getUint16(at + 4, true), height = data.getUint16(at + 6, true);
        if (width > RASTER_MAX_DIMENSION || height > RASTER_MAX_DIMENSION || width * height > RASTER_MAX_PIXELS) throw new Error('Image dimensions exceed preview policy');
        const packed = bytes[at + 8]; at += 9 + (packed & 128 ? 3 * (1 << ((packed & 7) + 1)) : 0) + 1; blocks();
      } else throw new Error('Invalid GIF');
    }
    throw new Error('Invalid GIF');
  }
}
