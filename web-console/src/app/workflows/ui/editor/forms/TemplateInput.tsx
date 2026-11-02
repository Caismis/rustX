import { useState } from 'react';
import { useTranslation } from '../../../../../locale/react';
import type { FieldAddress } from '../../../document/fields';
import type { Value } from '../../../document/model';
import type { Edit } from '../panels/shared';
import { PromptEditor } from './PromptEditor';
import css from '../Editor.module.css';

/** The parts a string value is typed as; another value shows none. */
export function templateParts(value?: Value): readonly Value[] {
  if (value?.type === 'template') return value.parts;
  return value?.type === 'reference' || value?.type === 'literal' && typeof value.value === 'string' && value.value ? [value] : [];
}

/** The plainest value of typed parts: nothing, fixed text, or a template. */
export function templateValue(parts: readonly Value[]): Value | undefined {
  if (!parts.length) return undefined;
  return parts.length === 1 && parts[0].type === 'literal' ? parts[0] : { type: 'template', parts: [...parts] };
}

/** As Dify's mixed variable text input: one string typed as text, where `/`
 * or `{` inserts a variable. Each focus starts a new undo step. */
export function TemplateInput({ parts, address, edit, label, badge, invalid, onChange }: {
  parts: readonly Value[]; address: FieldAddress; edit: Edit; label: string; badge?: string; invalid?: boolean; onChange: (parts: Value[], session: number) => void;
}) {
  const tx = useTranslation(), [session, setSession] = useState(0);
  return <PromptEditor parts={parts} address={address} edit={edit} label={label} compact invalid={invalid}
    placeholder={tx('workflows:value.template-placeholder')} variablePlaceholder={tx('workflows:branch.select-variable')} onChange={next => onChange(next, session)}>
    {editable => <div className={css.templateInput} aria-invalid={invalid || undefined} onFocus={() => setSession(value => value + 1)}>
      {editable}{badge && <span className={css.typeBadge}>{badge}</span>}
    </div>}
  </PromptEditor>;
}
