import { describe, expect, it } from 'vitest';
import { eventMessage, parseExpress, writeBinding } from '../src/presentation/markdown/a2ui-express';

describe('static Express trees', () => {
  it('resolves named children and data declared after the root', () => {
    const tree = parseExpress('root = Card(label)\nlabel = Text($/items/0/name, "caption")\n$/items = [{name: "模型"}]');
    expect(tree?.root.children[0]).toMatchObject({ kind: 'Text', children: [], props: { text: '模型', variant: 'caption' } });
  });
  it('interpolates data paths in Text and reacts to changes in card data', () => {
    const template = '${/stories/0/pts} 分 · ${/stories/0/cmt} 评';
    const surface = parseExpress('$/stories=[{pts:858,cmt:1669}]\nroot=Text(formatString(' + JSON.stringify(template) + '), "caption")')!;
    expect(surface.root).toMatchObject({ kind: 'Text', props: { text: '858 分 · 1669 评', variant: 'caption' } });
    const changed = writeBinding(surface.data, ['stories', '0', 'pts'], 860);
    expect(surface.render(changed)?.props.text).toBe('860 分 · 1669 评');
  });
  it('supports named formatString arguments, JSON Pointer escapes and literal placeholders', () => {
    const template = '${/stats/a~1b~0c} · \\${/literal}';
    const surface = parseExpress('$/stats={"a/b~c":"ready"}\nroot=Text(formatString(value=' + JSON.stringify(template) + '))');
    expect(surface?.root.props.text).toBe('ready · ${/literal}');
  });
  it('reads Playground button appearance independently of its outbound action', () => {
    const surface = parseExpress('root=Button(Text("Delete"), "solid", Event("delete"), color="danger", size="sm", block=true)');
    expect(surface?.root.props).toMatchObject({ variant: 'solid', color: 'danger', size: 'sm', block: true });
    expect(surface?.root.children[0]?.props.text).toBe('Delete');
  });
  it('retains both an openUrl destination and a bound event', () => {
    const surface = parseExpress(`$/date = "2026-11-20"
root = Card(Column([
  Text($/date),
  Button(Text("More"), variant="primary", action=Event("load_more", {date: $/date})),
  Button(Text("Open HN"), action=openUrl("https://news.ycombinator.com"))
]))`);
    const items = surface?.root.children[0]?.children;
    expect(items?.map(item => item.kind)).toEqual(['Text', 'Button', 'Button']);
    expect(items?.[2]?.children[0]?.props.text).toBe('Open HN');
    expect(eventMessage(items![1]!, surface!.resolve(surface!.data))).toBe('load_more\n{"date":"2026-11-20"}');
    expect(items?.[2]?.props).toMatchObject({ openUrl: 'https://news.ycombinator.com' });
  });
  it('resolves openUrl from the current card data', () => {
    const surface = parseExpress('$/link="https://example.com/"\nroot=Button(Text("Open"), action=openUrl(url=$/link))')!;
    expect(surface.root.props.openUrl).toBe('https://example.com/');
    const updated = writeBinding(surface.data, ['link'], 'https://example.org/');
    expect(surface.render(updated)?.props.openUrl).toBe('https://example.org/');
  });
  it('resolves the URL from a formatted property', () => {
    const surface = parseExpress('root=Button(Text("Open"), action=openUrl(formatString("https://example.com/")))');
    expect(surface?.root.kind).toBe('Button');
    expect(surface?.root.props.openUrl).toBe('https://example.com/');
  });
  it.each([
    'root=Card(root)',
    'root=Button(Text("Go"), Event("go"))',
    '<a2ui>root=Text("partial")',
    'root=Text(formatString("${/unclosed"))',
    'root=Text(formatString("${now()}"))',
  ])('leaves invalid or unsupported input as source', source => {
    expect(parseExpress(source)).toBeNull();
  });
});

it('updates a nested bound field and preserves the original parsed rows', () => {
  const source = '$/rows=[{name:"A",n:1}]\nroot=Column([Slider(value=$/rows/0/n),Table($/rows,_,title="Counts")])';
  const surface = parseExpress(source)!;
  const [slider, table] = surface.root.children;
  const changed = writeBinding(surface.data, slider.bindings!.value, 7);
  expect(surface.render(changed)?.children[1].props).toMatchObject({ rows: [{ name: 'A', n: 7 }], title: 'Counts' });
  expect(table.props.rows).toEqual([{ name: 'A', n: 1 }]);
});

it('resolves nested template scopes and isolates writable row data', () => {
  const surface = parseExpress(`
$/ = {title:"Bags", orders:[{items:[{qty:1}]}, {items:[{qty:2},{qty:5000}]}]}
$/orders/0/items/0/qty = +1e1
n = @index(offset=1)
root = List(children=_template($/orders, group), direction="horizontal")
group = Column(_template($items, item))
item = Row([Text(n), Text(formatted), Slider(value=$qty), Text($/title)])
formatted = formatString("Qty: \${formatNumber(value: qty, decimals: 1, grouping: false)}")
`)!;
  const [first, second] = surface.root.children;
  const [number, quantity, slider, title] = second.children[1].children;
  expect([number.props.text, quantity.props.text, title.props.text]).toEqual([2, 'Qty: 5000.0', 'Bags']);
  const changed = writeBinding(surface.data, slider.bindings.value, 27);
  expect(surface.render(changed)?.children[1].children[1].children[1].props.text).toBe('Qty: 27.0');
  expect(first.children[0].children[1].props.text).toBe('Qty: 10.0');
});

it('uses the Basic Catalog number, currency, date, and plural formatting', () => {
  const surface = parseExpress(`
金额 = formatCurrency(1234.5, currency="USD", grouping=false)
root = Column([
  Text(金额), Text(formatDate("2026-11-20", "yyyy-MM-dd")),
  Text(pluralize(.5, other="parts")),
  Text(r"""raw \u{5c} characters""")
])
`)!;
  expect(surface.root.children.map(child => child.props.text)).toEqual(['$1234.50', '2026-11-20', 'parts', 'raw \\ characters']);
});

it('reads the live scoped userMessage on activation and keeps the context fallback', () => {
  const surface = parseExpress(`
$/items = [{qty:3}, {qty:7}]
root = List(_template($/items, cart))
cart = Row([
  Button(Text("Checkout"), action=Event("checkout", userMessage=formatString("Pay \${qty}"))),
  Button(Text("Save"), action=Event("save", context={quantity: $qty}))
])
`)!;
  const [checkout, save] = surface.root.children[1].children;
  const data = writeBinding(surface.data, ['items', '1', 'qty'], 8);
  expect(eventMessage(checkout, surface.resolve(data))).toBe('Pay 8');
  expect(eventMessage(save, surface.resolve(data))).toBe('save\n{"quantity":8}');
});

it('bounds repeated and circular templates', () => {
  expect(parseExpress('$/items/10000 = 1\nroot = List(_template($/items, Text("item")))')).toBeNull();
  expect(parseExpress('$/items = [1]\nroot = List(_template($/items, root))')).toBeNull();
});

it('expands a template again when a local choice changes the bound array', () => {
  const surface = parseExpress(`
$/selection = ["a"]
selected = $/selection
root = Column([ChoicePicker(value=selected), List(_template($/selection, Text(@index(1))))])
`)!;
  const [control, list] = surface.root.children;
  expect(list.children).toHaveLength(1);
  const data = writeBinding(surface.data, control.bindings.value, ['a', 'b']);
  expect(surface.render(data)?.children[1].children.map(item => item.props.text)).toEqual([1, 2]);
});
