/** Pinned workspaces' stored shape, kept pure for tests (see pins.ts). */

/** Per desktop; the oldest pins go first past this. */
export const MAX_PINS = 50;

/** Desktop id → its pinned workspace keys, oldest pin first. */
export type Pins = Readonly<Record<string, readonly string[]>>;

export function parsePins(raw: string | null): Pins {
  if (!raw) return {};
  try {
    const value = JSON.parse(raw) as unknown;
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const out: Record<string, string[]> = {};
    for (const [device, keys] of Object.entries(value as Record<string, unknown>)) {
      if (!Array.isArray(keys)) continue;
      const clean = keys.filter((key): key is string => typeof key === "string" && key !== "");
      if (clean.length > 0) out[device] = [...new Set(clean)].slice(-MAX_PINS);
    }
    return out;
  } catch {
    return {};
  }
}

/** `pins` with `workspace` pinned (newest last) or unpinned on `device`. */
export function withPin(pins: Pins, device: string, workspace: string, pinned: boolean): Pins {
  const current = (pins[device] ?? []).filter((key) => key !== workspace);
  const next = pinned ? [...current, workspace].slice(-MAX_PINS) : current;
  const out: Record<string, readonly string[]> = { ...pins };
  if (next.length > 0) out[device] = next;
  else delete out[device];
  return out;
}

