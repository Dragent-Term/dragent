/** What the coding agents on a desktop used, as its Usage page shows it. */

export interface UsageTotals {
  readonly tokens: number;
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
  readonly replies: number;
  /** Estimated API cost in dollars of the priced part. */
  readonly cost: number;
  /** Tokens of models with no known price (left out of `cost`). */
  readonly unpricedTokens: number;
  readonly cacheSavings: number;
}

export interface UsageLimitWindow {
  /** "5-hour", "Weekly"… */
  readonly label: string;
  /** 0–100. */
  readonly usedPercent: number;
  /** Unix seconds when it starts over, when known. */
  readonly resetsAt: number | null;
}

export interface UsageAccount {
  /** "claude" or "codex". */
  readonly provider: string;
  /** "system" or a profile id. */
  readonly account: string;
  readonly label: string;
  /** New sessions of this agent use it. */
  readonly inUse: boolean;
  readonly plan: string | null;
  /** Unix seconds of the reading; null when there is none yet. */
  readonly updatedAt: number | null;
  readonly windows: readonly UsageLimitWindow[];
}

export interface UsageDay {
  /** "2026-11-16". */
  readonly date: string;
  readonly agents: Readonly<Record<string, { readonly tokens: number; readonly cost: number }>>;
}

export interface UsageSummary {
  readonly days: number;
  readonly from: string | null;
  readonly to: string | null;
  readonly total: UsageTotals;
  readonly agents: readonly { readonly agent: string; readonly totals: UsageTotals }[];
  readonly models: readonly { readonly agent: string; readonly model: string; readonly totals: UsageTotals }[];
  readonly projects: readonly { readonly name: string; readonly path: string; readonly totals: UsageTotals }[];
  readonly daily: readonly UsageDay[];
  readonly accounts: readonly UsageAccount[];
}

export const USAGE_RANGES = [
  { id: "1", label: "Today", hint: "Today's usage" },
  { id: "7", label: "7 days", hint: "The last 7 days" },
  { id: "30", label: "30 days", hint: "The last 30 days" },
] as const;
export type UsageRange = (typeof USAGE_RANGES)[number]["id"];

export type UsageMetric = "tokens" | "cost";

/** "1.2M", "34.5K", "812". */
export function shortCount(count: number): string {
  if (count >= 1e9) return `${(count / 1e9).toFixed(1)}B`;
  if (count >= 1e6) return `${(count / 1e6).toFixed(1)}M`;
  if (count >= 1e3) return `${(count / 1e3).toFixed(1)}K`;
  return String(Math.round(count));
}

/** "$0.42", "$12.30", "$1,204". */
export function dollars(amount: number): string {
  if (amount >= 1000) return `$${Math.round(amount).toLocaleString("en-US")}`;
  return `$${amount.toFixed(2)}`;
}

export function measure(totals: { tokens: number; cost: number }, metric: UsageMetric): number {
  return metric === "cost" ? totals.cost : totals.tokens;
}

export function formatMeasure(value: number, metric: UsageMetric): string {
  return metric === "cost" ? dollars(value) : shortCount(value);
}

/** "in 2 h 14 min", "in 3 days", "now"; null without a time. */
export function resetsIn(resetsAt: number | null, nowMs: number): string | null {
  if (resetsAt === null) return null;
  const seconds = resetsAt - nowMs / 1000;
  if (seconds <= 60) return "now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `in ${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 ? `in ${hours} h ${minutes % 60} min` : `in ${hours} h`;
  const days = Math.round(hours / 24);
  return `in ${days} day${days === 1 ? "" : "s"}`;
}

/** How full a limit is: calm below 70 %, warning to 90 %, then danger. */
export function limitTone(usedPercent: number): "success" | "warning" | "danger" {
  if (usedPercent >= 90) return "danger";
  if (usedPercent >= 70) return "warning";
  return "success";
}
