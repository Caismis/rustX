import { pythonLanguage } from '@codemirror/lang-python';
import { javascriptLanguage } from '@codemirror/lang-javascript';
import type { CodeLanguage } from '../../../document/code';
import { schemaObject } from '../../../document/schema';

/** Native selected-reference schemas supply annotations; unknown types stay unannotated. */
function pythonType(schema:unknown):string|undefined {
  const value=schemaObject(schema);
  switch(value?.type){
    case 'string': return /* i18n-raw: Python type name */ 'str';
    case 'number': case 'integer': return /* i18n-raw: Python type name */ 'float';
    case 'boolean': return /* i18n-raw: Python type name */ 'bool';
    case 'object': return /* i18n-raw: Python type name */ 'dict';
    case 'array': {const item=pythonType(value.items);return item?`list[${item}]`: /* i18n-raw: Python type name */ 'list';}
    default:return undefined;
  }
}

/** Replace only a real main declaration's parameters, preserving its body and comments. */
export function synchronizeMain(language:CodeLanguage,source:string,inputs:readonly {name:string;schema?:unknown}[]):string|undefined {
  // Lezer recovers bindings without semantic keyword validation. A shorthand
  // object binding still needs a legal JavaScript identifier.
  const reserved=new Set('await break case catch class const continue debugger default delete do else enum export extends false finally for function if implements import in instanceof interface let new null package private protected public return static super switch this throw true try typeof var void while with yield'.split(' '));
  if(language==='javascript'&&inputs.some(input=>reserved.has(input.name)))return undefined;
  const parser=language==='python3'?pythonLanguage.parser:javascriptLanguage.parser;
  const names=inputs.map(({name,schema})=>{const type=language==='python3'?pythonType(schema):undefined;return type?`${name}: ${type}`:name;});
  const params=language==='python3'?`(${names.join(', ')})`:names.length?`({${names.join(', ')}})`:'()';
  let valid=true;parser.parse(language==='python3'?`def main${params}:\n    pass`:`function main${params}{}`).iterate({enter(node){if(node.type.isError)valid=false;}});
  if(!valid)return undefined;
  let result:string|undefined;
  parser.parse(source).iterate({enter(node){
    if(['ClassDefinition','ClassDeclaration','ClassExpression','ArrowFunction','FunctionExpression','LambdaExpression'].includes(node.name))return false;
    const declaration=language==='python3'?'FunctionDefinition':'FunctionDeclaration';
    if(node.name!==declaration)return;
    const name=node.node.getChild(language==='python3'?'VariableName':'VariableDefinition'),parameters=node.node.getChild('ParamList');
    if(name && parameters && source.slice(name.from,name.to)==='main' && result===undefined)result=source.slice(0,parameters.from)+params+source.slice(parameters.to);
    return false;
  }});
  return result;
}
