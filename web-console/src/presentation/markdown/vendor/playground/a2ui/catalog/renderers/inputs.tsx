"use client";

import { useTranslation } from "../../../../../../../locale/react";
import { useId, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { format as formatDate, isValid, parseISO } from "date-fns";
import { CalendarDays, Check, ChevronsUpDown, Search } from "lucide-react";
import { Button } from "../../../components/ui/button";
import { Calendar } from "../../../components/ui/calendar";
import {
  Combobox,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
  ComboboxPopup,
  ComboboxTrigger,
} from "../../../components/ui/combobox";
import { Label } from "../../../components/ui/label";
import {
  Popover,
  PopoverPopup,
  PopoverTrigger,
} from "../../../components/ui/popover";
import { Switch } from "../../../components/ui/switch";
import { ToggleGroup, ToggleGroupItem } from "../../../components/ui/toggle-group";
import { cn } from "../../../lib/utils";
import { asArray } from "./format";
import type { RendererMap } from "./types";
import { useBound } from "./use-bound";

type Opt = { value: string; label: string; description?: string };

function cleanOptions(options: unknown): Opt[] {
  return asArray<Opt>(options)
    .filter((o) => !!o && typeof o === "object" && typeof o.value === "string")
    .map((o) => ({
      ...o,
      label: typeof o.label === "string" && o.label ? o.label : o.value,
    }));
}

function Field({
  id,
  label,
  hint,
  right,
  children,
}: {
  id: string;
  label?: string;
  hint?: string;
  right?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="a2-enter flex w-full min-w-0 flex-col gap-1.5">
      {label || right ? (
        <div className="flex min-w-0 items-baseline justify-between gap-3">
          {label ? (
            <Label
              htmlFor={id}
              className="truncate text-[12.5px] font-medium text-foreground/90"
            >
              {label}
            </Label>
          ) : (
            <span />
          )}
          {right}
        </div>
      ) : null}
      {children}
      {hint ? (
        <p className="text-[11px] text-muted-foreground">{hint}</p>
      ) : null}
    </div>
  );
}

const control = "h-8.5 text-[13px] sm:h-8.5";

type InputKeys =
  | "Combobox"
  | "Switch"
  | "DatePicker"
  | "ButtonGroup"
  | "ChipSelect";

export const inputRenderers: RendererMap<InputKeys> = {
  Combobox: ({ props }) => {
    const tx = useTranslation();
    const id = useId();
    const [value, setValue] = useBound<string>(props.value, props.setValue, "");
    const options = cleanOptions(props.options);
    const selected = options.find((o) => o.value === value) ?? null;
    return (
      <Field id={id} label={props.label}>
        <Combobox
          items={options}
          value={selected}
          onValueChange={(o: Opt | null) => {
            if (o) setValue(o.value);
          }}
          isItemEqualToValue={(a: Opt, b: Opt) => a.value === b.value}
        >
          <ComboboxTrigger
            id={id}
            render={
              <Button
                variant="outline"
                className={cn(
                  control,
                  "w-full min-w-0 justify-between font-normal",
                )}
              />
            }
          >
            <span
              className={cn("truncate", !selected && "text-muted-foreground")}
            >
              {selected?.label ?? props.placeholder ?? tx("playground:searchOptions")}
            </span>
            <ChevronsUpDown className="-me-1! text-muted-foreground" />
          </ComboboxTrigger>
          <ComboboxPopup aria-label={props.label ?? tx("playground:searchOptions")}>
            <div className="border-b p-2">
              <ComboboxInput
                className="rounded-md before:rounded-[calc(var(--radius-md)-1px)]"
                placeholder={tx("playground:search")}
                showTrigger={false}
                startAddon={<Search />}
              />
            </div>
            <ComboboxEmpty>{tx("playground:noOptions")}</ComboboxEmpty>
            <ComboboxList>
              {(o: Opt) => (
                <ComboboxItem key={o.value} value={o} className="text-[13px]">
                  {o.label}
                </ComboboxItem>
              )}
            </ComboboxList>
          </ComboboxPopup>
        </Combobox>
      </Field>
    );
  },

  Switch: ({ props }) => {
    const id = useId();
    const [checked, setChecked] = useBound<boolean>(
      props.value,
      props.setValue,
      false,
    );
    return (
      <div className="a2-enter a2-well flex w-full min-w-0 items-center justify-between gap-4 px-3.5 py-3">
        <div className="flex min-w-0 flex-col gap-0.5">
          <Label
            htmlFor={id}
            className="text-[13px] font-medium text-foreground/90"
          >
            {props.label}
          </Label>
          {props.description ? (
            <p className="text-[11.5px] leading-relaxed text-muted-foreground">
              {props.description}
            </p>
          ) : null}
        </div>
        <Switch id={id} checked={checked} onCheckedChange={setChecked} />
      </div>
    );
  },

  DatePicker: ({ props }) => {
    const tx = useTranslation();
    const id = useId();
    const [open, setOpen] = useState(false);
    const [value, setValue] = useBound<string>(props.value, props.setValue, "");
    const date = useMemo(() => {
      if (!value) return undefined;
      const d = parseISO(value);
      return isValid(d) ? d : undefined;
    }, [value]);
    return (
      <Field id={id} label={props.label}>
        <Popover open={open} onOpenChange={setOpen}>
          <PopoverTrigger
            id={id}
            render={
              <Button
                variant="outline"
                className={cn(
                  control,
                  "w-full min-w-0 justify-start font-normal",
                )}
              />
            }
          >
            <CalendarDays className="text-muted-foreground" />
              <span
                className={cn("truncate", !date && "text-muted-foreground")}
              >
                {date
                  ? formatDate(date, "EEE, MMM d, yyyy")
                  : (props.placeholder ?? tx("playground:pickDate"))}
              </span>
          </PopoverTrigger>
          <PopoverPopup align="start">
            <Calendar
              mode="single"
              selected={date}
              defaultMonth={date}
              onSelect={(d) => {
                if (d) setValue(formatDate(d, "yyyy-MM-dd"));
                setOpen(false);
              }}
            />
          </PopoverPopup>
        </Popover>
      </Field>
    );
  },

  ButtonGroup: ({ props }) => {
    const id = useId();
    const [value, setValue] = useBound<string>(props.value, props.setValue, "");
    const options = cleanOptions(props.options);
    return (
      <Field id={id} label={props.label}>
        <ToggleGroup
          id={id}
          variant="outline"
          size="sm"
          value={value ? [value] : []}
          onValueChange={(v: string[]) => {
            if (v[0]) setValue(v[0]);
          }}
          className="max-w-full overflow-x-auto"
        >
          {options.map((o) => (
            <ToggleGroupItem
              key={o.value}
              value={o.value}
              className="px-3 text-[12.5px]"
            >
              {o.label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </Field>
    );
  },

  ChipSelect: ({ props }) => {
    const tx = useTranslation();
    const id = useId();
    const [value, setValue] = useBound<string[]>(
      props.value,
      props.setValue,
      [],
    );
    const options = cleanOptions(props.options);
    const selected = Array.isArray(value) ? value : [];
    return (
      <Field
        id={id}
        label={props.label}
        right={
          selected.length ? (
            <span className="text-[11px] text-muted-foreground tabular-nums">
              {tx("playground:selected", { count: selected.length })}
            </span>
          ) : null
        }
      >
        <ToggleGroup
          id={id}
          multiple
          size="sm"
          value={selected}
          onValueChange={(v: string[]) => setValue(v)}
          className="flex-wrap justify-start gap-1.5"
        >
          {options.map((o) => {
            const on = selected.includes(o.value);
            return (
              <ToggleGroupItem
                key={o.value}
                value={o.value}
                className={cn(
                  "gap-1 rounded-full border-input px-3 text-[12.5px] before:rounded-full dark:bg-input/32",
                  on && "border-primary/40",
                )}
              >
                {on ? <Check className="size-3" strokeWidth={2.75} /> : null}
                {o.label}
              </ToggleGroupItem>
            );
          })}
        </ToggleGroup>
      </Field>
    );
  },
};
