import type { WorkflowListFilter, WorkflowListSort } from '../../../../../../../protocol/app-server/v79';
import { useTranslation } from '../../../../../locale/react';
import { IconCloseOutline16 } from '../../../../../presentation/primitives/icons';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { encodeField } from '../../../document/fields';
import { itemKind, listFields, listOperators, scalarValue } from '../../../document/data-nodes';
import { comparisonSymbols } from '../summary';
import { useReferenceSchema } from '../forms/useReferenceSchema';
import { ReferencePicker } from '../forms/ReferencePicker';
import { NumberField } from '../forms/fields';
import { BooleanInput, DataOutputs, DataSection, ScalarInput } from './DataFields';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';
import styles from './DataPanels.module.css';

export function ListPanel(props: PanelProps) {
  const tx = useTranslation(),
    { node, at, update } = useNode(props, 'list'),
    path = node.input.type === 'reference' ? node.input.path : [];
  const schema=useReferenceSchema(at('input'),path),item=schema?.items as Record<string,unknown>|undefined;
  const fields = listFields(item),
    hasFields = itemKind(item) === 'object';
  const selected = fields.find((field) => JSON.stringify(field.path) === JSON.stringify(node.filter?.path ?? [])),
    kind = selected?.kind ?? itemKind(item);
  const filter = node.filter,
    sort = node.sort,
    operators = listOperators(kind);
  const changeFilter = (next: WorkflowListFilter) => update((current) => ({ ...current, filter: next }), at('filter'));
  const changeSort = (next: WorkflowListSort) => update((current) => ({ ...current, sort: next }), at('sort'));
  return (
    <div className={styles.panel}>
      <DataSection title={tx('workflows:data.inputs')} required>
        <div className={styles.inputRow}>
          <div className={styles.input} data-empty={!path.length || undefined}>
            <ReferencePicker
              token
              address={at('input')}
              path={path}
              edit={props.edit}
              placeholder={tx('workflows:classifier.select-input')}
              onChoose={(choice) => {
                const nextFields = listFields(
                    (choice.schema as Record<string, unknown> | undefined)?.items as
                      Record<string, unknown> | undefined,
                  ),
                  first = nextFields[0],
                  type = first?.kind ?? 'unknown';
                update(
                  (current) => ({
                    ...current,
                    input: { type: 'reference', path: [...choice.path] },
                    ...(current.filter
                      ? {
                          filter: {
                            path: first?.path ?? [],
                            operator: listOperators(type)[0],
                            value: scalarValue(type),
                          },
                        }
                      : {}),
                    ...(current.sort ? { sort: { path: first?.path ?? [], order: current.sort.order } } : {}),
                  }),
                  at('input'),
                );
              }}
            />
          </div>
          {!!path.length && (
            <button
              type="button"
              className={css.iconButton}
              aria-label={tx('workflows:classifier.clear-input')}
              onClick={() => update((current) => ({ ...current, input: { type: 'reference', path: [] } }), at('input'))}
            >
              <IconCloseOutline16 />
            </button>
          )}
        </div>
      </DataSection>
      <DataSection
        title={tx('workflows:list.filter')}
        aside={
          <Switch
            label={tx('workflows:data.filter')}
            checked={!!filter}
            onChange={(enabled) =>
              update(
                ({ filter: _, ...current }) =>
                  enabled
                    ? {
                        ...current,
                        filter: {
                          path: fields[0]?.path ?? [],
                          operator: listOperators(fields[0]?.kind ?? kind)[0],
                          value: scalarValue(fields[0]?.kind ?? kind),
                        },
                      }
                    : current,
                at('filter'),
              )
            }
          />
        }
      >
        {filter && (
          <>
            {hasFields && (
              <select
                className={css.input}
                aria-label={tx('workflows:list.field')}
                value={JSON.stringify(filter.path)}
                onChange={(event) => {
                  const path = JSON.parse(event.target.value) as string[],
                    field = fields.find((field) => JSON.stringify(field.path) === event.target.value);
                  changeFilter({
                    path,
                    operator: listOperators(field?.kind ?? 'unknown')[0],
                    value: scalarValue(field?.kind ?? 'unknown'),
                  });
                }}
              >
                {fields.map((field) => (
                  <option key={JSON.stringify(field.path)} value={JSON.stringify(field.path)}>
                    {field.path.join(' / ')}
                  </option>
                ))}
              </select>
            )}
            <div className={styles.condition}>
              <select
                aria-label={tx('workflows:data.operator')}
                value={filter.operator}
                onChange={(event) =>
                  changeFilter({ ...filter, operator: event.target.value as WorkflowListFilter['operator'] })
                }
              >
                {operators.map((operator) => (
                  <option key={operator} value={operator}>
                    {kind === 'number'
                      ? comparisonSymbols[operator]
                      : kind === 'boolean'
                        ? tx(operator === 'equal' ? 'workflows:list.is' : 'workflows:list.is-not')
                        : tx(`workflows:data.op-${operator}`)}
                  </option>
                ))}
              </select>
              {!['empty', 'not_empty'].includes(filter.operator) &&
                (kind === 'boolean' ? (
                  <BooleanInput value={filter.value} address={at('filter', 'value')} edit={props.edit}
                    label={tx('workflows:data.compare-value')} onChange={value => changeFilter({ ...filter, value })} />
                ) : (
                  <ScalarInput
                    numeric={kind === 'number'}
                    value={filter.value}
                    address={at('filter', 'value')}
                    edit={props.edit}
                    label={tx('workflows:data.compare-value')}
                    onChange={(value, session) =>
                      update(
                        (current) => ({ ...current, filter: { ...current.filter!, value } }),
                        at('filter', 'value'),
                        undefined,
                        `${encodeField(at('filter', 'value'))}:${session}`,
                      )
                    }
                  />
                ))}
            </div>
          </>
        )}
      </DataSection>
      <DataSection
        title={tx('workflows:list.extract')}
        aside={
          <Switch
            label={tx('workflows:list.extract')}
            checked={!!node.extract}
            onChange={(enabled) =>
              update(
                ({ extract: _, ...current }) =>
                  enabled ? { ...current, extract: { type: 'literal', value: 1 } } : current,
                at('extract'),
              )
            }
          />
        }
      >
        {node.extract && (
          <>
            <ScalarInput
              numeric
              value={node.extract}
              address={at('extract')}
              edit={props.edit}
              label={tx('workflows:list.serial')}
              onChange={(extract, session) =>
                update(
                  (current) => ({ ...current, extract }),
                  at('extract'),
                  undefined,
                  `${encodeField(at('extract'))}:${session}`,
                )
              }
            />
            <p className={styles.help}>{tx('workflows:list.serial-help')}</p>
          </>
        )}
      </DataSection>
      <DataSection
        title={tx('workflows:data.limit')}
        aside={
          <Switch
            label={tx('workflows:data.limit')}
            checked={node.limit != null}
            onChange={(enabled) =>
              update(({ limit: _, ...current }) => (enabled ? { ...current, limit: 10 } : current), at('limit'))
            }
          />
        }
      >
        {node.limit != null && (
          <div className={styles.limit}>
            <input
              className={styles.slider}
              type="range"
              min={1}
              max={20}
              step={1}
              value={node.limit}
              aria-label={tx('workflows:list.limit-slider')}
              onChange={(event) =>
                update((current) => ({ ...current, limit: Number(event.target.value) }), at('limit'))
              }
            />
            <NumberField label={tx('workflows:data.take')} address={at('limit')} value={node.limit} min={1} max={20} />
          </div>
        )}
      </DataSection>
      <DataSection
        title={tx('workflows:list.order')}
        aside={
          <Switch
            label={tx('workflows:data.sort')}
            checked={!!sort}
            onChange={(enabled) =>
              update(
                ({ sort: _, ...current }) =>
                  enabled ? { ...current, sort: { path: fields[0]?.path ?? [], order: 'asc' } } : current,
                at('sort'),
              )
            }
          />
        }
      >
        {sort && (
          <>
            {hasFields && (
              <select
                className={css.input}
                aria-label={tx('workflows:list.sort-field')}
                value={JSON.stringify(sort.path)}
                onChange={(event) => changeSort({ ...sort, path: JSON.parse(event.target.value) })}
              >
                {fields.map((field) => (
                  <option key={JSON.stringify(field.path)} value={JSON.stringify(field.path)}>
                    {field.path.join(' / ')}
                  </option>
                ))}
              </select>
            )}
            <div className={styles.options} role="radiogroup" aria-label={tx('workflows:data.order')}>
              {(['asc', 'desc'] as const).map((order) => (
                <button
                  type="button"
                  role="radio"
                  tabIndex={sort.order === order ? 0 : -1}
                  aria-checked={sort.order === order}
                  onKeyDown={(event) => {
                    if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
                      event.preventDefault();
                      const next =
                        event.key === 'Home' ? 'asc' : event.key === 'End' ? 'desc' : order === 'asc' ? 'desc' : 'asc';
                      changeSort({ ...sort, order: next });
                      event.currentTarget.parentElement
                        ?.querySelectorAll<HTMLButtonElement>('button')
                        [next === 'asc' ? 0 : 1]?.focus();
                    }
                  }}
                  key={order}
                  onClick={() => changeSort({ ...sort, order })}
                >
                  {tx(`workflows:data.${order}`)}
                </button>
              ))}
            </div>
          </>
        )}
      </DataSection>
      <DataOutputs
        items={[
          {
            name: 'result',
            type: `Array[${itemKind(item) === 'unknown' ? '?' : itemKind(item)}]`,
            description: tx('workflows:list.output-result'),
          },
          { name: 'first_record', type: itemKind(item), description: tx('workflows:list.output-first') },
          { name: 'last_record', type: itemKind(item), description: tx('workflows:list.output-last') },
        ]}
      />
    </div>
  );
}
