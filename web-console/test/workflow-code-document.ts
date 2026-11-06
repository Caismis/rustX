import type { CodeLanguage } from '../src/app/workflows/document/code';
import type { WorkflowDocument } from '../src/app/workflows/document/model';
import { setNodePresentation, setOrder } from '../src/app/workflows/document/geometry';

export function codeDocument(language:CodeLanguage='python3'):WorkflowDocument {
  let document:WorkflowDocument={description:'Python/JavaScript input and typed output example',block:{
    input:{type:'object',properties:{query:{type:'string'},priority:{type:'number'},items:{type:'array',items:{type:'number'}}},required:['query','priority','items'],additionalProperties:false},
    nodes:{start:{type:'start'},code:{type:'code',code_language:language,variables:{arg1:{type:'reference',path:['args','query']}},code:language==='python3'?'def main(arg1):\n    return {"result":arg1,"count":2}\n':'function main({arg1}) {\n    return {result:arg1,count:2};\n}' ,outputs:{result:'string',count:'number'}},done:{type:'return',outputs:[{variable:'answer',value_selector:['code','result']},{variable:'count',value_selector:['code','count']}]}},
    edges:[{from:'start',to:'code'},{from:'code',to:'done'}]}};
  for(const [id,x,y] of [['code',340,100],['done',700,100]] as const)document=setNodePresentation(document,[],id,{position:{x,y}});
  return setOrder(document,{kind:'node_output',address:{block:[],node:'code'},path:[]},['result','count']);
}
