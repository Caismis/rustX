import { useState } from 'react';
import type { ComponentProps } from 'react';
import { BackgroundJobItem } from '../src/app/agent/BackgroundJobItem';

/** Direct row fixture; menu grouping and exclusive expansion are tested through the real menu. */
export function JobItemHarness(props: Pick<ComponentProps<typeof BackgroundJobItem>, 'job' | 'client' | 'sessionId'>) {
  const [expanded, setExpanded] = useState(false), [armed, setArmed] = useState(false);
  const [accepted, setAccepted] = useState<{ current: () => boolean }>();
  return <ul><BackgroundJobItem {...props} now={Date.now()} stopping={!!accepted?.current()} onStopped={current => setAccepted({ current })} expanded={expanded} onToggle={() => setExpanded(value => !value)} armed={armed} onArm={() => setArmed(value => !value)}/></ul>;
}
