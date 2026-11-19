import { useState, type ReactNode } from 'react';
import { boundProps, writeBinding, type ExpressNode, type ExpressSurface } from './a2ui-express';
import { ExpressControl, type ExpressControlProps } from './ExpressControl';
import { ExpressVisualization } from './ExpressVisualization';
import css from './ExpressCard.module.css';

function LocalControl({ kind, props, children, bound, onChange }: ExpressControlProps & { bound: boolean }) {
  const [value, setValue] = useState(props.value);
  return <ExpressControl kind={kind} props={bound ? props : { ...props, value }}
    onChange={bound ? onChange : setValue}>{children}</ExpressControl>;
}

function stringList(value: unknown): string[] | undefined {
  return Array.isArray(value) && value.every(item => typeof item === 'string') ? value : undefined;
}

export function ExpressCard({ surface }: { surface: ExpressSurface }) {
  const [data, setData] = useState(surface.data);

  function renderNode(node: ExpressNode, index: number): ReactNode {
    const props = boundProps(node, data);
    switch (node.kind) {
      case 'Text': {
        const heading = /^h[1-6]$/.test(node.variant ?? '');
        const Tag = heading ? node.variant as 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6' : 'span';
        return <Tag key={index} className={heading ? css.heading : node.variant === 'caption' ? css.caption : css.text}>
          {node.bindings?.text ? String(props.text ?? '') : node.text}
        </Tag>;
      }
      case 'Divider': return <hr key={index} className={css.divider}/>;
      case 'List': return <ul key={index} className={css.list}>{node.children.map((child, i) => <li key={i}>{renderNode(child, i)}</li>)}</ul>;
      case 'Table':
      case 'LineChart':
      case 'BarChart': {
        const rows = Array.isArray(props.rows) ? props.rows.filter(row => row && typeof row === 'object' && !Array.isArray(row)) : [];
        return <ExpressVisualization key={index} kind={node.kind} rows={rows} columns={stringList(props.columns)}
          title={typeof props.title === 'string' ? props.title : undefined}
          x={typeof props.x === 'string' ? props.x : undefined} y={typeof props.y === 'string' ? props.y : stringList(props.y)}/>;
      }
      case 'Button': case 'Slider': case 'CheckBox': case 'ChoicePicker': case 'TextField': case 'DateTimeInput': {
        const path = node.bindings?.value;
        return <LocalControl key={index} kind={node.kind} props={props} bound={!!path}
          onChange={value => path && setData(previous => writeBinding(previous, path, value))}>
          {node.children.map(renderNode)}
        </LocalControl>;
      }
      default: return <div key={index} className={css[node.kind]}>{node.children.map(renderNode)}</div>;
    }
  }

  return <div className={css.surface} data-a2ui-express="">{renderNode(surface.root, 0)}</div>;
}
