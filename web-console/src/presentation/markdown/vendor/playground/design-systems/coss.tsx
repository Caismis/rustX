"use client";

/**
 * coss ui: the catalog drawn with coss ui's Base UI and Tailwind primitives. Each component
 * here takes the same props the standard (or playground) component of that name takes. Only leaves are
 * swapped: a container (Card, Section) keeps its own implementation and picks up coss's palette from
 * the theme, so switching never remounts what is inside it (typed values, a table's sort, a tab).
 */
import type { ReactNode } from "react";
import {
  ArrowDownRight,
  ArrowUpRight,
  CircleAlert,
  CircleCheck,
  Info,
  Minus,
  TriangleAlert,
} from "lucide-react";
import { useGenerativeUIAccessibility } from "@copilotkit/react-core/v2/generative-ui";
import { asArray } from "../a2ui/catalog/renderers/format";
import type { RendererMap } from "../a2ui/catalog/renderers/types";
import { Alert, AlertDescription, AlertTitle } from "../components/ui/alert";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Card } from "../components/ui/card";
import { Progress } from "../components/ui/progress";
import { Separator } from "../components/ui/separator";
import { cn } from "../lib/utils";
import { str, trendOf, useButton, valueText } from "./shared";
import type { StandardProps } from "./shared";

function CossCard({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return (
    <Card className={cn("w-full min-w-0 gap-4 p-5", className)}>
      {children}
    </Card>
  );
}

function TrendBadge({ delta, trend }: { delta?: string; trend?: string }) {
  if (!delta) return null;
  const t = trendOf(delta, trend);
  const Icon =
    t === "down" ? ArrowDownRight : t === "flat" ? Minus : ArrowUpRight;
  return (
    <Badge
      variant={t === "down" ? "error" : t === "flat" ? "secondary" : "success"}
      className="tabular-nums"
    >
      <Icon />
      {delta}
    </Badge>
  );
}

const softTone = {
  danger: "error",
  success: "success",
  warning: "warning",
  info: "info",
} as const;

function CossBadge(p: StandardProps) {
  const aria = useGenerativeUIAccessibility();
  const tone = softTone[p.color as keyof typeof softTone];
  return (
    <Badge
      {...aria}
      variant={
        p.color === "danger" && p.variant === "solid"
          ? "destructive"
          : tone && p.variant !== "solid"
            ? tone
            : p.variant === "solid"
              ? "default"
              : p.variant === "outline"
                ? "outline"
                : "secondary"
      }
    >
      {str(p.label)}
    </Badge>
  );
}

function CossButton(p: StandardProps) {
  const { content, attributes } = useButton(p);
  return (
    <Button
      {...attributes}
      variant={
        p.color === "danger"
          ? "destructive"
          : p.variant === "primary" || p.variant === "solid"
            ? "default"
            : p.variant === "soft"
              ? "secondary"
              : p.variant === "borderless" || p.variant === "ghost"
                ? "ghost"
                : "outline"
      }
      size={p.size === "sm" || p.size === "xs" ? "sm" : "default"}
      className={p.block ? "w-full" : undefined}
    >
      {content}
    </Button>
  );
}

export const cossStandard: Record<string, (p: StandardProps) => ReactNode> = {
  Badge: CossBadge,
  Button: CossButton,
  Divider: (p) => (
    <Separator
      orientation={p.axis === "vertical" ? "vertical" : "horizontal"}
    />
  ),
};

type Own =
  | "Stat"
  | "StatGroup"
  | "KeyValueList"
  | "Heading"
  | "Alert"
  | "Callout"
  | "Progress";

export const cossOwn: Partial<RendererMap<Own>> = {
  Stat: ({ props }) => (
    <CossCard className="gap-2 py-4">
      <span className="truncate text-sm text-muted-foreground">
        {str(props.label)}
      </span>
      <span className="text-2xl font-semibold tabular-nums">
        {valueText(props.value)}
      </span>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <TrendBadge delta={props.delta} trend={props.trend} />
        {props.caption ? (
          <span className="text-xs text-muted-foreground">{props.caption}</span>
        ) : null}
      </div>
    </CossCard>
  ),
  StatGroup: ({ props }) => (
    <div className="grid w-full min-w-0 grid-cols-[repeat(auto-fit,minmax(150px,1fr))] gap-3">
      {asArray<Record<string, unknown>>(props.items)
        .filter((it) => !!it && typeof it === "object" && !Array.isArray(it))
        .map((it, i) => (
        <CossCard key={i} className="gap-2 py-4">
          <span className="text-sm text-muted-foreground">{str(it.label)}</span>
          <span className="text-2xl font-semibold tabular-nums">
            {valueText(it.value)}
          </span>
          <TrendBadge delta={str(it.delta)} trend={str(it.trend)} />
        </CossCard>
      ))}
    </div>
  ),
  KeyValueList: ({ props }) => (
    <CossCard className="gap-2">
      {props.title ? (
        <div className="font-medium leading-none">{props.title}</div>
      ) : null}
      <dl className="flex flex-col">
        {asArray<{ label?: string; value?: unknown; hint?: string }>(
          props.items,
        )
          .filter((it) => !!it && typeof it === "object" && !Array.isArray(it))
          .map((it, i) => (
          <div
            key={i}
            className="flex items-baseline justify-between gap-4 border-b py-2 text-sm last:border-b-0"
          >
            <dt className="text-muted-foreground">{str(it.label)}</dt>
            <dd className="text-right font-medium tabular-nums">
              {valueText(it.value)}
              {it.hint ? (
                <span className="block text-xs font-normal text-muted-foreground">
                  {it.hint}
                </span>
              ) : null}
            </dd>
          </div>
        ))}
      </dl>
    </CossCard>
  ),
  Heading: ({ props }) => (
    <header className="flex min-w-0 flex-col gap-1">
      {props.eyebrow ? (
        <span className="text-sm font-medium text-muted-foreground">
          {props.eyebrow}
        </span>
      ) : null}
      <h2
        className={cn(
          "font-semibold tracking-tight",
          props.level === "1"
            ? "text-3xl"
            : props.level === "3"
              ? "text-lg"
              : "text-2xl",
        )}
      >
        {props.text}
      </h2>
      {props.subtitle ? (
        <p className="text-sm text-muted-foreground">{props.subtitle}</p>
      ) : null}
    </header>
  ),
  // coss's Alert is role="alert"; only an error should interrupt a screen reader, as the default Alert does.
  Alert: ({ props }) => {
    const tone = props.tone ?? "info";
    const Icon =
      tone === "error"
        ? CircleAlert
        : tone === "warning"
          ? TriangleAlert
          : tone === "success"
            ? CircleCheck
            : Info;
    return (
      <Alert role={tone === "error" ? "alert" : "status"} variant={tone}>
        <Icon aria-hidden />
        {props.title ? <AlertTitle>{props.title}</AlertTitle> : null}
        <AlertDescription>{props.message}</AlertDescription>
      </Alert>
    );
  },
  Callout: ({ props }) => (
    <Alert role={undefined}>
      {props.title ? <AlertTitle>{props.title}</AlertTitle> : null}
      <AlertDescription className="flex flex-col items-start gap-3">
        {props.text}
        {props.actionLabel && props.action ? (
          <Button size="sm" variant="outline" onClick={props.action}>
            {props.actionLabel}
          </Button>
        ) : null}
      </AlertDescription>
    </Alert>
  ),
  Progress: ({ props }) => {
    const raw =
      typeof props.value === "number" && Number.isFinite(props.value)
        ? props.value
        : 0;
    const value = Math.max(0, Math.min(100, raw));
    return (
      <div className="flex w-full min-w-0 flex-col gap-2">
        <div className="flex items-baseline justify-between gap-2 text-sm">
          <span className="font-medium">{props.label}</span>
          <span className="tabular-nums text-muted-foreground">
            {Math.round(value)}%
          </span>
        </div>
        <Progress value={value} />
        {props.caption ? (
          <span className="text-xs text-muted-foreground">{props.caption}</span>
        ) : null}
      </div>
    );
  },
};
