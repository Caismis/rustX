import type { ReactNode } from 'react';
import type { ExpressNode } from './a2ui-express';
import css from './ExpressCard.module.css';

function renderNode(node: ExpressNode, index: number): ReactNode {
  switch (node.kind) {
    case 'Text': {
      const heading = /^h[1-6]$/.test(node.variant ?? '');
      const Tag = heading ? node.variant as 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6' : 'span';
      return <Tag key={index} className={heading ? css.heading : node.variant === 'caption' ? css.caption : css.text}>{node.text}</Tag>;
    }
    case 'Divider': return <hr key={index} className={css.divider}/>;
    case 'List': return <ul key={index} className={css.list}>{node.children.map((child, i) => <li key={i}>{renderNode(child, i)}</li>)}</ul>;
    default: return <div key={index} className={css[node.kind]}>{node.children.map(renderNode)}</div>;
  }
}

export function ExpressCard({ node }: { node: ExpressNode }) {
  return <div className={css.surface} data-a2ui-express="">{renderNode(node, 0)}</div>;
}
