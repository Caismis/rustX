export type GeometryFormat = 'glb' | 'stl' | 'step' | 'iges' | 'brep';
export function geometryFormat(name: string): GeometryFormat {
  const extension = name.split('.').at(-1)?.toLowerCase();
  if (extension === 'stp') return 'step';
  if (extension === 'igs') return 'iges';
  if (extension === 'glb' || extension === 'stl' || extension === 'step' || extension === 'iges' || extension === 'brep') return extension;
  throw new Error('unsupported');
}
