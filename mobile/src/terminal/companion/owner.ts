/**
 * Pure owner identity for the per-tab companion terminal.
 *
 * Render masking must not wait for passive effects: when account, device,
 * tab, rpc client, socket epoch or manual retry changes, the new owner must
 * render a fresh blank runtime immediately. These helpers derive a stable
 * string key from the full owner identity plus an eligibility gate and a
 * keyed selector, all without React, so unit tests can exercise the exact
 * masking the hook renders with.
 */

export interface CompanionOwnerFields {
  readonly accountId: string | null;
  readonly deviceId: string;
  readonly tabId: number;
  readonly epoch: number;
  readonly retry: number;
  readonly clientToken: string;
}

export interface CompanionEligibility {
  readonly foreground: boolean;
  readonly socketReady: boolean;
  readonly leaseActive: boolean;
  readonly online: boolean;
  readonly rpcAvailable: boolean;
  readonly tabValid: boolean;
}

const clientIds = new WeakMap<object, number>();
let clientSequence = 0;

/** Stable token per rpc client identity; null maps to "none". Deterministic. */
export function clientTokenFor(client: unknown): string {
  if (typeof client !== "object" || client === null) return "none";
  const existing = clientIds.get(client);
  if (existing !== undefined) return `client:${existing}`;
  clientSequence += 1;
  clientIds.set(client, clientSequence);
  return `client:${clientSequence}`;
}

/** Full owner key: any field change yields a different key. No separators collide. */
export function companionOwnerKey(fields: CompanionOwnerFields): string {
  return [
    fields.accountId ?? "",
    fields.deviceId,
    Number.isSafeInteger(fields.tabId) ? String(fields.tabId) : "invalid",
    Number.isSafeInteger(fields.epoch) ? String(fields.epoch) : "invalid",
    Number.isSafeInteger(fields.retry) && fields.retry >= 0 ? String(fields.retry) : "invalid",
    fields.clientToken,
  ].join("");
}

/** Poll/attach/write gate: every condition must hold; no partial credit. */
export function isCompanionEligible(flags: CompanionEligibility): boolean {
  return (
    flags.foreground &&
    flags.socketReady &&
    flags.leaseActive &&
    flags.online &&
    flags.rpcAvailable &&
    flags.tabValid
  );
}

/**
 * Keyed render selector: expose the owned value only while the record key
 * still matches the current owner key; otherwise fall back (fresh blank).
 * Masks stale output/status synchronously during render.
 */
export function selectOwnedCompanion<T>(recordKey: string, ownerKey: string, value: T, fallback: T): T {
  return recordKey === ownerKey ? value : fallback;
}
