import { useLayoutEffect, useMemo, useState } from 'react';
import { AttachmentIntake, type AttachmentIntakes } from '../../client/uploads';

/** Candidates are render-local; only a committed Composer activates ownership. */
export function useAttachmentIntake(owners: AttachmentIntakes, key: string, binding: string) {
  const candidate = useMemo(() => new AttachmentIntake(), [owners, key]);
  const intake = owners.lookup(key) ?? candidate;
  const [, refresh] = useState(0);
  useLayoutEffect(() => {
    const active = owners.activate(key, binding, intake);
    // A different commit may have selected this key since our render.
    if (active !== intake) refresh(value => value + 1);
  }, [owners, key, binding, intake]);
  return intake;
}
