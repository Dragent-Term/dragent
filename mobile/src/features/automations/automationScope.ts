import type { StatusTone } from "../../components";
import type { PreviewAutomationRun } from "../../preview/machineTypes";

/** Desktop-style scopes for the automation list: mine, teammates', run results. */
export type AutomationScope = "mine" | "team" | "inbox";

const SCOPES: readonly AutomationScope[] = ["mine", "team", "inbox"];

export function parseAutomationScope(value: unknown): AutomationScope {
  return SCOPES.find((scope) => scope === value) ?? "mine";
}

export function automationScopeOptions(mineCount: number, inboxCount: number): readonly {
  id: AutomationScope;
  label: string;
  hint: string;
}[] {
  return [
    { id: "mine", label: "Mine", hint: `My automations, ${mineCount} sample${mineCount === 1 ? "" : "s"}` },
    { id: "team", label: "Team", hint: "Teammate automations" },
    { id: "inbox", label: "Inbox", hint: `Sample run results, ${inboxCount} result${inboxCount === 1 ? "" : "s"}` },
  ];
}

const runDateOptions = {
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
} as const;

/** One shared run-date format honoring the automation's time zone. */
export function formatAutomationRunAt(timeZone: string | null | undefined, at: number): string {
  try {
    return new Intl.DateTimeFormat(
      undefined,
      timeZone ? { ...runDateOptions, timeZone } : runDateOptions,
    ).format(at);
  } catch {
    return new Intl.DateTimeFormat(undefined, runDateOptions).format(at);
  }
}

/** Actual run statuses only — the sample set records successes today. */
export const AUTOMATION_RUN_STATUS: Record<
  PreviewAutomationRun["status"],
  { label: string; tone: StatusTone }
> = {
  succeeded: { label: "Succeeded", tone: "success" },
};
