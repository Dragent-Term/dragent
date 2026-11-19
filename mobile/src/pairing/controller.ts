import { PairingError, refreshDeviceToken, type PairingFetchFn, type RedeemResult } from "./api";
import { deviceCredentialStore, type DeviceCredential } from "./deviceStore";
import type { KeyValueStore } from "../storage/types";
import { serializeCredentials } from "../storage/credentials";

export interface PairedDevice {
  readonly deviceId: string;
  readonly deviceName: string;
  readonly gateway: string;
  readonly userId: string;
  readonly displayName: string;
}

export type DeviceStatus = "checking" | "unpaired" | "paired";

export interface DeviceSnapshot {
  readonly status: DeviceStatus;
  readonly device: PairedDevice | null;
  readonly error: string | null;
}

export interface DeviceControllerDeps {
  fetchFn: PairingFetchFn;
  storage: KeyValueStore;
  now?: () => number;
}

const SAVE_ERROR = "Pairing is active but could not be saved on this device. Reopen the app to retry saving it.";
const REVOKED_MESSAGE = "This phone was unpaired from your desktop. Pair again to reconnect.";
const REFRESH_MARGIN_MS = 60_000;

/**
 * This phone's pairing state machine. Replaces account sign-in for the
 * companion: one active pairing, serialized rotating token refresh
 * (single-flight), revoked/expired credentials fall back to unpaired.
 * The access token feeds the existing relay socket/session code wherever it
 * used the portal account token.
 */
export class DeviceController {
  private snapshot: DeviceSnapshot = { status: "checking", device: null, error: null };
  private readonly listeners = new Set<() => void>();
  private credential: DeviceCredential | null = null;
  private accessToken: string | null = null;
  private accessExpiresAt = 0;
  private epoch = 0;
  private refreshing: Promise<string | null> | null = null;
  private needsSave = false;

  constructor(private readonly deps: DeviceControllerDeps) {}

  getSnapshot = (): DeviceSnapshot => this.snapshot;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  private publish(value: DeviceSnapshot) {
    this.snapshot = value;
    this.listeners.forEach((listener) => listener());
  }
  private now(): number {
    return this.deps.now ? this.deps.now() : Date.now();
  }
  private advanceEpoch(): number {
    this.refreshing = null;
    return ++this.epoch;
  }

  private describe(credential: DeviceCredential): PairedDevice {
    return Object.freeze({
      deviceId: credential.deviceId,
      deviceName: credential.deviceName,
      gateway: credential.gateway,
      userId: credential.userId,
      displayName: credential.displayName,
    });
  }

  private async persist(credential: DeviceCredential, epoch: number): Promise<boolean> {
    if (epoch !== this.epoch) return false;
    // Rotation has consumed the old token. Replace it before any fallible
    // save so a storage failure cannot cause the old token to be sent again.
    this.credential = credential;
    this.needsSave = true;
    let error: string | null = null;
    try {
      await deviceCredentialStore(this.deps.storage, credential.gateway).save(credential);
    } catch {
      error = SAVE_ERROR;
    }
    if (epoch !== this.epoch) return false;
    this.needsSave = error !== null;
    this.publish({ status: "paired", device: this.describe(credential), error });
    return true;
  }

  /** Reject the pairing: clear secrets, stop refresh, land on unpaired. */
  private async rejectPairing(message: string, expectedEpoch: number) {
    if (expectedEpoch !== this.epoch) return;
    const forget = this.forget();
    const forgottenEpoch = this.epoch;
    await forget;
    if (forgottenEpoch === this.epoch) this.publish({ status: "unpaired", device: null, error: message });
  }

