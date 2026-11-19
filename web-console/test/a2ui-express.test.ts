import { describe, expect, it } from 'vitest';
import { boundProps, parseExpress, writeBinding } from '../src/presentation/markdown/a2ui-express';

describe('static Express trees', () => {
  it('resolves named children and data declared after the root', () => {
    const tree = parseExpress('root = Card(label)\nlabel = Text($/items/0/name, "caption")\n$/items = [{name: "模型"}]');
    expect(tree?.root.children[0]).toMatchObject({ kind: 'Text', children: [], text: '模型', variant: 'caption' });
  });
  it.each(['root=Card(root)', 'root=Button(Text("Go"), Event("go"))', '<a2ui>root=Text("partial")', 'root=Text($/missing)'])('leaves invalid or unsupported input as source', source => {
    expect(parseExpress(source)).toBeNull();
  });
});

it('updates a nested bound field and preserves the original parsed rows', () => {
  const source = '$/rows=[{name:"A",n:1}]\nroot=Column([Slider(value=$/rows/0/n),Table($/rows,_,title="Counts")])';
  const surface = parseExpress(source)!;
  const [slider, table] = surface.root.children;
  const changed = writeBinding(surface.data, slider.bindings!.value, 7);
  expect(boundProps(table, changed)).toMatchObject({ rows: [{ name: 'A', n: 7 }], title: 'Counts' });
  expect(boundProps(table, surface.data).rows).toEqual([{ name: 'A', n: 1 }]);
});
