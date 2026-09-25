import { useReducer, type RefObject } from 'react';
import { discoveryQuery } from '../commands/registry';

type Trigger = { source: 'typed' | 'launcher'; query: string; highlight: number } | undefined;
type Event = { type: 'track'; draft: string } | { type: 'toggle' } | { type: 'dismiss' } | { type: 'highlight'; index: number };

/** One owner for both entry gestures. A launcher never edits the draft. */
export function inputTrigger(state: Trigger, event: Event): Trigger {
  switch (event.type) {
    case 'track': {
      const query = discoveryQuery(event.draft);
      return query === undefined ? undefined : { source: 'typed', query, highlight: 0 };
    }
    case 'toggle': return state?.source === 'launcher' ? undefined : { source: 'launcher', query: '', highlight: 0 };
    case 'dismiss': return undefined;
    case 'highlight': return state && { ...state, highlight: event.index };
  }
}

export function useInputTrigger(input: RefObject<HTMLTextAreaElement | null>) {
  const [state, dispatch] = useReducer(inputTrigger, undefined);
  const restore = () => input.current?.focus({ preventScroll: true });
  return {
    state,
    track: (draft: string) => dispatch({ type: 'track', draft }),
    toggle: () => { dispatch({ type: 'toggle' }); restore(); },
    dismiss: () => dispatch({ type: 'dismiss' }),
    highlight: (index: number) => dispatch({ type: 'highlight', index }),
    restore,
  };
}
