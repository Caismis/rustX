import type { WorkflowDocument } from '../src/app/workflows/document/model';
import { setNodePresentation } from '../src/app/workflows/document/geometry';
const literal=(value:string)=>({type:'literal' as const,value});
export function dataNodesDocument():WorkflowDocument {
  let document:WorkflowDocument={description:'Workflow data nodes',block:{
    input:{type:'object',properties:{query:{type:'string'},priority:{type:'number'},memo:{type:'object',properties:{},additionalProperties:false},numbers:{type:'array',items:{type:'integer'}}},required:['query','priority','memo','numbers'],additionalProperties:false},
    nodes:{start:{type:'start'},
      classify:{type:'classifier',model:{mode:'session'},input:{type:'reference',path:['args','query']},instruction:[literal('Choose the best class.')],classes:[{id:'class_1',label:'CLASS 1',description:[literal('Support requests')]},{id:'class_2',label:'CLASS 2',description:[literal('Other questions')]}]},
      left:{type:'template',variables:{arg1:{type:'reference',path:['args','query']}},template:'{{ arg1 | upper }}'},
      right:{type:'template',variables:{arg1:{type:'reference',path:['args','query']}},template:'{{ arg1 | lower }}'},
      merge:{type:'aggregate',groups:[{name:'output',output_type:'string',variables:[['left','output'],['right','output']]}]},
      list:{type:'list',input:{type:'reference',path:['args','numbers']}},
      repeat:{type:'loop',loop_count:1,loop_variables:[{label:'count',var_type:'number',value:{type:'literal',value:0}}],body:{nodes:{inner_start:{type:'start'}},edges:[]}},
      done:{type:'return',outputs:[{variable:'answer',value_selector:['merge','output']},{variable:'numbers',value_selector:['list','result']}]},
    },edges:[{from:'start',to:'classify'},{from:'classify',to:'left',port:'class_1'},{from:'classify',to:'right',port:'class_2'},{from:'left',to:'merge'},{from:'right',to:'merge'},{from:'merge',to:'list'},{from:'list',to:'repeat'},{from:'repeat',to:'done'}],
  }};
  for (const [id,x,y] of [['classify',300,100],['left',620,40],['right',620,320],['merge',960,100],['list',1280,100],['repeat',1280,420],['done',1660,100]] as const)
    document=setNodePresentation(document,[],id,{position:{x,y}});
  return document;
}
