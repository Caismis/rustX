import { readJson } from '../../../../../../shared/json-numbers';
import clsx from 'clsx';
import { useState, type ReactNode } from 'react';
import { useTranslation } from '../../../../../locale/react';
import { IconChevronDownOutline14 } from '../../../../../presentation/primitives/icons';
import type { Value } from '../../../document/model';
import type { FieldAddress } from '../../../document/fields';
import { TemplateInput } from '../forms/TemplateInput';
import { ReferencePicker } from '../forms/ReferencePicker';
import { ValueEditor } from '../forms/ValueEditor';
import { IconPencil } from '../../icons';
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

/** Boolean operands keep reference and constant changes explicit, as Tool inputs do. */
export function BooleanInput({ value, address, edit, label, onChange }: {
  value: Value; address: FieldAddress; edit: Edit; label: string; onChange: (value: Value) => void;
}) {
  const tx = useTranslation(), variable = value.type === 'reference';
  if (!variable && !(value.type === 'literal' && typeof value.value === 'boolean')) {
    return <ValueEditor label={label} address={address} value={value} edit={edit} />;
  }
  return <div className={css.toolArgumentRow} role="group" aria-label={label}>
    <div className={css.typeSwitch} role="radiogroup" aria-label={tx('workflows:value.kind')}>
      <button type="button" role="radio" aria-checked={variable} aria-label={tx('workflows:tool.variable')} title={tx('workflows:tool.variable')} className={clsx(variable && css.typeSwitchActive)}
        onClick={() => { if (!variable) onChange({ type: 'reference', path: [] }); }}>{/* i18n-raw: Dify's variable glyph */ '{x}'}</button>
      <button type="button" role="radio" aria-checked={!variable} aria-label={tx('workflows:tool.constant')} title={tx('workflows:tool.constant')} className={clsx(!variable && css.typeSwitchActive)}
        onClick={() => { if (variable) onChange({ type: 'literal', value: false }); }}><IconPencil size={14} /></button>
    </div>
    {variable ? <ReferencePicker token address={address} path={value.path} edit={edit} />
      : <select className={css.input} aria-label={label} value={String(value.value)}
        onChange={event => onChange({ type: 'literal', value: event.target.value === 'true' })}>
        <option value="true">{/* i18n-raw: boolean literal */ 'True'}</option>
        <option value="false">{/* i18n-raw: boolean literal */ 'False'}</option>
      </select>}
  </div>;
}
