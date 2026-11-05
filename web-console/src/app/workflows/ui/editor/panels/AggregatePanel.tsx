import { useState } from 'react';
import { useTranslation } from '../../../../../locale/react';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { Modal } from '../../../../../presentation/primitives/Modal';
import { Button } from '../../../../../presentation/primitives/Button';
import { renameAggregateOutput } from '../../../document/commands';
import { aggregateGroupUsed, removeAggregateGroup, toggleAggregateGroups } from '../../../document/aggregate';
import { variableType } from '../../../document/data-nodes';
import { IconPlus } from '../../icons';
import { ReferencePicker } from '../forms/ReferencePicker';
import { RouteId } from './InferencePanels';
import { DataOutputs, DataSection } from './DataFields';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';
import styles from './DataPanels.module.css';

export function AggregatePanel(props: PanelProps) {
  const tx = useTranslation(),
    { node, at, update } = useNode(props, 'aggregate'),
    [confirm, setConfirm] = useState<{ kind: 'disable' } | { kind: 'remove'; name: string }>();
  const toggle = (grouped: boolean) =>
    props.edit((document) => toggleAggregateGroups(document, props.block, props.id, grouped));
  const remove = (name: string) =>
    props.edit((document) => removeAggregateGroup(document, props.block, props.id, name), undefined, {
      array: at('groups'),
      removed: node.groups.findIndex((group) => group.name === name),
    });
  const requestRemove = (name: string) => {
    if (aggregateGroupUsed(props.document, props.block, props.id, name)) setConfirm({ kind: 'remove', name });
    else remove(name);
  };
  return (
    <div className={styles.panel}>
      <section className={css.group}>
        {(node.grouped ? node.groups : node.groups.slice(0, 1)).map((group, index) => (
          <div className={styles.group} key={`${index}:${group.name}`}>
            <h3>
              {node.grouped ? (
                <div>
                  <RouteId
                    label={tx('workflows:data.group-name')}
                    value={group.name}
                    address={at('groups', index, 'name')}
                    rename={(document, from, to) => renameAggregateOutput(document, props.block, props.id, from, to)}
                  />
                </div>
              ) : (
                <span>{tx('workflows:aggregate.variables')}</span>
              )}
              {group.output_type && group.variables.length > 0 && (
                <span className={styles.badge}>{group.output_type}</span>
              )}
              <ReferencePicker
                token
                disabled={group.variables.length >= 32}
                addLabel={tx('workflows:aggregate.add-variable', { p0: node.grouped?group.name:'output' })}
                address={at('groups', index, 'variables', group.variables.length)}
                path={[]}
                edit={props.edit}
                onChoose={(choice) => {
                  if (
                    group.variables.length >= 32 ||
                    group.variables.some((path) => JSON.stringify(path) === JSON.stringify(choice.path))
                  )
                    return;
                  const output_type =
                    variableType(choice.schema as Record<string, unknown> | undefined) ?? group.output_type;
                  update(
                    (current) => ({
                      ...current,
                      groups: current.groups.map((item, i) =>
                        i === index ? { ...item, output_type, variables: [...item.variables, [...choice.path]] } : item,
                      ),
                    }),
                    at('groups', index, 'variables'),
                    { array: at('groups', index, 'variables'), inserted: group.variables.length },
                  );
                }}
              />
              {node.grouped && node.groups.length > 1 && (
                <button
                  type="button"
                  className={css.iconButton}
                  aria-label={tx('workflows:data.remove-group', { p0: group.name })}
                  onClick={() => requestRemove(group.name)}
                >
                  <IconTrashOutline16 />
                </button>
              )}
            </h3>
            {!group.variables.length ? (
              <p className={styles.empty}>{tx('workflows:aggregate.empty')}</p>
            ) : (
              <div className={styles.variables}>
                {group.variables.map((path, position) => (
                  <div className={styles.source} key={position}>
                    <div className={styles.input}>
                      <ReferencePicker
                        token
                        address={at('groups', index, 'variables', position)}
                        path={path}
                        edit={props.edit}
                        onChoose={(choice) => {
                          if (
                            group.variables.some(
                              (path, j) => j !== position && JSON.stringify(path) === JSON.stringify(choice.path),
                            )
                          )
                            return;
                          update(
                            (current) => ({
                              ...current,
                              groups: current.groups.map((item, i) =>
                                i === index
                                  ? {
                                      ...item,
                                      variables: item.variables.map((path, j) =>
                                        j === position ? [...choice.path] : path,
                                      ),
                                    }
                                  : item,
                              ),
                            }),
                            at('groups', index, 'variables', position),
                          );
                        }}
                      />
                    </div>
                    <button
                      type="button"
                      className={css.iconButton}
                      aria-label={tx('workflows:data.remove-source', { p0: String(position + 1) })}
                      onClick={() =>
                        update(
                          (current) => ({
                            ...current,
                            groups: current.groups.map((item, i) => {
                              if (i !== index) return item;
                              const variables = item.variables.filter((_, j) => j !== position);
                              return {
                                ...item,
                                variables,
                                output_type: variables.length ? item.output_type : undefined,
                              };
                            }),
                          }),
                          at('groups', index, 'variables'),
                          { array: at('groups', index, 'variables'), removed: position },
                        )
                      }
                    >
                      <IconTrashOutline16 />
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
        ))}
        {node.grouped && (
          <button
            type="button"
            className={css.promptAddMessage}
            disabled={node.groups.length >= 32}
            onClick={() => {
              let n = 2;
              while (node.groups.some((group) => group.name === `Group${n}`)) n++;
              update(
                (current) => ({ ...current, groups: [...current.groups, { name: `Group${n}`, variables: [] }] }),
                at('groups'),
                { array: at('groups'), inserted: node.groups.length },
              );
            }}
          >
            <IconPlus size={14} />
            {tx('workflows:data.add-group')}
          </button>
        )}
      </section>
      <DataSection
        title={tx('workflows:aggregate.grouping')}
        aside={
          <Switch
            label={tx('workflows:aggregate.grouping')}
            checked={node.grouped}
            onChange={(enabled) => {
              if (
                !enabled &&
                node.groups
                  .slice(1)
                  .some((group) => aggregateGroupUsed(props.document, props.block, props.id, group.name))
              )
                setConfirm({ kind: 'disable' });
              else toggle(enabled);
            }}
          />
        }
      >
        <p className={styles.help}>{tx('workflows:aggregate.grouping-help')}</p>
      </DataSection>
      {node.grouped && (
        <DataOutputs
          items={node.groups.map((group) => ({
            name: `${group.name}.output`,
            type: group.output_type ?? '?',
            description: tx('workflows:aggregate.output-description', { p0: group.name }),
          }))}
        />
      )}
      {confirm && (
        <Modal
          open
          title={tx('workflows:aggregate.remove-title')}
          description={tx('workflows:aggregate.remove-description')}
          closeLabel={tx('workflows:dialog.close')}
          onClose={() => setConfirm(undefined)}
          footer={
            <>
              <Button onClick={() => setConfirm(undefined)}>{tx('workflows:dialog.cancel')}</Button>
              <Button
                variant="primary"
                onClick={() => {
                  if (confirm.kind === 'disable') toggle(false);
                  else remove(confirm.name);
                  setConfirm(undefined);
                }}
              >
                {tx('workflows:aggregate.confirm-remove')}
              </Button>
            </>
          }
        />
      )}
    </div>
  );
}
