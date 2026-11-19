/** Strict readers for the desktop's `usage.*` replies: malformed is `null`. */
import type { UsageAccount, UsageDay, UsageLimitWindow, UsageSummary, UsageTotals } from "./types";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function count(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

function text(value: unknown, max: number): string | null {
  return typeof value === "string" && value.length <= max ? value : null;
}

function list<T>(value: unknown, max: number, read: (entry: unknown) => T | null): T[] | null {
  if (!Array.isArray(value) || value.length > max) return null;
  const out: T[] = [];
  for (const entry of value) {
    const parsed = read(entry);
    if (parsed === null) return null;
    out.push(parsed);
  }
  return out;
}

export function readTotals(value: unknown): UsageTotals | null {
  if (!isRecord(value)) return null;
  const fields = {
    tokens: count(value.tokens),
    input: count(value.input),
    output: count(value.output),
    cacheRead: count(value.cache_read),
    cacheWrite: count(value.cache_write),
    replies: count(value.replies),
    cost: count(value.cost),
    unpricedTokens: count(value.unpriced_tokens) ?? 0,
    cacheSavings: count(value.cache_savings) ?? 0,
  };
  if (Object.values(fields).some((field) => field === null)) return null;
  return Object.freeze(fields as UsageTotals);
}

function readWindow(value: unknown): UsageLimitWindow | null {
  if (!isRecord(value)) return null;
  const label = text(value.label, 64);
  const used = count(value.used_percent);
  if (!label || used === null) return null;
  const resetsAt = typeof value.resets_at === "number" && Number.isFinite(value.resets_at) ? value.resets_at : null;
  return Object.freeze({ label, usedPercent: Math.min(100, used), resetsAt });
}

export function readAccount(value: unknown): UsageAccount | null {
  if (!isRecord(value)) return null;
  const provider = text(value.provider, 32);
  const account = text(value.account, 64);
  const label = text(value.label, 200);
  const windows = value.windows === undefined ? [] : list(value.windows, 20, readWindow);
  if (!provider || !account || label === null || windows === null) return null;
  return Object.freeze({
    provider,
    account,
    label,
    inUse: value.default === true,
    plan: typeof value.plan === "string" && value.plan.length <= 64 ? value.plan : null,
    updatedAt: typeof value.updated_at === "number" && Number.isFinite(value.updated_at) ? value.updated_at : null,
    windows: Object.freeze(windows),
  });
}

function readDay(value: unknown): UsageDay | null {
  if (!isRecord(value) || !isRecord(value.agents)) return null;
  const date = text(value.date, 10);
  if (!date || !/^\d{4}-\d{2}-\d{2}$/.test(date)) return null;
  const agents: Record<string, { tokens: number; cost: number }> = {};
  for (const [agent, totals] of Object.entries(value.agents).slice(0, 50)) {
    if (!isRecord(totals)) return null;
    const tokens = count(totals.tokens);
    const cost = count(totals.cost);
    if (tokens === null || cost === null || agent.length > 64) return null;
    agents[agent] = Object.freeze({ tokens, cost });
  }
  return Object.freeze({ date, agents: Object.freeze(agents) });
}

export function readAccounts(value: unknown): readonly UsageAccount[] | null {
  const accounts = list(value, 100, readAccount);
  return accounts === null ? null : Object.freeze(accounts);
}

export function readUsageSummary(value: unknown): UsageSummary | null {
  if (!isRecord(value) || !isRecord(value.range)) return null;
  const total = readTotals(value.total);
  const agents = list(value.agents, 100, (entry) => {
    if (!isRecord(entry)) return null;
    const agent = text(entry.agent, 64);
    const totals = readTotals(entry.totals);
    return agent && totals ? Object.freeze({ agent, totals }) : null;
  });
  const models = list(value.models, 100, (entry) => {
    if (!isRecord(entry)) return null;
    const agent = text(entry.agent, 64);
    const model = text(entry.model, 200);
    const totals = readTotals(entry.totals);
    return agent && model !== null && totals ? Object.freeze({ agent, model, totals }) : null;
  });
  const projects = list(value.projects, 100, (entry) => {
    if (!isRecord(entry)) return null;
    const name = text(entry.name, 512);
    const path = text(entry.path, 4096);
    const totals = readTotals(entry.totals);
    return name !== null && path !== null && totals ? Object.freeze({ name, path, totals }) : null;
  });
  const daily = list(value.days, 400, readDay);
  const accounts = readAccounts(value.accounts ?? []);
  const days = count(value.range.days);
  if (!total || !agents || !models || !projects || !daily || !accounts || days === null) return null;
  return Object.freeze({
    days,
    from: text(value.range.from, 10),
    to: text(value.range.to, 10),
    total,
    agents: Object.freeze(agents),
    models: Object.freeze(models),
    projects: Object.freeze(projects),
    daily: Object.freeze(daily),
    accounts,
  });
}

export function readAccountsReply(value: unknown): readonly UsageAccount[] | null {
  return isRecord(value) ? readAccounts(value.accounts) : null;
}
