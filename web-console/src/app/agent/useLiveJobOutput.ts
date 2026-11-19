import { useEffect, useState, useSyncExternalStore } from 'react';
import type { AppServerClient } from '../../client/app-server';

export interface LiveJobOutput {
  text: string;
  truncated: boolean;
  error?: string;
}

const POLL_MS = 1000;
const noopSubscribe = () => () => {};
const noProof = () => undefined;

/** Only an expanded, live job reads the conversation-owned live output tail. */
export function useLiveJobOutput(client: AppServerClient | undefined, session: string | undefined, job: string | undefined, polling = true): LiveJobOutput | undefined {
  const proof = useSyncExternalStore(
    client?.subscribe ?? noopSubscribe,
    client && session ? () => client.getSnapshot().views[session]?.attachmentObservation : noProof,
  );
  const [read, setRead] = useState<{ job: string; proof: typeof proof; output: LiveJobOutput }>();

  useEffect(() => {
    if (!client || !session || !job || !proof || !polling) return;
    let observing = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const current = () => observing && client.isAttachmentObservationCurrent(session, proof);
    const sample = async () => {
      if (!current()) return;
      try {
        const { text, truncated } = await client.request({ method: 'job/output', params: { target: proof.target, job_id: job } }, 'job_output', undefined, current);
        if (current()) setRead({ job, proof, output: { text, truncated } });
      } catch (cause) {
        if (current()) {
          const message = cause instanceof Error ? cause.message : String(cause);
          setRead(previous => ({
            job,
            proof,
            output: {
              text: previous?.proof === proof && previous.job === job ? previous.output.text : '',
              truncated: previous?.proof === proof && previous.job === job && previous.output.truncated,
              error: message,
            },
          }));
        }
      }
      if (current()) timer = setTimeout(() => { void sample(); }, POLL_MS);
    };
    void sample();
    return () => {
      observing = false;
      clearTimeout(timer);
    };
  }, [client, session, job, proof, polling]);

  return read && read.job === job && read.proof === proof ? read.output : undefined;
}
