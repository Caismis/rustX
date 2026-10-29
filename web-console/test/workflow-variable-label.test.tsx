import { afterEach, expect, it } from 'vitest';
import { cleanup, render } from '@testing-library/react';
import { translator } from '../src/locale/translation';
import { NodePreview } from '../src/app/workflows/ui/editor/NodePreview';
import type { Predicate } from '../src/app/workflows/document/model';
import { PredicateView, type VariableSources } from '../src/app/workflows/ui/editor/VariableLabel';

afterEach(cleanup);
const tx = translator('en');
const sources: VariableSources = { args: { kind: 'start', title: '' }, draft: { kind: 'agent', title: 'Drafter' } };

it('shows a card reference as Dify does: source icon and title, a slash, then {x} and the name', () => {
  const node = { type: 'review', subject: { value: { type: 'reference', path: ['draft', 'proposal'] } }, context: [] } as never;
  const { container } = render(<NodePreview tx={tx} node={node} sources={sources} />);
  const label = container.querySelector('[data-node-preview] [title]:not([data-node-preview] > div)')!;
  expect(label.getAttribute('title')).toBe('Drafter / proposal');
  expect(label.textContent).toBe('Drafter/{x}proposal');
  expect(label.querySelector('svg')).toBeTruthy();
});

it('shows a branch comparison as variable, operator and value, naming the Start by its default title', () => {
  const predicate = { type: 'contains', left: { type: 'reference', path: ['args', 'topic'] }, right: { type: 'literal', value: 'rust' } } satisfies Predicate;
  const { container } = render(<div><PredicateView tx={tx} sources={sources} predicate={predicate} /></div>);
  expect(container.textContent).toBe(`${tx('workflows:canvas.start-title')}/{x}topic${tx('workflows:branch.contains')}rust`);
});
