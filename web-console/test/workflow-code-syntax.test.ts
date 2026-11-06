import { expect,it } from 'vitest';
import { synchronizeMain } from '../src/app/workflows/ui/editor/forms/code-syntax';
import { nodeTemplate } from '../src/app/workflows/document/commands';

it('creates Dify Python defaults with two unbound inputs and one string output',()=>{
  const node=nodeTemplate('code');
  expect(node).toMatchObject({type:'code',code_language:'python3',variables:{arg1:{type:'reference',path:[]},arg2:{type:'reference',path:[]}},outputs:{result:'string'}});
  expect(node).toHaveProperty('code','\ndef main(arg1: str, arg2: str):\n    return {\n        "result": arg1 + arg2,\n    }\n');
});

it('syncs Python annotations from native types and preserves nested defaults, comments and the body',()=>{
  const source='# def main(fake):\ndef main(\n    old: dict[str, tuple[int, str]] = {"a": (1,"b")}\n) -> dict:\n    return {"result":"def main(still_not_a_signature)"}\n';
  expect(synchronizeMain('python3',source,[{name:'text',schema:{type:'string'}},{name:'items',schema:{type:'array',items:{type:'number'}}},{name:'flag',schema:{type:'boolean'}}])).toBe('# def main(fake):\ndef main(text: str, items: list[float], flag: bool) -> dict:\n    return {"result":"def main(still_not_a_signature)"}\n');
});

it('syncs JavaScript destructured inputs while preserving async and nested expressions in the body',()=>{
  const source='// function main(fake)\nasync function main({old = (()=>({a:1}))()}) {\n  return {result: "function main(fake)"};\n}';
  expect(synchronizeMain('javascript',source,[{name:'arg1'},{name:'items'}])).toBe('// function main(fake)\nasync function main({arg1, items}) {\n  return {result: "function main(fake)"};\n}');
});

it('does not rewrite nested main functions, strings, or invalid parameter names',()=>{
  expect(synchronizeMain('javascript','const helper=()=>{function main(fake){}};',[{name:'arg1'}])).toBeUndefined();
  expect(synchronizeMain('python3','class Helper:\n    def main(self): pass\n',[{name:'arg1'}])).toBeUndefined();
  expect(synchronizeMain('python3','def main(old): return {}',[{name:'class'}])).toBeUndefined();
  expect(synchronizeMain('javascript','function main(old){return {}}',[{name:'default'}])).toBeUndefined();
});
