/**
 * The connection log: what happened to the phone's links to its desktops,
 * in plain words, for when something won't connect. Never messages, code,
 * keys or anything a desktop sent — only states, timings and error words.
 */

export type LogLevel = "info" | "warn" | "error";

export interface LogEntry {
  /** Unix milliseconds (of the first time, when repeated). */
  readonly at: number;
  /** The last time it happened (repeats fold into one line). */
  readonly last: number;
  readonly count: number;
  readonly level: LogLevel;
  /** The paired desktop's id, or null for the app as a whole. */
  readonly desktop: string | null;
  readonly text: string;
}

export const MAX_LOG = 300;
const MAX_TEXT = 300;

/** Add an event, newest first; the same event straight after itself is counted, not repeated. */
export function addLog(list: readonly LogEntry[], event: { level: LogLevel; desktop: string | null; text: string; at: number }): LogEntry[] {
  const text = event.text.replace(/\s+/g, " ").trim().slice(0, MAX_TEXT);
  if (!text) return [...list];
  const latest = list[0];
  if (latest && latest.text === text && latest.desktop === event.desktop && latest.level === event.level) {
    return [{ ...latest, last: event.at, count: latest.count + 1 }, ...list.slice(1)];
  }
  return [{ at: event.at, last: event.at, count: 1, level: event.level, desktop: event.desktop, text }, ...list].slice(0, MAX_LOG);
}

export function readLog(raw: string | null): LogEntry[] {
  if (!raw) return [];
  try {
    const value: unknown = JSON.parse(raw);
    if (!Array.isArray(value)) return [];
    return value
      .filter((item): item is LogEntry =>
        typeof item === "object" && item !== null && typeof item.at === "number" && typeof item.last === "number" &&
        typeof item.count === "number" && ["info", "warn", "error"].includes(item.level) &&
        (item.desktop === null || typeof item.desktop === "string") && typeof item.text === "string")
      .slice(0, MAX_LOG);
  } catch {
    return [];
  }
}

const clock = (at: number) => {
  const date = new Date(at);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())} ${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}`;
};

/** The log as plain text to copy into a bug report, oldest first. */
export function formatLog(list: readonly LogEntry[], nameOf: (desktop: string) => string): string {
  return [...list]
    .reverse()
    .map((entry) => {
      const who = entry.desktop ? nameOf(entry.desktop) : "App";
      const times = entry.count > 1 ? ` (×${entry.count}, last ${clock(entry.last)})` : "";
      return `${clock(entry.at)} ${entry.level.toUpperCase().padEnd(5)} ${who}: ${entry.text}${times}`;
    })
    .join("\n");
}
