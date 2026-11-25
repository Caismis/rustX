import { useTranslation } from "../../../../../../locale/react";
import { cn } from "../../lib/utils";
import { Loader2Icon } from "lucide-react";
import type React from "react";

export function Spinner({
  className,
  ...props
}: React.ComponentProps<typeof Loader2Icon>): React.ReactElement {
  const tx = useTranslation();
  return (
    <Loader2Icon
      aria-label={tx("playground:loading")}
      className={cn("animate-spin", className)}
      role="status"
      {...props}
    />
  );
}
