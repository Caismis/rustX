import type { GeometryFormat } from './format';

export type GeometryImportStage = 'initializing' | 'importing' | 'preparing';
/** Observations only; terminal model validation and the import deadline are separate. */
export const geometryStages = (format: Exclude<GeometryFormat, 'glb'>): readonly GeometryImportStage[] => format === 'stl'
  ? ['importing', 'preparing'] : ['initializing', 'importing', 'preparing'];

/** Accept each real observation once, in the format's finite order. */
export function geometryStageObserver(format: Exclude<GeometryFormat, 'glb'>, observe: (stage: GeometryImportStage) => void) {
  const stages = geometryStages(format); let next = 0;
  return (stage: unknown) => {
    if (next < stages.length && stage === stages[next]) observe(stages[next++]);
  };
}
