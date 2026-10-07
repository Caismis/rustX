/* Copyright (c) 2026 DeepSeek. MIT. Harness terminal presentation adapted to rustX PTY; see PROVENANCE.md. */
import { useEffect, useRef, useState } from 'react';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import type { WorkbenchRequest, WorkbenchResult } from '../../workspaces/workbench';
import { useTranslation } from '../../locale/react';
import { TerminalTheme } from './WorkbenchTerminalTheme';
import css from '../../presentation/right-panel/Workbench.module.css';
export default function WorkbenchTerminal({ id, call }: { id: string; call: (request: WorkbenchRequest, signal?: AbortSignal) => Promise<WorkbenchResult> }) {
  const tx = useTranslation(), root = useRef<HTMLDivElement>(null);
  const [error, setError] = useState(''), [exited, setExited] = useState(false);
  useEffect(() => {
    const abort = new AbortController();
    const terminal = new Terminal({ cursorBlink: true, scrollback: 3000, fontSize: 13, minimumContrastRatio: 4.5, fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace' });
    terminal.attachCustomKeyEventHandler(e => !(e.ctrlKey && !e.altKey && !e.metaKey && e.code === 'Backquote' || e.altKey && (e.metaKey || e.ctrlKey) && e.code === 'KeyP'));
    const fit = new FitAddon(); terminal.loadAddon(fit); terminal.open(root.current!);
    const palette = new TerminalTheme(terminal);
    const updateTheme = () => { const style = getComputedStyle(root.current!); palette.update(style.backgroundColor, style.color); };
    updateTheme(); const themeObserver = new MutationObserver(updateTheme); themeObserver.observe(document.body, { attributes: true, attributeFilter: ['data-ds-dark-theme', 'style', 'class'] }); themeObserver.observe(document.documentElement, { attributes: true });
    let input = Promise.resolve(), inputFailed = false;
    const report = (cause: unknown) => { if (!abort.signal.aborted) { inputFailed = true; setError(String(cause)); terminal.options.disableStdin = true; } };
    const send = (request: WorkbenchRequest) => { input = input.then(async () => { if (!abort.signal.aborted && !inputFailed) await call(request, abort.signal); }).catch(report); };
    const data = terminal.onData(data => send({ kind: 'input', id, data }));
    const resize = terminal.onResize(({ cols, rows }) => send({ kind: 'resize', id, cols, rows }));
    const observer = new ResizeObserver(() => { if (root.current?.clientWidth && root.current.clientHeight) fit.fit(); }); observer.observe(root.current!); fit.fit(); if (document.activeElement?.getAttribute('role') !== 'tab') terminal.focus();
    void (async () => { let cursor = 0; try { while (!abort.signal.aborted) { const result = await call({ kind: 'poll', id, cursor }, abort.signal); if (abort.signal.aborted) break; if (result.reset) terminal.reset(); await new Promise<void>(resolve => terminal.write(result.output ?? '', resolve)); cursor = result.cursor!; if (result.exited) { terminal.options.disableStdin = true; setExited(true); break; } } } catch (cause) { report(cause); } })();
    return () => { abort.abort(); themeObserver.disconnect(); palette.dispose(); observer.disconnect(); data.dispose(); resize.dispose(); terminal.dispose(); };
  }, [id, call]);
  return <section className={css.terminal} aria-label={tx('artifacts:workbench.terminal')}>{error && <p role="alert">{error}</p>}{exited && <p role="status">{tx('artifacts:workbench.exited')}</p>}<div ref={root} className={css.terminalBody}/></section>;
}
