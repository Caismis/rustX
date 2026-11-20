import { describe, expect, it } from 'vitest';
import { boundProps, parseExpress, writeBinding } from '../src/presentation/markdown/a2ui-express';

describe('static Express trees', () => {
  it('resolves named children and data declared after the root', () => {
    const tree = parseExpress('root = Card(label)\nlabel = Text($/items/0/name, "caption")\n$/items = [{name: "模型"}]');
    expect(tree?.root.children[0]).toMatchObject({ kind: 'Text', children: [], text: '模型', variant: 'caption' });
  });
  it('reads Playground button appearance independently of its outbound action', () => {
    const surface = parseExpress('root=Button(Text("Delete"), "solid", Event("delete"), color="danger", size="sm", block=true)');
    expect(surface?.root.props).toMatchObject({ variant: 'solid', color: 'danger', size: 'sm', block: true });
    expect(surface?.root.children[0]?.text).toBe('Delete');
  });
  it('keeps a card with both event and renderer function actions display-only', () => {
    const surface = parseExpress(`$/date = "2026-11-20"
root = Card(Column([
  Text($/date),
  Button(Text("More"), variant="primary", action=Event("load_more", {date: $/date})),
  Button(Text("Open HN"), action=openUrl("https://news.ycombinator.com"))
]))`);
    const items = surface?.root.children[0]?.children;
    expect(items?.map(item => item.kind)).toEqual(['Text', 'Button', 'Button']);
    expect(items?.[2]?.children[0]?.text).toBe('Open HN');
    expect(items?.slice(1).map(button => button.props?.action)).toEqual([undefined, undefined]);
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
