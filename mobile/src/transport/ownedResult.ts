/**
 * Platform-independent owned-result masking. No React, no native imports:
 * pure domain/store tests can exercise the exact selector the hooks render
 * with, without loading the socket lifecycle.
 */

/**
 * A result owned by one socket client generation. Keyed by BOTH client
 * identity and connection epoch: two different clients (accounts, devices)
 * can both be at epoch 1, so epoch alone leaks the old generation into the
 * new render before passive effect cleanup runs.
 */
export interface OwnedResult<TClient, TState> {
  readonly client: TClient;
  readonly epoch: number;
  readonly state: TState;
}

/** The owned state, or null when masked (stale owner/generation). */
export function selectOwnedState<TClient, TState>(
  result: OwnedResult<TClient, TState> | null,
  client: TClient | null,
  epoch: number,
): TState | null {
  if (!client || !result || result.client !== client || result.epoch !== epoch) return null;
  return result.state;
}
