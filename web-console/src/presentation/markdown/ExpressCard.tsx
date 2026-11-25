import { createContext, useContext, useState, type ReactNode } from 'react';
import { eventMessage, writeBinding, type ExpressNode, type ExpressSurface } from './a2ui-express';
import { scalarText } from './express-values';
import { ExpressControl, type ExpressControlProps } from './ExpressControl';
import { ExpressVisualization, PlaygroundChart } from './ExpressVisualization';
import { ExpressAlert, ExpressCandlestick, ExpressKeyValues, ExpressSparkline, ExpressStat, ExpressStatGroup, ExpressStockTicker, UnsupportedExpressComponent } from './ExpressMarket';
import { BundledIcon } from '../primitives/BundledIcon';
import { MarkdownText } from './MarkdownText';
import css from './ExpressCard.module.css';

export const ExpressEventContext = createContext<((message: string) => void) | undefined>(undefined);

function LocalControl({ kind, props, children, bound, onChange }: ExpressControlProps & { bound: boolean }) {
  const [state, setState] = useState(() => ({ initial: props.value, value: props.value }));
  if (!bound && !Object.is(props.value, state.initial)) {
    setState({ initial: props.value, value: props.value });
  }
  const value = Object.is(props.value, state.initial) ? state.value : props.value;
  return <ExpressControl kind={kind} props={bound ? props : { ...props, value }}
    onChange={bound ? onChange : next => setState(prev => ({ ...prev, value: next }))}>{children}</ExpressControl>;
}

function stringList(value: unknown): string[] | undefined {
  return Array.isArray(value) && value.every(item => typeof item === 'string') ? value : undefined;
}

export function ExpressCard({ surface, streaming }: { surface: ExpressSurface; streaming: boolean }) {
  const send = useContext(ExpressEventContext);
  const [model, setModel] = useState(() => ({ surface, root: surface.root, data: surface.data }));
  if (model.surface !== surface) {
    setModel({ surface, root: surface.root, data: surface.data });
  }
  const { root, data } = model.surface === surface ? model : { root: surface.root, data: surface.data };

  function renderNode(node: ExpressNode, index: number): ReactNode {
    const { props } = node;
    switch (node.kind) {
      case 'Text':
      case 'Title': {
        const variant = scalarText(props.variant ?? props.size) || (node.kind === 'Title' ? 'h3' : '');
        const Tag = /^h[1-6]$/.test(variant) ? variant as 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6' : variant === 'description' ? 'p' : 'span';
        return <Tag key={index} className={css.text} data-variant={variant}>
          {scalarText(props.text)}
          {node.children.map(renderNode)}
        </Tag>;
      }
      case 'Heading':
      case 'Section': {
        const level = scalarText(props.level);
        const Heading = level === '1' ? 'h1' : level === '3' ? 'h3' : 'h2';
        const eyebrow = scalarText(props.eyebrow);
        const description = scalarText(props.description ?? props.subtitle);
        const title = scalarText(props.title ?? props.text);
        return <section key={index} className={css.Column}>
          {(eyebrow || title || description) && <header className={css.sectionHeader}>
            {eyebrow && <span className={css.text} data-variant="label">{eyebrow}</span>}
            {title && <Heading className={css.text}>{title}</Heading>}
            {description && <p className={css.text} data-variant="description">{description}</p>}
          </header>}
          {node.children.map(renderNode)}
        </section>;
      }
      case 'Markdown': return <MarkdownText key={index} text={scalarText(props.text)} streaming={streaming && props.streaming === true}/>;
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
        const label = scalarText(props.label);
        return <span key={index} role={label ? 'img' : undefined} aria-label={label || undefined} aria-hidden={label ? undefined : true}><BundledIcon name={name.startsWith('lucide:') ? name : `lucide:${name.replaceAll('_', '-')}`} size={20} fallback={null}/></span>;
      }
      case 'Table':
      case 'LineChart':
      case 'BarChart': {
        if (node.kind !== 'Table' && Object.hasOwn(props, 'data')) return <PlaygroundChart key={index} kind={node.kind} props={props}/>;
        const rows = Array.isArray(props.rows) ? props.rows.filter(row => row && typeof row === 'object' && !Array.isArray(row)) : [];
        return <ExpressVisualization key={index} kind={node.kind} rows={rows} columns={stringList(props.columns)}
          title={typeof props.title === 'string' ? props.title : undefined}
          x={typeof props.x === 'string' ? props.x : undefined} y={typeof props.y === 'string' ? props.y : stringList(props.y)}/>;
      }
      case 'AreaChart': return <PlaygroundChart key={index} kind={node.kind} props={props}/>;
      case 'StockTicker': return <ExpressStockTicker key={index} props={props}/>;
      case 'CandlestickChart': return <ExpressCandlestick key={index} props={props}/>;
      case 'KeyValueList': return <ExpressKeyValues key={index} props={props}/>;
      case 'Alert': return <ExpressAlert key={index} props={props}/>;
      case 'Sparkline': return <ExpressSparkline key={index} props={props}/>;
      case 'Stat': return <ExpressStat key={index} props={props}/>;
      case 'StatGroup': return <ExpressStatGroup key={index} props={props}/>;
      case 'Unsupported': return <UnsupportedExpressComponent key={index} name={scalarText(props.name)}/>;
      case 'Button': {
        const activate = send && !streaming && node.event ? () => {
          const message = eventMessage(node, surface.resolve(data));
          if (message) send(message);
        } : undefined;
        return <ExpressControl key={index} kind={node.kind} props={props} onChange={() => {}} onActivate={activate}>
          {node.children.length ? node.children.map(renderNode) : scalarText(props.label ?? props.text)}
        </ExpressControl>;
      }
      case 'Slider': case 'CheckBox': case 'ChoicePicker': case 'TextField': case 'DateTimeInput': {
        const path = node.bindings?.value;
        return <LocalControl key={index} kind={node.kind} props={props} bound={!!path}
          onChange={value => path && setModel(previous => {
            const base = previous.surface === surface ? previous.data : surface.data;
            const nextData = writeBinding(base, path, value);
            const tree = surface.render(nextData);
            return tree ? { surface, root: tree, data: nextData } : previous;
          })}>
          {node.children.map(renderNode)}
        </LocalControl>;
      }
      default: {
        const gap = typeof props.gap === 'number' && Number.isFinite(props.gap) ? Math.max(0, Math.min(16, props.gap)) * 4 : undefined;
        return <div key={index} className={css[node.kind]} data-align={scalarText(props.align)}
          data-justify={scalarText(props.justify)} style={gap === undefined ? undefined : { gap }}>
          {node.children.map(renderNode)}
        </div>;
      }
    }
  }

  return <div className={css.surface} data-a2ui-express="" data-ds="coss">{renderNode(root, 0)}</div>;
}
