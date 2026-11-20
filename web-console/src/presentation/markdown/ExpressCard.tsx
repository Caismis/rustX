import { createContext, useContext, useState, type ReactNode } from 'react';
import { eventMessage, writeBinding, type ExpressNode, type ExpressSurface } from './a2ui-express';
import { scalarText } from './express-values';
import { ExpressControl, type ExpressControlProps } from './ExpressControl';
import { ExpressVisualization } from './ExpressVisualization';
import { BundledIcon } from '../primitives/BundledIcon';
import css from './ExpressCard.module.css';

export const ExpressEventContext = createContext<((message: string) => void) | undefined>(undefined);

function LocalControl({ kind, props, children, bound, onChange }: ExpressControlProps & { bound: boolean }) {
  const [value, setValue] = useState(props.value);
  return <ExpressControl kind={kind} props={bound ? props : { ...props, value }}
    onChange={bound ? onChange : setValue}>{children}</ExpressControl>;
}

function stringList(value: unknown): string[] | undefined {
  return Array.isArray(value) && value.every(item => typeof item === 'string') ? value : undefined;
}

export function ExpressCard({ surface, streaming }: { surface: ExpressSurface; streaming: boolean }) {
  const send = useContext(ExpressEventContext);
  const [{ root, data }, setModel] = useState(() => ({ root: surface.root, data: surface.data }));

  function renderNode(node: ExpressNode, index: number): ReactNode {
    const { props } = node;
    switch (node.kind) {
      case 'Text': {
        const variant = scalarText(props.variant);
        const Tag = /^h[1-6]$/.test(variant) ? variant as 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6' : variant === 'description' ? 'p' : 'span';
        return <Tag key={index} className={css.text} data-variant={variant}>
          {scalarText(props.text)}
        </Tag>;
      }
      case 'Divider': return <hr key={index} data-slot="separator" className={css.divider}/>;
      case 'Card': return <div key={index} data-slot="card" className={css.Card}>{node.children.map(renderNode)}</div>;
      case 'List': {
        const numbered = props.marker === 'number';
        const List = numbered ? 'ol' : 'ul';
        return <List key={index} className={css.list} data-direction={scalarText(props.direction)} data-marker={numbered ? 'number' : undefined}>
          {node.children.map((child, i) => <li key={i}>{renderNode(child, i)}</li>)}
          {stringList(props.items)?.map((item, i) => <li key={node.children.length + i}>{item}</li>)}
        </List>;
      }
      case 'Icon': {
        const name = scalarText(props.name);
        return <span key={index} aria-label={scalarText(props.label) || undefined}><BundledIcon name={name.startsWith('lucide:') ? name : `lucide:${name.replaceAll('_', '-')}`} size={20} fallback={null}/></span>;
      }
      case 'Table':
      case 'LineChart':
      case 'BarChart': {
        const rows = Array.isArray(props.rows) ? props.rows.filter(row => row && typeof row === 'object' && !Array.isArray(row)) : [];
        return <ExpressVisualization key={index} kind={node.kind} rows={rows} columns={stringList(props.columns)}
          title={typeof props.title === 'string' ? props.title : undefined}
          x={typeof props.x === 'string' ? props.x : undefined} y={typeof props.y === 'string' ? props.y : stringList(props.y)}/>;
      }
      case 'Button': {
        const activate = send && !streaming && node.event ? () => {
          const message = eventMessage(node, surface.resolve(data));
          if (message) send(message);
        } : undefined;
        return <ExpressControl key={index} kind={node.kind} props={props} onChange={() => {}} onActivate={activate}>
          {node.children.map(renderNode)}
        </ExpressControl>;
      }
      case 'Slider': case 'CheckBox': case 'ChoicePicker': case 'TextField': case 'DateTimeInput': {
        const path = node.bindings?.value;
        return <LocalControl key={index} kind={node.kind} props={props} bound={!!path}
          onChange={value => path && setModel(previous => {
            const model = writeBinding(previous.data, path, value);
            const tree = surface.render(model);
            return tree ? { root: tree, data: model } : previous;
          })}>
          {node.children.map(renderNode)}
        </LocalControl>;
      }
      default: return <div key={index} className={css[node.kind]} data-align={scalarText(props.align)}
        data-justify={scalarText(props.justify)}>{node.children.map(renderNode)}</div>;
    }
  }

  return <div className={css.surface} data-a2ui-express="" data-ds="shadcn">{renderNode(root, 0)}</div>;
}
