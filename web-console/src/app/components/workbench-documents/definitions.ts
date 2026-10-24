import { languageForPath } from '../../../presentation/primitives/file-language';
import { binaryDocumentPath, matchingDocumentPreviews, type DocumentPreviewDefinition } from './registry';
export type Viewer = 'markdown' | 'html' | 'image' | 'pdf' | 'excel' | 'office' | 'code' | 'text';
/** Harness registration order is semantic: specialized views precede source code. */
const definitions: DocumentPreviewDefinition[] = [
  { id: 'markdown', extensions: ['md', 'markdown'], loading: 'text-pages', wrap: false },
  { id: 'html', extensions: ['html', 'htm'], loading: 'bytes-complete', wrap: false },
  { id: 'image', extensions: ['png','jpg','jpeg','gif','webp','bmp','avif'], binaryExtensions: ['png','jpg','jpeg','gif','webp','bmp','avif'], loading: 'bytes-complete', wrap: false },
  { id: 'pdf', extensions: ['pdf'], binaryExtensions: ['pdf'], loading: 'bytes-complete', wrap: false },
  { id: 'office', extensions: ['docx','pptx'], binaryExtensions: ['docx','pptx'], loading: 'renderer', wrap: false },
  { id: 'excel', extensions: ['xlsx','csv','tsv'], binaryExtensions: ['xlsx'], loading: 'bytes-complete', wrap: false },
];
// Harness's known-binary rule, with rustX's raster/OOXML admission boundary.
const unsupported = new Set('mp4 mov avi mkv webm flv wmv m4v mp3 wav flac ogg m4a aac wma opus zip gz tgz bz2 xz zst 7z rar tar jar doc xls ppt odt ods odp pages numbers exe dll so dylib bin o class pyc wasm ttf otf woff woff2 eot dmg iso img sqlite db psd ai sketch tiff tif heic heif ico'.split(' '));
export function viewersForPath(path: string): Viewer[] {
  const specialized = matchingDocumentPreviews(definitions, path).map(value => value.id as Viewer);
  if (!specialized.length && unsupported.has(path.split('.').at(-1)?.toLowerCase() ?? '')) return [];
  return binaryDocumentPath(definitions,path) ? specialized : [...specialized, ...(languageForPath(path) ? ['code' as const] : []), 'text'];
}
export const loadsBytes = (viewer: Viewer) => viewer === 'html' || viewer === 'image' || viewer === 'pdf' || viewer === 'excel';
