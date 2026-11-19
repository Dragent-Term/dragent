/**
 * Mounted push boundary. Owns one controller per pairing+gateway generation,
 * registers it for the existing forget drain, and renews on foreground.
 * Once per pairing it asks for notifications by itself (its own sheet, then
 * on "Turn on" the system prompt and registration); after that only Settings' Enable/Disable buttons
 * change it. Cleanup only clears timers/listeners, never unregisters.
 */
import { createContext, useCallback, useContext, useEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from "react";
import { AppState } from "react-native";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { useDevice } from "../pairing/DeviceContext";
import { registerCompanionConnection } from "../machines/companionConnections";
import { PushController, PUSH_RENEWAL_MS, type PushOptSnapshot } from "./pushController";
import type { PushNativeDriver } from "./pushController";
import { loadNativePushDriver } from "./nativePushDriver";
import { ensurePushContext } from "./pushSync";
import { NotificationAsk } from "./NotificationAsk";
import type { PushPlatform } from "./pushApi";

interface PushContextValue extends PushOptSnapshot {
  enable: () => Promise<void>;
  disable: () => Promise<void>;
  retry: () => Promise<void>;
}

const PushContext = createContext<PushContextValue | null>(null);
/** Set once the app has asked for notifications for a pairing. */
const ASKED_KEY = "dragent.push.asked.v1";
const disabledSnapshot: PushOptSnapshot = Object.freeze({ status: "unavailable", reason: "Pair this phone to manage push notifications.", pending: false });
const getDisabledSnapshot = () => disabledSnapshot;
const subscribeDisabled = () => () => {};

const fallbackDriver: PushNativeDriver = {
  available: false,
  unavailableReason: "Push notifications are loading.",
  platform: "apns" as PushPlatform,
  requestPermission: async () => false,
  getDevicePushToken: async () => { throw new Error("Push notifications are loading."); },
};

function usePushDriver(): PushNativeDriver {
  const [driver, setDriver] = useState<PushNativeDriver>(fallbackDriver);
  // Loaded once; the driver itself never requests permission on load.
  useEffect(() => {
    let active = true;
    void loadNativePushDriver().then(({ driver: loaded }) => {
      if (active) setDriver(loaded);
    });
    return () => {
      active = false;
    };
  }, []);
  return driver;
}

export function PushProvider({ children }: { children: ReactNode }) {
  const [asking, setAsking] = useState(false);
  const { status: deviceStatus, device, gateway, getAccessToken } = useDevice();
  const driver = usePushDriver();

  const pairingId = deviceStatus === "paired" ? (device?.userId ?? null) : null;
  const phoneId = deviceStatus === "paired" ? (device?.deviceId ?? null) : null;

  // §8: the extension / messaging service open pushes with account|phone.
  useEffect(() => {
    ensurePushContext(pairingId, phoneId);
  }, [pairingId, phoneId]);

  const controller = useMemo(() => {
    if (!pairingId || !gateway) return null;
    return new PushController({
      prefs: AsyncStorage,
      getAccountId: () => pairingId,
      getGateway: () => gateway,
      getAccessToken,
      driver,
      fetchFn: fetch.bind(globalThis),
      renewalMs: PUSH_RENEWAL_MS,
      isForeground: () => AppState.currentState === "active",
    });
  }, [pairingId, gateway, getAccessToken, driver]);

  const snapshot = useSyncExternalStore(
    controller ? controller.subscribe : subscribeDisabled,
    controller ? controller.getSnapshot : getDisabledSnapshot,
    controller ? controller.getSnapshot : getDisabledSnapshot,
  );
  useEffect(() => () => controller?.close(), [controller]);

  // Restore persisted opt-in (flag only) per generation; no native/network.
  useEffect(() => {
    if (!controller) return;
    let active = true;
    void controller
      .restore()
      .then(() => controller.handleForeground())
      .then(async () => {
        // Once a desktop is paired, ask once for notifications: the system
        // prompt, then this phone's registration. Asked once per pairing,
        // whatever the answer; Settings → Notifications turns it on later.
        if (!active || !pairingId || !driver.available) return;
        if (controller.getSnapshot().status !== "disabled") return;
        const key = `${ASKED_KEY}:${pairingId}`;
        try {
          if (await AsyncStorage.getItem(key)) return;
          await AsyncStorage.setItem(key, "1");
        } catch {
          return;
        }
        // Our own explanation first; the system prompt comes on "Turn on".
        if (active) setAsking(true);
      })
      .catch(() => {});
    return () => {
      active = false;
    };
  }, [controller, pairingId, driver.available]);

  // Forget drain: revoke server lease, then drop local state. Normal
  // unmount cleanup only detaches the registry entry — never revokes.
  useEffect(() => {
    if (!controller || !pairingId) return;
    const release = registerCompanionConnection(pairingId, {
      revoke: () => controller.revoke(),
      close: () => controller.close(),
    });
    return release;
  }, [controller, pairingId]);

  // Foreground renewal (≤15 min cadence inside the controller) + interval.
  useEffect(() => {
    if (!controller) return;
    const subscription = AppState.addEventListener("change", (state) => {
      if (state === "active") void controller.handleForeground();
    });
    const timer = setInterval(() => {
      void controller.handleForeground();
    }, PUSH_RENEWAL_MS);
    return () => {
      subscription.remove();
      clearInterval(timer);
    };
  }, [controller]);

  const enable = useCallback(async () => {
    await controller?.enable();
  }, [controller]);
  const disable = useCallback(async () => {
    await controller?.disable();
  }, [controller]);

  const value = useMemo<PushContextValue>(
    () => ({ ...snapshot, enable, disable, retry: enable }),
    [snapshot, enable, disable],
  );
  return (
    <PushContext.Provider value={value}>
      {children}
      <NotificationAsk
        visible={asking}
        pending={snapshot.pending}
        onAllow={() => {
          void controller?.enable().finally(() => setAsking(false));
        }}
        onLater={() => setAsking(false)}
      />
    </PushContext.Provider>
  );
}

export function usePush(): PushContextValue {
  const value = useContext(PushContext);
  if (!value) throw new Error("usePush requires PushProvider");
  return value;
}
