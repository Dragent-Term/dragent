/**
 * Pure owner identity for live coordination polling.
 *
 * Mirrors the companion terminal owner: when account, device, rpc client,
 * socket epoch, retry or scope (project/team filter) changes, the new owner
 * renders a fresh blank state immediately. Same-render masking needs no
 * effects, so unit tests exercise the exact masking the hooks render with.
 */
export interface CoordinationOwnerFields {
  readonly accountId: string | null;
  readonly deviceId: string;
  readonly epoch: number;
  readonly retry: number;
  readonly clientToken: string;
  readonly projectId: string;
  readonly teamId: string;
}

export interface CoordinationEligibility {
  readonly foreground: boolean;
  readonly socketReady: boolean;
  readonly sessionActive: boolean;
  readonly online: boolean;
  readonly rpcAvailable: boolean;
  readonly deviceValid: boolean;
}

const clientIds = new WeakMap<object, number>();
let clientSequence = 0;

/** Bounded live cadence: 5s or slower, sequential, foreground-gated. */
export const COORDINATION_POLL_MS = 5000;

/** Stable token per rpc client identity; null maps to "none". Deterministic. */
export function coordinationClientTokenFor(client: unknown): string {
  if (typeof client !== "object" || client === null) return "none";
  const existing = clientIds.get(client);
  if (existing !== undefined) return `client:${existing}`;
  clientSequence += 1;
  clientIds.set(client, clientSequence);
  return `client:${clientSequence}`;
}

/**
 * Full owner key: a JSON tuple, so no field-boundary collisions are possible
 * (`["ab", "c"]` and `["a", "bc"]` key differently). Any field change yields a
 * different key. Numbers stay numbers, so epoch `1` never equals `"1"`.
 */
export function coordinationOwnerKey(fields: CoordinationOwnerFields): string {
  return JSON.stringify([
    fields.accountId ?? "",
    fields.deviceId,
    Number.isSafeInteger(fields.epoch) ? fields.epoch : "invalid",
    Number.isSafeInteger(fields.retry) && fields.retry >= 0 ? fields.retry : "invalid",
    fields.clientToken,
    fields.projectId,
    fields.teamId,
  ]);
}

/** Poll gate: every condition must hold; no partial credit. */
export function isCoordinationEligible(flags: CoordinationEligibility): boolean {
  return (
    flags.foreground &&
    flags.socketReady &&
    flags.sessionActive &&
    flags.online &&
    flags.rpcAvailable &&
    flags.deviceValid
  );
}

/**
 * Keyed render selector: expose the owned value only while the record key
 * still matches the current owner key; otherwise fall back. Masks stale
 * output synchronously during render.
 */
export function selectOwnedCoordination<T>(recordKey: string, ownerKey: string, value: T, fallback: T): T {
  return recordKey === ownerKey ? value : fallback;
}
