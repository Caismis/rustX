import type { NodeDefinition } from './model';

export type CodeNode=Extract<NodeDefinition,{type:'code'}>;
export type CodeLanguage=CodeNode['code_language'];

/** Dify/Graphon creation defaults, with native selectors and output types. */
export function codeDefaults(code_language:CodeLanguage='python3'):CodeNode {
  return {type:'code',code_language,variables:{arg1:{type:'reference',path:[]},arg2:{type:'reference',path:[]}},
    code:code_language==='python3'
      ? /* i18n-raw: authored Python starter source */ '\ndef main(arg1: str, arg2: str):\n    return {\n        "result": arg1 + arg2,\n    }\n'
      : /* i18n-raw: authored JavaScript starter source */ '\nfunction main({arg1, arg2}) {\n    return {\n        result: arg1 + arg2\n    }\n}\n',
    outputs:{result:'string'}};
}
