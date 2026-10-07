import { languageForPath } from '../../../presentation/primitives/file-language';
import { binaryDocumentPath, matchingDocumentPreviews, type DocumentPreviewDefinition } from './registry';
export type Viewer = 'markdown' | 'html' | 'image' | 'pdf' | 'excel' | 'office' | 'code' | 'text';
/** Harness registration order is semantic: specialized views precede source code. */
const definitions: DocumentPreviewDefinition[] = [
  { id: 'markdown', extensions: ['md', 'markdown'], loading: 'text-pages', wrap: false },
  { id: 'html', extensions: ['html', 'htm'], loading: 'text-pages', wrap: false },
  { id: 'image', extensions: ['png','jpg','jpeg','gif','webp','bmp','ico','svg'], binaryExtensions: ['png','jpg','jpeg','gif','webp','bmp','ico'], loading: 'bytes-complete', wrap: false },
  { id: 'pdf', extensions: ['pdf'], binaryExtensions: ['pdf'], loading: 'bytes-complete', wrap: false },
  { id: 'office', extensions: ['docx','pptx','doc','ppt'], binaryExtensions: ['docx','pptx','doc','ppt'], loading: 'renderer', wrap: false },
  { id: 'excel', extensions: ['xlsx','xls','csv','tsv'], binaryExtensions: ['xlsx','xls'], loading: 'bytes-complete', wrap: false },
];
export function viewersForPath(path: string): Viewer[] {
  const specialized = matchingDocumentPreviews(definitions, path).map(value => value.id as Viewer);
  return binaryDocumentPath(definitions,path) ? specialized : [...specialized, ...(languageForPath(path) ? ['code' as const] : []), 'text'];
}
export const loadsBytes = (viewer: Viewer) => viewer === 'image' || viewer === 'pdf' || viewer === 'excel';
