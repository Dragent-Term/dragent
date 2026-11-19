/**
 * The phone's own list of the alerts it received, newest first. Kept only
 * on this phone (the alerts were decrypted here), at most `MAX` of them.
 */

export interface NotificationEntry {
  /** The system's id for it when known, else one made from its content and time. */
  readonly id: string;
  readonly title: string;
  readonly body: string;
  /** The paired desktop it's about. */
  readonly desktop: string | null;
  /** Unix milliseconds it arrived. */
  readonly at: number;
  readonly read: boolean;
}

export const MAX_ENTRIES = 100;
/** The same alert seen twice this close together (banner, then tap) is one. */
const SAME_ALERT_MS = 2 * 60 * 1000;

const same = (a: NotificationEntry, b: Omit<NotificationEntry, "read">) =>
  a.id === b.id || (a.title === b.title && a.body === b.body && a.desktop === b.desktop && Math.abs(a.at - b.at) < SAME_ALERT_MS);

/** Add an alert (or mark one already listed as read when `read`), newest first, capped. */
export function addEntry(list: readonly NotificationEntry[], entry: Omit<NotificationEntry, "read">, read = false): NotificationEntry[] {
  const existing = list.find((item) => same(item, entry));
  if (existing) {
    return read && !existing.read ? list.map((item) => (item === existing ? { ...item, read: true } : item)) : [...list];
  }
  return [{ ...entry, read }, ...list].sort((a, b) => b.at - a.at).slice(0, MAX_ENTRIES);
}

export function unreadCount(list: readonly NotificationEntry[]): number {
  return list.filter((item) => !item.read).length;
}

/** What was stored, kept only if it reads back as entries. */
export function readEntries(raw: string | null): NotificationEntry[] {
  if (!raw) return [];
  try {
    const value: unknown = JSON.parse(raw);
    if (!Array.isArray(value)) return [];
    return value
      .filter((item): item is NotificationEntry =>
        typeof item === "object" && item !== null &&
        typeof item.id === "string" && typeof item.title === "string" && typeof item.body === "string" &&
        (item.desktop === null || typeof item.desktop === "string") && typeof item.at === "number" && typeof item.read === "boolean")
      .slice(0, MAX_ENTRIES);
  } catch {
    return [];
  }
}
