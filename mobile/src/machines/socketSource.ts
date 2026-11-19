import type { RelaySocketSource } from "../transport/useRelaySocket";

export interface MachineSocketSourceInput {
  readonly gateway: string | null;
  readonly deviceId: string | null | undefined;
  /** Paired user id, or null when unpaired. */
  readonly accountId: string | null | undefined;
  /**
   * This phone's own paired device id. Required (may be null) so a desktop
   * socket can't be built without it: E2EE binds every channel and sealed
   * catalog to (account, phone, desktop), and without it the socket has no
   * security and nothing from the desktop can be opened.
   */
  readonly phoneDeviceId: string | null | undefined;
  readonly signedIn: boolean;
  /** Controller method; may stay stable across pairings. */
  readonly getAccessToken: () => Promise<string | null>;
  /** §4.6 step 4: typed input on these terminal handles didn't reach the desktop. */
  readonly onInputLost?: ((handles: readonly number[], message: string) => void) | undefined;
  readonly requireDevice?: boolean | undefined;
}

/**
 * Build the socket source for one pairing generation. The paired user id
 * travels in the source (forcing a new client per pairing in
 * useRelaySocket's owner identity) AND in a fresh resolver closure per
 * pairing: the controller's getAccessToken can remain stable across
 * pairings and paired can persist across batched updates, so neither alone
 * identifies the pairing.
 * Callers memoize this per [gateway, deviceId, accountId, signedIn,
 * getAccessToken] so the source (and client) is stable within a pairing.
 */
export function createMachineSocketSource(
  input: MachineSocketSourceInput,
): RelaySocketSource {
  const accountId =
    input.signedIn && typeof input.accountId === "string" && input.accountId.trim() !== ""
      ? input.accountId
      : null;
  const resolve = input.getAccessToken;
  return {
    gateway: input.gateway,
    deviceId: input.deviceId,
    ...(input.requireDevice === undefined ? {} : { requireDevice: input.requireDevice }),
    accountId,
    phoneDeviceId: accountId ? input.phoneDeviceId : null,
    // Fresh closure per account generation: even with a stable controller
    // method underneath, the identity change forces a new owner/client.
    resolveAccessToken: accountId ? () => resolve() : null,
    // Read when the client is built (never part of the owner identity): a
    // new callback identity must not tear down a live connection.
    ...(input.onInputLost ? { onInputLost: input.onInputLost } : {}),
  };
}
