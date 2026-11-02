import { importStl } from './stl';
import { GeometryError } from './limits';
import { serveGeometry } from './worker';
serveGeometry((bytes, format) => {
  if (format !== 'stl') throw new GeometryError('unsupported');
  return importStl(bytes);
});
