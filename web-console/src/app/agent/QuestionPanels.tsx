import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from 'react';
import type { QuestionSpecification, QuestionnaireSubmission } from '../../../../protocol/app-server/v66';
import { Questionnaire } from './Questionnaire';
import type { AppServerClient } from '../../client/app-server';
import { sameValue, useClientSelector } from '../../client/selectors';

export interface QuestionRecord {
  questions: QuestionSpecification[];
  submission: QuestionnaireSubmission;
  form?: { eyebrow: string; title: string };
}
interface Panels {
  conversation?: string;
  record?: { key: string; value: QuestionRecord };
  show: (key: string, record: QuestionRecord) => void;
  close: () => void;
  reveal: (key: string) => boolean;
  canReveal: (key: string) => boolean;
  register: (keys: string[], reveal: () => void) => () => void;
}
const Context = createContext<Panels | undefined>(undefined);
export const useQuestionPanels = () => useContext(Context);
export const questionPanelKey = (conversation: string | undefined, kind: 'call' | 'interaction', id: string) => JSON.stringify([conversation, kind, id]);

/** Logical branch changes retire a panel even before the next attachment arrives. */
export function ConversationQuestionPanels({ client, sessionId, children }: { client: AppServerClient; sessionId?: string; children: ReactNode }) {
  const owner = useClientSelector(client, state => {
    const view = sessionId ? state.views[sessionId] : undefined;
    return { endpoint: state.endpoint, authority: state.authorityRevision, sessionId,
      node: view?.nodeId, conversation: view?.nodeConversationId ?? view?.target?.conversation_id };
  }, sameValue);
  return <QuestionPanels scope={JSON.stringify(owner)} conversation={owner.conversation}>{children}</QuestionPanels>;
}

/** Presentation only: a recorded answer never gets a native response channel. */
export function QuestionPanels({ scope, conversation, children }: { scope: string; conversation?: string; children: ReactNode }) {
  const [selected, select] = useState<{ scope: string; key: string; value: QuestionRecord }>();
  if (selected && selected.scope !== scope) select(undefined);
  const pending = useRef(new Map<string, () => void>());
  const [revision, refresh] = useState(0);
  const register = useCallback<Panels['register']>((keys, reveal) => {
    for (const key of keys) pending.current.set(key, reveal);
    refresh(value => value + 1);
    return () => {
      for (const key of keys) if (pending.current.get(key) === reveal) pending.current.delete(key);
      refresh(value => value + 1);
    };
  }, []);
  const value = useMemo<Panels>(() => ({
    conversation,
    record: selected?.scope === scope ? selected : undefined,
    show: (key, value) => select({ scope, key, value }),
    close: () => select(undefined),
    reveal: key => { const reveal = pending.current.get(key); if (!reveal) return false; select(undefined); reveal(); return true; },
    canReveal: key => pending.current.has(key),
    register,
  }), [scope, conversation, selected, revision, register]);
  return <Context value={value}>{children}</Context>;
}

export function RecordedQuestionPanel() {
  const panels = useQuestionPanels(), record = panels?.record;
  if (!record) return null;
  return <Questionnaire key={record.key} questions={record.value.questions} form={record.value.form}
    record={record.value.submission} onClose={panels.close}/>;
}
