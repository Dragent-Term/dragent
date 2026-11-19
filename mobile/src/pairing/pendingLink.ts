/**
 * The pending v2 pairing link, held in memory only (docs/design/e2ee.md §6.2
 * step 3). The deep-link handler (`app/+native-intent.ts`) and the scan
 * screen stash the parsed link here; the confirm screen takes it. Nothing
 * secret (`d`, `k`, and above all `p`) ever enters navigation state, route
 * parameters or history, which the OS may persist.
 *
 * Ownership: `takePendingPairLink` (or `clearPendingPairLink`) transfers the
 * secret to the caller, which must wipe it (`wipeBytes`) when pairing
 * finishes or is abandoned. Setting a new link wipes the previous secret.
 */

import { wipeBytes, type PairLink } from "./pairing";

let pending: PairLink | null = null;

/** Stash a freshly scanned, pasted or deep-linked pairing link. Wipes any previous one. */
export function setPendingPairLink(link: PairLink): void {
  clearPendingPairLink();
  pending = link;
}

/** The pending link without taking ownership (the secret stays stored). */
export function peekPendingPairLink(): PairLink | null {
  return pending;
}

/**
 * Take the pending link. The caller owns `secretP` afterwards and must wipe
 * it when pairing finishes or is abandoned.
 */
export function takePendingPairLink(): PairLink | null {
  const link = pending;
  pending = null;
  return link;
}

/** Abandon the pending link now: the secret is wiped. */
export function clearPendingPairLink(): void {
  if (pending) {
    wipeBytes(pending.secretP);
    pending = null;
  }
}
