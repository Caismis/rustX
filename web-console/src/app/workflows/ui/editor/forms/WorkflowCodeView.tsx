import { useLayoutEffect, useMemo, useRef } from 'react';
import CodeMirror, { EditorView, ExternalChange, Prec, type ReactCodeMirrorRef } from '@uiw/react-codemirror';
import { syntaxHighlighting } from '@codemirror/language';
import { python } from '@codemirror/lang-python';
import { javascript } from '@codemirror/lang-javascript';
import { jinja } from '@codemirror/lang-jinja';
import type { CodeLanguage } from '../../../document/code';
import { classHighlighter } from '@lezer/highlight';

const languages={python3:python(),javascript:javascript(),jinja2:jinja()};
const highlighting=[syntaxHighlighting(classHighlighter),EditorView.lineWrapping];
const setup={syntaxHighlighting:false,foldGutter:false,highlightActiveLine:false,highlightActiveLineGutter:false,autocompletion:false,highlightSelectionMatches:false};

export interface VariableInsertion {
  source: string;
  from: number;
  to: number;
  rect: DOMRect;
  /** Refuse a stale completion after the source or caret has changed. */
  current: () => boolean;
  complete: (name: string) => boolean;
  focus: () => void;
}

export default function WorkflowCodeView({language,label,value,onChange,onBlur,field,onVariableRequest}:{language:CodeLanguage|'jinja2';label:string;value:string;onChange:(value:string)=>void;onBlur:()=>void;field:string;onVariableRequest?:(request:VariableInsertion)=>void}) {
  const editor=useRef<ReactCodeMirrorRef>(null);
  const request=useRef(onVariableRequest);request.current=onVariableRequest;
  // Template insertion must run before CodeMirror pairs the opening brace.
  const extensions=useMemo(()=>[languages[language],...highlighting,EditorView.contentAttributes.of({'aria-label':label,'data-workflow-field':field,spellcheck:'false'}),Prec.highest(EditorView.inputHandler.of((view,from,to,text)=>{
    if(!request.current || view.composing || (text!=='/' && text!=='{'))return false;
    view.dispatch({changes:{from,to,insert:text},selection:{anchor:from+1},userEvent:'input.type'});
    const source=view.state.doc.toString(),pos=from+1,rect=view.coordsAtPos(pos);
    if(rect) {
      const current=()=>view.dom.isConnected && view.state.doc.toString()===source && view.state.selection.main.head===pos;
      request.current({source,from,to:pos,rect:new DOMRect(rect.left,rect.top,rect.right-rect.left,rect.bottom-rect.top),current,focus:()=>{if(view.dom.isConnected)view.focus();},complete:name=>{
        if(!current())return false;
        const insert=`{{ ${name} }}`;
        view.dispatch({changes:{from,to:pos,insert},selection:{anchor:from+insert.length},userEvent:'input.complete'});
        return true;
      }});
    }
    return true;
  }))],[language,label,field]);
  useLayoutEffect(()=>{const view=editor.current?.view;if(view && view.state.doc.toString()!==value)view.dispatch({changes:{from:0,to:view.state.doc.length,insert:value},annotations:ExternalChange.of(true)});},[value]);
  return <CodeMirror ref={editor} value={value} onChange={onChange} onBlur={onBlur} theme="none" height="100%" extensions={extensions} basicSetup={setup} indentWithTab={false}/>;
}
