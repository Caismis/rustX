import { useLayoutEffect, useMemo, useRef } from 'react';
import CodeMirror, { EditorView, ExternalChange, type ReactCodeMirrorRef } from '@uiw/react-codemirror';
import { StreamLanguage, syntaxHighlighting } from '@codemirror/language';
import { shell } from '@codemirror/legacy-modes/mode/shell';
import { classHighlighter } from '@lezer/highlight';

const language=[StreamLanguage.define(shell),syntaxHighlighting(classHighlighter),EditorView.lineWrapping];
const setup={syntaxHighlighting:false,foldGutter:false,highlightActiveLine:false,highlightActiveLineGutter:false,autocompletion:false,highlightSelectionMatches:false};

export default function BashCodeView({label,value,onChange,onBlur,field}:{label:string;value:string;onChange:(value:string)=>void;onBlur:()=>void;field:string}) {
  const editor=useRef<ReactCodeMirrorRef>(null);
  const extensions=useMemo(()=>[...language,EditorView.contentAttributes.of({'aria-label':label,'data-workflow-field':field,spellcheck:'false'})],[label,field]);
  useLayoutEffect(()=>{const view=editor.current?.view;if(view && view.state.doc.toString()!==value)view.dispatch({changes:{from:0,to:view.state.doc.length,insert:value},annotations:ExternalChange.of(true)});},[value]);
  return <CodeMirror ref={editor} value={value} onChange={onChange} onBlur={onBlur} theme="none" height="100%" extensions={extensions} basicSetup={setup} indentWithTab={false}/>;
}
