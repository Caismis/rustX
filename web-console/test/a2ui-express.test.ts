import { describe, expect, it } from 'vitest';
import { parseExpress } from '../src/presentation/markdown/a2ui-express';

describe('static Express trees', () => {
  it('resolves named children and data declared after the root', () => {
    const tree = parseExpress('root = Card(label)\nlabel = Text($/items/0/name, "caption")\n$/items = [{name: "模型"}]');
    expect(tree?.children[0]).toEqual({ kind: 'Text', children: [], text: '模型', variant: 'caption' });
  });
  it.each(['root=Card(root)', 'root=Button(Text("Go"), Event("go"))', '<a2ui>root=Text("partial")', 'root=Text($/missing)'])('leaves invalid or unsupported input as source', source => {
    expect(parseExpress(source)).toBeNull();
  });
});
