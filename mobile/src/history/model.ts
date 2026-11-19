/** Past sessions on the phone: what the desktop sends, and how it's grouped and labelled. */

export interface PastSession {
  /** The agent's command: claude, codex, gemini, opencode, mimo, copilot, cursor-agent. */
  readonly provider: string;
  readonly id: string;
  readonly title: string;
  /** Unix seconds of its last change. */
  readonly updatedAt: number;
  /** The account it belongs to, when not the system login. */
  readonly accountLabel: string | null;
  /** Bytes on disk; null when not measured. */
  readonly size: number | null;
}

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

export function readPastSessions(value: unknown): readonly PastSession[] | null {
  if (!isRecord(value) || !Array.isArray(value.sessions)) return null;
  const out: PastSession[] = [];
  for (const entry of value.sessions.slice(0, 200)) {
    if (!isRecord(entry)) continue;
    const { provider, id, title } = entry;
    if (typeof provider !== "string" || typeof id !== "string" || typeof title !== "string" || !provider || !id) continue;
    out.push(
      Object.freeze({
        provider: provider.slice(0, 32),
        id: id.slice(0, 128),
        title: title.slice(0, 200) || "Untitled",
        updatedAt: typeof entry.updated_at === "number" && Number.isFinite(entry.updated_at) ? entry.updated_at : 0,
        accountLabel: typeof entry.account_label === "string" && entry.account_label ? entry.account_label.slice(0, 64) : null,
        size: typeof entry.size === "number" && entry.size >= 0 ? entry.size : null,
      }),
    );
  }
  return Object.freeze(out);
}

/** The caption a conversation sits under, by the day it last changed. */
export function periodOf(updatedAt: number, now: Date): string {
  const day = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const days = Math.round((day(now) - day(new Date(updatedAt * 1000))) / 86_400_000);
  if (days <= 0) return "TODAY";
  if (days === 1) return "YESTERDAY";
  if (days <= 6) return "THIS WEEK";
  if (days <= 30) return "THIS MONTH";
  return "EARLIER";
}

/** "just now", "5 minutes ago", "3 hours ago", "2 days ago", "4 months ago". */
export function ageLabel(updatedAt: number, nowMs: number): string {
  const seconds = Math.max(0, Math.floor(nowMs / 1000 - updatedAt));
  const plural = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"} ago`;
  if (seconds < 60) return "just now";
  if (seconds < 3600) return plural(Math.floor(seconds / 60), "minute");
  if (seconds < 86_400) return plural(Math.floor(seconds / 3600), "hour");
  if (seconds < 2_592_000) return plural(Math.floor(seconds / 86_400), "day");
  return plural(Math.floor(seconds / 2_592_000), "month");
}

/** "2.3 GB", "840 MB", "12 KB". */
export function bytesLabel(bytes: number): string {
  const kb = 1024;
  if (bytes >= kb ** 3) return `${(bytes / kb ** 3).toFixed(1)} GB`;
  if (bytes >= kb ** 2) return `${Math.round(bytes / kb ** 2)} MB`;
  return `${Math.max(1, Math.round(bytes / kb))} KB`;
}

/** Every typed word appears in the title, the agent's name or the account. */
export function matchesSearch(session: PastSession, agentName: string, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const haystack = `${session.title} ${agentName} ${session.accountLabel ?? ""}`.toLowerCase();
  return words.every((word) => haystack.includes(word));
}

/** Sessions under their captions, newest first, as the desktop lists them. */
export function groupByPeriod(sessions: readonly PastSession[], now: Date): { caption: string; sessions: PastSession[] }[] {
  const groups: { caption: string; sessions: PastSession[] }[] = [];
  for (const session of [...sessions].sort((a, b) => b.updatedAt - a.updatedAt)) {
    const caption = periodOf(session.updatedAt, now);
    const last = groups[groups.length - 1];
    if (last && last.caption === caption) last.sessions.push(session);
    else groups.push({ caption, sessions: [session] });
  }
  return groups;
}
