import { useLocale } from '../../../locale/react';
import { en, zh } from '../../../locale/dictionaries/geometry';
import type { GeometryImportStage } from './geometry/stages';

/** A label for an observed event, shared by both geometry entry points. */
export function GeometryLoading({ stage }: { stage: 'reading' | 'loading' | GeometryImportStage }) {
  const locale = useLocale();
  return <>{(locale.active === 'zh' ? zh : en)[stage]}</>;
}
