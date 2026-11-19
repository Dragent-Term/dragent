/**
 * Settings → Remove desktop (docs/design/e2ee.md §6.4 "A phone unpairs a
 * desktop", §12.6). Local, total and immediate first; the server last, for
 * availability only.
 *
 * Dependencies are injected so node tests can assert the order (every
 * E2EE desktop is unpaired before anything touches the server, and local
 * state always clears even when the network fails) without React Native.
 */
export interface RemoveDesktopDeps {
  /** Desktop ids the phone's pin store knows (E5a's pairedDesktops). */
  readonly pairedDesktops: () => readonly string[];
  /**
   * E5a's atomic phone-side unpair: pin + tombstone, channels closed,
   * push pin removed, cached catalog dropped, keys deleted when last.
   */
  readonly unpairDesktop: (
    desktop: string,
    options?: { deleteOnServer?: (() => Promise<void>) | undefined },
  ) => Promise<unknown>;
  /** DELETE this phone's device record (availability only). */
  readonly revokePhoneDevice: () => Promise<void>;
  readonly endCompanionConnections: () => Promise<void>;
  readonly clearLabel: () => Promise<void>;
  readonly forgetPairing: () => Promise<void>;
}

export async function removeDesktop(deps: RemoveDesktopDeps): Promise<void> {
  const desktops = (() => {
    try {
      return deps.pairedDesktops();
    } catch {
      // No E2EE store on this build (web, missing module): nothing to unpair.
      return [];
    }
  })();
  // §6.4 steps 1–5 per desktop, local first. One desktop's failure never
  // blocks the next: local state still clears below regardless.
  for (const desktop of desktops) {
    try {
      await deps.unpairDesktop(desktop);
    } catch {
      /* Local state still clears below. */
    }
  }
  // §6.4 step 6: then the server. Availability only — errors are swallowed.
  try {
    await deps.revokePhoneDevice();
  } catch {
    /* Local pairing still clears below. */
  }
  try {
    await deps.endCompanionConnections();
  } catch {
    /* Local pairing still clears below. */
  }
  try {
    await deps.clearLabel();
  } catch {
    /* Best effort; the label store is phone-local. */
  }
  await deps.forgetPairing();
}
