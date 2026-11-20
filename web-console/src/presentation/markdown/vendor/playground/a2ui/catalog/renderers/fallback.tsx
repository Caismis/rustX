"use client";

import { useTranslation } from "../../../../../../../locale/react";

/** Shown for any catalog component whose renderer is not implemented yet. */
export function PendingRenderer({ name }: { name: string }) {
  const tx = useTranslation();
  return (
    <div className="a2-card px-4 py-3 text-[12px] text-muted-foreground">
      <span className="font-mono text-foreground/80">{name}</span>{" "}
      {tx("playground:pendingRenderer")}
    </div>
  );
}
