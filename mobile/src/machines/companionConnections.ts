export interface CompanionConnection {
  revoke(): Promise<void>;
  close(): void;
}

export interface CompanionConnectionsClock {
  setTimeout(callback: () => void, delayMs: number): unknown;
  clearTimeout(handle: unknown): void;
}

export const COMPANION_SIGN_OUT_TIMEOUT_MS = 2_000;

const systemClock: CompanionConnectionsClock = {
  setTimeout: (callback, delayMs) => setTimeout(callback, delayMs),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/**
 * Sign-out owns revocation. Ordinary unregister/navigation leaves the server's
 * 24-hour lease intact so a later socket can resume it.
 */
export function createCompanionConnections(clock: CompanionConnectionsClock = systemClock) {
  // Count registrations so two consumers sharing one client can unregister
  // independently. A drain still revokes/closes that client only once.
  const accounts = new Map<string, Map<CompanionConnection, number>>();

  function registerCompanionConnection(accountId: string, connection: CompanionConnection): () => void {
    if (!accountId) return () => {};
    let bucket = accounts.get(accountId);
    if (!bucket) {
      bucket = new Map();
      accounts.set(accountId, bucket);
    }
    bucket.set(connection, (bucket.get(connection) ?? 0) + 1);
    const registrationBucket = bucket;
    let registered = true;
    return () => {
      if (!registered) return;
      registered = false;
      // An old cleanup must not remove a client registered after sign-out.
      if (accounts.get(accountId) !== registrationBucket) return;
      const remaining = (registrationBucket.get(connection) ?? 1) - 1;
      if (remaining > 0) registrationBucket.set(connection, remaining);
      else registrationBucket.delete(connection);
      if (registrationBucket.size === 0) accounts.delete(accountId);
    };
  }

  async function endCompanionConnections(accountId: string): Promise<void> {
    const bucket = accounts.get(accountId);
    if (!bucket) return;
    // Remove ownership synchronously, before any user callback can reenter.
    accounts.delete(accountId);
    const connections = [...bucket.keys()];
    let timer: unknown;
    let timerArmed = false;
    try {
      const deadline = new Promise<void>((resolve) => {
        timer = clock.setTimeout(resolve, COMPANION_SIGN_OUT_TIMEOUT_MS);
        timerArmed = true;
      });
      const revocations = Promise.all(connections.map(async (connection) => {
        try { await connection.revoke(); } catch { /* Close even if revocation fails. */ }
      }));
      await Promise.race([revocations, deadline]);
    } catch {
      // A faulty timeout adapter must not strand a signed-out connection.
    } finally {
      if (timerArmed) {
        try { clock.clearTimeout(timer); } catch { /* Still close every client. */ }
      }
      for (const connection of connections) {
        try { connection.close(); } catch { /* One client cannot block the rest. */ }
      }
    }
  }

  return { registerCompanionConnection, endCompanionConnections };
}

const sharedConnections = createCompanionConnections();
export const registerCompanionConnection = sharedConnections.registerCompanionConnection;
export const endCompanionConnections = sharedConnections.endCompanionConnections;
