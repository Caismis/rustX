import { importStl } from './stl';
import { GeometryError } from './limits';
import { serveGeometry } from './worker';
serveGeometry((bytes, format, stage) => {
  if (format !== 'stl') throw new GeometryError('unsupported');
  stage('importing');
  return importStl(bytes);
});
