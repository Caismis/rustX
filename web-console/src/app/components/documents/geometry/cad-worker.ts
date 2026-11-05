import { importCad } from './cad';
import { GeometryError } from './limits';
import { serveGeometry } from './worker';
serveGeometry((bytes, format, stage, module) => {
  if (format !== 'step' && format !== 'iges' && format !== 'brep') throw new GeometryError('unsupported');
  if (!(module instanceof WebAssembly.Module)) throw new GeometryError('invalid');
  return importCad(bytes, format, module, stage, true);
});
