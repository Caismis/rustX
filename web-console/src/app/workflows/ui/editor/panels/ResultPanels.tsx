import { useTranslation } from '../../../../../locale/react';
import { blockField } from '../../../document/fields';
import { blockAt } from '../../../document/model';
import { SchemaEditor } from '../forms/SchemaEditor';
import { ValueEditor } from '../forms/ValueEditor';
import { Section } from '../forms/fields';
import { useNode, type PanelProps } from './shared';

export function ReturnPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at } = useNode(props, 'return');
  return <>
    <Section title={tx('workflows:panel.output')}>
      <ValueEditor label={tx('workflows:inspector.return-output')} address={at('output')} value={node.output} edit={props.edit} />
    </Section>
    <SchemaEditor label={tx('workflows:inspector.block-output')} value={blockAt(props.document, props.block)!.output} address={blockField(props.block, ['output'])} edit={props.edit} />
  </>;
}
