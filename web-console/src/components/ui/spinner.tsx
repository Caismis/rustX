import { Loader2Icon } from "lucide-react";
import type React from "react";
import { cn } from "@/lib/utils";
import { useTranslation } from "@/locale/react";

export function Spinner({
  className,
  ...props
}: React.ComponentProps<typeof Loader2Icon>): React.ReactElement {
  const tx = useTranslation();
  return (
    <Loader2Icon
      aria-label={tx("common:activity.loading")}
      className={cn("animate-spin", className)}
      role="status"
      {...props}
    />
  );
}
