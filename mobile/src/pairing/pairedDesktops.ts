import AsyncStorage from "@react-native-async-storage/async-storage";

const LABELS_KEY = "dragent.paired-desktop-labels.v1";

/**
 * Local display labels for paired desktops. Rename is phone-local only —
 * there is no server rename endpoint — keyed by pairing gateway. Forget
 * (revoke + credential clear) lives in DeviceContext; labels are dropped
 * with it.
 */
export async function getPairedDesktopLabel(gateway: string): Promise<string | null> {
  try {
    const raw = await AsyncStorage.getItem(LABELS_KEY);
    if (!raw) return null;
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    const label = (value as Record<string, unknown>)[gateway];
    return typeof label === "string" && label ? label : null;
  } catch {
    return null;
  }
}

export function normalizePairedDesktopLabel(raw: string): string {
  const label = raw.trim().replace(/\s+/g, " ");
  if (!label) throw new Error("Name this desktop so you can recognize it.");
  if (label.length > 100) throw new Error("Keep the desktop name to 100 characters or fewer.");
  return label;
}

export async function setPairedDesktopLabel(gateway: string, label: string | null): Promise<void> {
  let labels: Record<string, string> = {};
  try {
    const raw = await AsyncStorage.getItem(LABELS_KEY);
    if (raw) {
      const value: unknown = JSON.parse(raw);
      if (value && typeof value === "object" && !Array.isArray(value)) {
        for (const [key, entry] of Object.entries(value as Record<string, unknown>)) {
          if (typeof entry === "string" && entry) labels[key] = entry;
        }
      }
    }
  } catch {
    labels = {};
  }
  if (label === null) delete labels[gateway];
  else labels[gateway] = label;
  await AsyncStorage.setItem(LABELS_KEY, JSON.stringify(labels));
}
