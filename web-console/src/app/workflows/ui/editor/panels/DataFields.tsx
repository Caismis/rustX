import { readJson } from '../../../../../../shared/json-numbers';
import { useState, type ReactNode } from 'react';
import { useTranslation } from '../../../../../locale/react';
import { IconChevronDownOutline14 } from '../../../../../presentation/primitives/icons';
import type { Value } from '../../../document/model';
import type { FieldAddress } from '../../../document/fields';
import { TemplateInput } from '../forms/TemplateInput';
import type { Edit } from './shared';
import css from '../Editor.module.css';
import styles from './DataPanels.module.css';

export function DataOutputs({ items }: { items: { name: string; type: string; description: string }[] }) {
  const tx = useTranslation(),
    [open, setOpen] = useState(false);
  return (
    <section className={css.group}>
      <h3 className={css.groupHeading}>
        <button
          type="button"
          className={css.outputToggle}
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
        >
          {tx('workflows:prompt.output-variables')}
          <IconChevronDownOutline14 />
        </button>
      </h3>
      {open && (
        <dl className={css.outputVars}>
          {items.map((item) => (
            <div key={item.name}>
              <dt>
                <b>{item.name}</b>
                <code>{item.type}</code>
              </dt>
              <dd>{item.description}</dd>
            </div>
          ))}
        </dl>
      )}
    </section>
  );
}
export function DataSection({
  title,
  aside,
  children,
  required,
}: {
  title: string;
  aside?: ReactNode;
  children?: ReactNode;
  required?: boolean;
}) {
  return (
    <section className={css.group}>
      <h3 className={css.groupHeading}>
        <span>
          {title}
          {required && (
            <span aria-hidden className={css.required}>
              *
            </span>
          )}
        </span>
        {aside}
      </h3>
      {children}
    </section>
  );
}
export function ScalarInput({
  value,
  address,
  edit,
  label,
  onChange,
  numeric,
}: {
  value: Value;
  numeric?: boolean;
  address: FieldAddress;
  edit: Edit;
  label: string;
  onChange: (value: Value, session: number) => void;
}) {
  const parts =
    value.type === 'template'
      ? value.parts
      : value.type === 'reference'
        ? [value]
        : value.type === 'literal'
          ? [{ type: 'literal' as const, value: String(value.value ?? '') }]
          : [];
  return (
    <div className={styles.scalar} data-list-scalar>
      <TemplateInput
        parts={parts}
        address={address}
        edit={edit}
        label={label}
        onChange={(parts, session) => {
          let value: Value = parts.length === 1 && parts[0].type === 'literal' ? parts[0] : { type: 'template', parts };
          if (numeric && value.type === 'literal' && typeof value.value === 'string') {
            const parsed = readJson(value.value);
            if (parsed.ok && typeof parsed.value === 'number') value = { type: 'literal', value: parsed.value };
          }
          onChange(value, session);
        }}
      />
    </div>
  );
}