  async restore(gateway: string | null): Promise<void> {
    const epoch = this.advanceEpoch();
    try {
      if (!gateway) {
        this.publish({ status: "unpaired", device: null, error: null });
        return;
      }
      const saved = await serializeCredentials(this.deps.storage, async () => {
        if (epoch !== this.epoch) return null;
        return deviceCredentialStore(this.deps.storage, gateway).load();
      });
      if (epoch !== this.epoch) return;
      if (saved) {
        this.credential = saved;
        this.publish({ status: "paired", device: this.describe(saved), error: null });
        return;
      }
      this.publish({ status: "unpaired", device: null, error: null });
    } catch {
      if (epoch !== this.epoch) return;
      this.publish({ status: "unpaired", device: null, error: "Saved pairing could not be restored. Pair again to reconnect." });
    }
  }

  /** Adopt the credentials from a successful redeem. */
  async completePairing(redeemed: RedeemResult, deviceName: string): Promise<boolean> {
    const epoch = this.advanceEpoch();
    const credential: DeviceCredential = Object.freeze({
      gateway: redeemed.gatewayUrl,
      deviceId: redeemed.deviceId,
      deviceToken: redeemed.deviceToken,
      deviceName,
      userId: redeemed.user.id,
      displayName: redeemed.user.displayName,
      pairedAt: this.now(),
    });
    this.accessToken = null;
    this.accessExpiresAt = 0;
    return this.persist(credential, epoch);
  }

  /**
   * Resolve a fresh access token for the relay socket. Single-flight across
   * concurrent callers; rotates the device token on every refresh and
   * persists the rotation before returning. A 401 means the desktop revoked
   * (or a rotated-out token was reused): the pairing is rejected and every
   * waiter resolves null so sockets surface the pair screen, never a retry
   * storm.
   */
  getAccessToken = async (): Promise<string | null> => {
    if (!this.credential) return null;
    if (!this.needsSave && this.accessToken && this.accessExpiresAt - this.now() > REFRESH_MARGIN_MS) {
      return this.accessToken;
    }
    if (!this.refreshing) {
      const epoch = this.epoch;
      const current = this.credential;
      const promise = refreshDeviceToken(this.deps.fetchFn, current.gateway, {
        deviceId: current.deviceId,
        deviceToken: current.deviceToken,
      }).then(async (result) => {
        if (epoch !== this.epoch || this.credential !== current) {
          throw new Error("Pairing changed while refreshing.");
        }
        this.accessToken = result.accessToken;
        this.accessExpiresAt = result.accessExpiresAt;
        const rotated: DeviceCredential = Object.freeze({ ...current, deviceToken: result.deviceToken });
        if (!(await this.persist(rotated, epoch))) throw new Error("Pairing changed while refreshing.");
        return result.accessToken;
      }).catch(async (error: unknown) => {
        if (epoch === this.epoch && error instanceof PairingError && error.kind === "unauthorized") {
          await this.rejectPairing(REVOKED_MESSAGE, epoch);
          return null;
        }
        throw error;
      });
      this.refreshing = promise;
      void promise.finally(() => {
        if (this.refreshing === promise) this.refreshing = null;
      }).catch(() => {});
    }
    return this.refreshing;
  };

  /** Clear local credentials (the caller revokes server-side first). */
  async forget(): Promise<void> {
    const epoch = this.advanceEpoch();
    const gateway = this.credential?.gateway;
    this.credential = null;
    this.accessToken = null;
    this.accessExpiresAt = 0;
    this.needsSave = false;
    this.publish({ status: "unpaired", device: null, error: null });
    try {
      if (gateway) {
        await serializeCredentials(this.deps.storage, () =>
          deviceCredentialStore(this.deps.storage, gateway).clear());
      }
    } catch {
      if (epoch === this.epoch) {
        this.publish({ status: "unpaired", device: null, error: "Unable to remove the saved pairing from this device. Try again." });
      }
    }
  }

  dispose() {
    this.advanceEpoch();
    this.credential = null;
    this.accessToken = null;
    this.accessExpiresAt = 0;
    this.needsSave = false;
    this.listeners.clear();
  }
}
