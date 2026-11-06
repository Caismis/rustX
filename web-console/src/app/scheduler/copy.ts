import type { en } from '../../locale/dictionaries/scheduler';
import { useLocale } from '../../locale/react';
import { translator, type TranslationKey } from '../../locale/translation';
import { useMemo } from 'react';
export type TaskManagerKey = keyof typeof en;
export type TaskTranslate = (key: TaskManagerKey, params?: Readonly<Record<string, string | number>>) => string;
export function useTaskCopy(): TaskTranslate {
  const locale = useLocale().active;
  return useMemo(() => { const tx = translator(locale); return (key, params) => tx(('scheduler:' + key) as TranslationKey, params); }, [locale]);
}
