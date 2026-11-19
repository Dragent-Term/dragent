import AsyncStorage from "@react-native-async-storage/async-storage";
import { createContext, useCallback, useContext, useEffect, useMemo, useSyncExternalStore, type ReactNode } from "react";
import { Platform } from "react-native";
import { E2EE_WEB_MESSAGE, phoneSecurity } from "../transport/e2ee/native";
import { nativePairingCrypto, runPairing } from "./pairingSession";
import { createNativeSocket } from "../transport/socket";
import { secureStorage } from "../storage/secure";
import { DeviceController, type DeviceSnapshot } from "./controller";
import type { PairingFetchFn, PairingPlatform } from "./api";
import type { PairLink } from "./pairing";

const GATEWAY_POINTER_KEY = "dragent.pairing-gateway.v1";

/**
 * Development override only (mock-server, loopback). Production always uses
 * the gateway the desktop QR names. There is no user-facing connection
 * setting.
 */
export function pairingGatewayOverride(): string {
  const raw = process.env.EXPO_PUBLIC_GATEWAY_URL ?? "";
  return raw.trim();
}

export function resolveDeviceGateway(deviceGateway: string): string {
  return pairingGatewayOverride() || deviceGateway;
}

async function readGatewayPointer(): Promise<string | null> {
  try {
    const raw = await AsyncStorage.getItem(GATEWAY_POINTER_KEY);
    if (typeof raw === "string" && raw.trim()) return raw;
    return null;
  } catch {
    return null;
  }
}

/**
 * Platform class for redeem (§6.2 step 4: ios or android only). Pairing
 * refuses to run on the web platform (§1.3).
 */
function platformName(): PairingPlatform {
  if (Platform.OS === "ios" || Platform.OS === "android") return Platform.OS;
  throw new Error(E2EE_WEB_MESSAGE);
}

interface DeviceContextValue extends DeviceSnapshot {
  /** Gateway for relay + HTTP: env override wins, else the paired gateway. Null until paired. */
  gateway: string | null;
  /** True once restore finished (paired or unpaired). */
  ready: boolean;
  /**
   * Pair with a scanned v2 link (held in memory, never in route params).
   * Redeems without a device name, runs the IKpsk1 handshake to the QR key,
   * pins on `pair-done`, adopts the credentials, and navigates home on true.
   * The link's secret is wiped when pairing finishes or is abandoned.
   */
  pair: (input: { link: PairLink; deviceName: string }) => Promise<void>;
  /** Revoke server-side, drain local channels, clear credentials. */
  forget: () => Promise<void>;
  getAccessToken: () => Promise<string | null>;
}

const DeviceContext = createContext<DeviceContextValue | null>(null);

const fetchFn: PairingFetchFn = (url, init) => fetch(url, {
  ...init,
  cache: "no-store",
  credentials: "omit",
});

export function DeviceProvider({ children }: { children: ReactNode }) {
  const controller = useMemo(
    () => new DeviceController({ fetchFn, storage: secureStorage }),
    [],
  );
  const snapshot = useSyncExternalStore(controller.subscribe, controller.getSnapshot, controller.getSnapshot);

  useEffect(() => {
    let active = true;
    void readGatewayPointer().then((gateway) => {
      if (active) void controller.restore(pairingGatewayOverride() || gateway);
    });
    return () => {
      active = false;
    };
  }, [controller]);
  useEffect(() => () => controller.dispose(), [controller]);

  const pair = useCallback(async (input: { link: PairLink; deviceName: string }) => {
    // Throws on the web platform (§1.3): E2EE runs only in the signed apps.
    const security = phoneSecurity();
    const platform = platformName();
    const gateway = (pairingGatewayOverride() || input.link.gateway).trim();
    if (!gateway) throw new Error("Scan the QR code shown in Settings → Mobile on your desktop.");
    // Pointer first: the code is single-use, so if redeem succeeds the next
    // restore must find this gateway even when credential persistence below
    // still needs a retry.
    await AsyncStorage.setItem(GATEWAY_POINTER_KEY, gateway);
    const paired = await runPairing(
      {
        crypto: nativePairingCrypto(security.native),
        fetchFn,
        createSocket: createNativeSocket,
        security,
      },
      {
        code: input.link.code,
        gateway,
        desktop: input.link.desktop,
        desktopKey: input.link.desktopKey,
        secretP: input.link.secretP,
        deviceName: input.deviceName,
        platform,
      },
    );
    await AsyncStorage.setItem(GATEWAY_POINTER_KEY, paired.gateway);
    await controller.completePairing(
      {
        deviceId: paired.deviceId,
        deviceToken: paired.deviceToken,
        user: { id: paired.userId, displayName: paired.displayName },
        gatewayUrl: paired.gateway,
        expiresAt: paired.expiresAt,
      },
      input.deviceName,
    );
  }, [controller]);

  const getAccessToken = useMemo(
    () => controller.getAccessToken,
    [controller],
  );

  const forget = useCallback(async () => {
    try {
      await AsyncStorage.removeItem(GATEWAY_POINTER_KEY);
    } catch {
      // Local channels still close below; the pointer is best-effort.
    }
    await controller.forget();
  }, [controller]);

  const value = useMemo<DeviceContextValue>(() => ({
    ...snapshot,
    ready: snapshot.status !== "checking",
    gateway: snapshot.device ? resolveDeviceGateway(snapshot.device.gateway) : null,
    pair,
    forget,
    getAccessToken,
  }), [snapshot, pair, forget, getAccessToken]);

  return <DeviceContext value={value}>{children}</DeviceContext>;
}

export function useDevice(): DeviceContextValue {
  const value = useContext(DeviceContext);
  if (!value) throw new Error("useDevice requires DeviceProvider");
  return value;
}
