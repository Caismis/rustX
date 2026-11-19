import { cleanup, render } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { ExpressVisualization } from '../src/presentation/markdown/ExpressVisualization';

afterEach(cleanup);

const sales = [
  { month: '一月', units: -2 },
  { month: '二月', units: 3 },
  { month: '三月', units: 10 },
];

const parts = (container: HTMLElement, part: string) => container.querySelectorAll(`[data-part="${part}"]`);

describe('ExpressVisualization Table', () => {
  it('renders a semantic table with caption, headers, and missing cells', () => {
    const { container, getByRole, getByText } = render(
      <ExpressVisualization kind="Table" title="销量明细" rows={[
        { month: '一月', units: -2, note: null },
        { month: '二月', units: 3 },
      ]} columns={['month', 'units', 'note']} />,
    );
    expect(getByRole('table')).toBeTruthy();
    expect(container.querySelector('caption')?.textContent).toBe('销量明细');
    expect(getByText('销量明细')).toBeTruthy();
    expect(getByRole('columnheader', { name: 'month' })).toBeTruthy();
    expect(getByRole('cell', { name: '-2' })).toBeTruthy();
    const missing = container.querySelector('[data-missing]');
    expect(missing?.textContent).toBe('—');
    // Explicit columns win: a key absent from `columns` never renders.
    expect(getByRole('columnheader', { name: 'note' })).toBeTruthy();
  });

  it('falls back to first-seen row keys and stays reactive to rows', () => {
    const { container, getByRole, rerender } = render(
      <ExpressVisualization kind="Table" rows={[{ a: 1, b: 'x' }, { a: 2, c: true }]} />,
    );
    expect(getByRole('columnheader', { name: 'a' })).toBeTruthy();
    expect(getByRole('columnheader', { name: 'b' })).toBeTruthy();
    expect(getByRole('columnheader', { name: 'c' })).toBeTruthy();
    expect(getByRole('cell', { name: 'true' })).toBeTruthy();
    rerender(<ExpressVisualization kind="Table" rows={[{ a: 9, b: 'y', c: false }]} />);
    expect(getByRole('cell', { name: '9' })).toBeTruthy();
    expect(getByRole('cell', { name: 'false' })).toBeTruthy();
    expect(container.querySelectorAll('tbody tr')).toHaveLength(1);
  });
});

describe('ExpressVisualization LineChart', () => {
  it('renders an accessible figure with legend, line path, and points', () => {
    const { container, getByRole } = render(
      <ExpressVisualization kind="LineChart" title="销量趋势" rows={sales} x="month" y="units" />,
    );
    const svg = getByRole('img');
    expect(svg.querySelector('title')?.textContent).toBe('销量趋势');
    expect(svg.querySelector('desc')?.textContent).toContain('units');
    expect(parts(container, 'line')).toHaveLength(1);
    expect(parts(container, 'point')).toHaveLength(3);
    const legend = container.querySelector('ul');
    expect(legend?.textContent).toContain('units');
    expect(container.querySelector('figcaption')?.textContent).toBe('销量趋势');
    // Data crosses zero, so the baseline is drawn.
    expect(parts(container, 'zero')).toHaveLength(1);
  });

  it('breaks the line at missing values and keeps lone points', () => {
    const gapped = [
      { m: 'a', v: 1 },
      { m: 'b', v: 2 },
      { m: 'c', v: null },
      { m: 'd', v: 3 },
      { m: 'e', v: 4 },
    ];
    const { container, rerender } = render(<ExpressVisualization kind="LineChart" rows={gapped} x="m" y="v" />);
    expect(parts(container, 'line')).toHaveLength(2);
    expect(parts(container, 'point')).toHaveLength(4);
    rerender(<ExpressVisualization kind="LineChart" rows={[{ m: 'only', v: 5 }]} x="m" y="v" />);
    expect(parts(container, 'line')).toHaveLength(0);
    expect(parts(container, 'point')).toHaveLength(1);
  });

  it('omits the zero baseline when all values share one sign', () => {
    const { container } = render(
      <ExpressVisualization kind="LineChart" rows={[{ m: 'a', v: 3 }, { m: 'b', v: 10 }]} x="m" y="v" />,
    );
    expect(parts(container, 'zero')).toHaveLength(0);
  });

  it('spaces numeric x values proportionally and draws a multi-series legend', () => {
    const { container } = render(
      <ExpressVisualization kind="LineChart" rows={[
        { t: 1, a: 1, b: 2 },
        { t: 2, a: 2, b: 1 },
        { t: 10, a: 3, b: 3 },
      ]} x="t" y={['a', 'b']} />,
    );
    const xs = [...parts(container, 'point')].map(circle => Number(circle.getAttribute('cx')));
    // Sorted by x: the 2→10 gap dwarfs the 1→2 gap only on a linear scale.
    expect(xs[2] - xs[1]).toBeGreaterThan((xs[1] - xs[0]) * 3);
    const legend = container.querySelector('ul');
    expect(legend?.textContent).toContain('a');
    expect(legend?.textContent).toContain('b');
    expect(parts(container, 'line')).toHaveLength(2);
  });
});

describe('ExpressVisualization BarChart', () => {
  it('draws grouped bars around a zero baseline, including negatives', () => {
    const { container } = render(
      <ExpressVisualization kind="BarChart" title="月度销量" rows={sales} x="month" y="units" />,
    );
    const bars = parts(container, 'bar');
    expect(bars).toHaveLength(3);
    const zeroY = Number(parts(container, 'zero')[0].getAttribute('y1'));
    // Negative bar hangs below the baseline; positive bars rise above it.
    expect(Number(bars[0].getAttribute('y'))).toBe(zeroY);
    expect(Number(bars[1].getAttribute('y'))).toBeLessThan(zeroY);
    expect(Number(bars[2].getAttribute('y'))).toBeLessThan(zeroY);
    expect(container.querySelector('figcaption')?.textContent).toBe('月度销量');
  });

  it('paints a visible sliver for zero values and groups multi-series bars', () => {
    const { container } = render(
      <ExpressVisualization kind="BarChart" rows={[
        { m: 'a', p: 0, q: 4 },
        { m: 'b', p: -1, q: null },
      ]} x="m" y={['p', 'q']} />,
    );
    const bars = parts(container, 'bar');
    // q is missing for row b: 2 + 1 = 3 bars; zero-height p/a still paints.
    expect(bars).toHaveLength(3);
    const zeroBar = bars[0];
    expect(Number(zeroBar.getAttribute('height'))).toBeGreaterThan(0);
    const legend = container.querySelector('ul');
    expect(legend?.textContent).toContain('p');
    expect(legend?.textContent).toContain('q');
  });
});

describe('ExpressVisualization empty states', () => {
  it('reports no data for charts without plottable values', () => {
    const { getByText, rerender } = render(
      <ExpressVisualization kind="LineChart" rows={[]} x="m" y="v" />,
    );
    expect(getByText('No data')).toBeTruthy();
    rerender(<ExpressVisualization kind="BarChart" rows={[{ m: 'a' }]} x="m" y="missing" />);
    expect(getByText('No data')).toBeTruthy();
  });

  it('keeps the table headers and labels the empty state', () => {
    const { getByRole } = render(
      <ExpressVisualization kind="Table" title="空表" rows={[]} columns={['a', 'b']} />,
    );
    expect(getByRole('table')).toBeTruthy();
    expect(getByRole('columnheader', { name: 'a' })).toBeTruthy();
    expect(getByRole('cell', { name: 'No data' }).getAttribute('colspan')).toBe('2');
  });
});
