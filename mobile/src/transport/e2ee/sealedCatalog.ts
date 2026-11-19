/**
 * The catalog at rest (docs/design/e2ee.md §6.5), phone side. The relay's
 * `catalog` stream carries `{catalog: <sealed blob>, machine: <routing
 * summary>}`; the blob is opened only through the pin store, which checks
 * the pinned desktop key and the persisted high-water mark first. Nothing
 * from a blob that fails is shown: the last good copy stays (§9).
 */
import { utf8Encode } from "./bytes";
import type { CatalogCheck, PhoneSecurity } from "./security";

/** §6.5 blob padding bucket (trailing spaces, as the crate encodes it). */
const PAD_BUCKET = 4096;
const MAX_BLOB = 1024 * PAD_BUCKET;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Rebuild the blob bytes from the relay's JSON object: the §6.5 field order
 * and space padding. The native decoder checks every field; this only
 * restores the wire form (the relay may re-serialise the object).
 */
export function sealedBlobBytes(catalog: unknown): Uint8Array | null {
  if (!isRecord(catalog) || !Array.isArray(catalog.recipients)) return null;
  const { v, desktop, version, nonce, body } = catalog;
  if (v !== 1 || typeof desktop !== "string" || typeof version !== "number" || typeof nonce !== "string" || typeof body !== "string") return null;
  const recipients: { phone: string; wrap: string }[] = [];
  for (const r of catalog.recipients) {
    if (!isRecord(r) || typeof r.phone !== "string" || typeof r.wrap !== "string") return null;
    recipients.push({ phone: r.phone, wrap: r.wrap });
  }
  const json = utf8Encode(JSON.stringify({ v: 1, desktop, version, recipients, nonce, body }));
  if (json.length > MAX_BLOB) return null;
  const padded = new Uint8Array(Math.ceil(json.length / PAD_BUCKET) * PAD_BUCKET).fill(0x20);
  padded.set(json);
  return padded;
}

export type SealedCatalogResult =
  | { readonly kind: "catalog"; readonly json: unknown; readonly version: number; readonly machine: unknown }
  /** The blob didn't check out (§9): show the last good copy if there is one. */
  | { readonly kind: "rejected"; readonly lastGood: unknown | null; readonly machine: unknown; readonly reason: CatalogCheck["status"] }
  | { readonly kind: "unpaired" | "lost" | "unreadable" };

export const CATALOG_REJECTED_MESSAGE = "This desktop's data from the server didn't check out. Showing the last good copy.";

/** Verify and open one `catalog` stream payload for `desktop`. */
export function openSealedCatalogPayload(
  security: PhoneSecurity,
  ids: { account: string; phone: string; desktop: string },
  payload: unknown,
): SealedCatalogResult {
  if (!isRecord(payload)) return { kind: "unreadable" };
  const blob = sealedBlobBytes(payload.catalog);
  if (!blob) return { kind: "unreadable" };
  const check = security.openCatalog(ids, blob);
  switch (check.status) {
    case "accepted":
      return { kind: "catalog", json: check.json, version: check.version, machine: payload.machine };
    case "unchanged":
      return check.json !== null
        ? { kind: "catalog", json: check.json, version: check.version, machine: payload.machine }
        : { kind: "rejected", lastGood: null, machine: payload.machine, reason: "unchanged" };
    case "rollback":
    case "equivocation":
    case "rejected":
      return { kind: "rejected", lastGood: check.lastGood, machine: payload.machine, reason: check.status };
    case "unpaired":
    case "lost":
      return { kind: check.status };
  }
}
