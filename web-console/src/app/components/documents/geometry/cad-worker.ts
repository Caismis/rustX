import { importCad } from './cad';
import { GeometryError } from './limits';
import { serveGeometry } from './worker';
serveGeometry((bytes, format) => {
  if (format !== 'step' && format !== 'iges' && format !== 'brep') throw new GeometryError('unsupported');
  return importCad(bytes, format);
});
