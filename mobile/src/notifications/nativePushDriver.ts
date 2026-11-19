/**
 * Lazy native push driver. Static `expo-notifications` imports live only
 * inside this loader, so web and Expo Go bundles never evaluate native push
 * code. Permission is requested ONLY by the controller's explicit Enable
 * path — never on load, mount, or foreground.
 */
import { Platform } from "react-native";
import Constants from "expo-constants";
import type { PushNativeDriver } from "./pushController";
import type { PushPlatform } from "./pushApi";

export interface NativePushDriverResult {
  readonly driver: PushNativeDriver;
}

function expoGoOrWeb(): boolean {
  if (Platform.OS === "web") return true;
  return Constants.appOwnership === "expo" || Constants.executionEnvironment === "storeClient";
}

export async function loadNativePushDriver(): Promise<NativePushDriverResult> {
  if (Platform.OS === "web") {
    return {
      driver: {
        available: false,
        unavailableReason: "Push notifications need a native development build; the web preview cannot receive them.",
        platform: "apns",
        requestPermission: async () => false,
        getDevicePushToken: async () => { throw new Error("Push notifications are unavailable in the web preview."); },
      },
    };
  }
  if (expoGoOrWeb()) {
    return {
      driver: {
        available: false,
        unavailableReason: "Push notifications require a development build; Expo Go cannot provide device tokens.",
        platform: Platform.OS === "android" ? "fcm" : "apns",
        requestPermission: async () => false,
        getDevicePushToken: async () => { throw new Error("Push notifications require a development build."); },
      },
    };
  }
  if (Platform.OS !== "ios" && Platform.OS !== "android") {
    return {
      driver: {
        available: false,
        unavailableReason: "Push notifications are available on iOS and Android only.",
        platform: "apns",
        requestPermission: async () => false,
        getDevicePushToken: async () => { throw new Error("Push notifications are unavailable on this device."); },
      },
    };
  }
  let module: typeof import("expo-notifications");
  try {
    module = await import("expo-notifications");
  } catch {
    return {
      driver: {
        available: false,
        unavailableReason: "Push support is missing from this build. Rebuild the native app.",
        platform: (Platform.OS === "android" ? "fcm" : "apns") as PushPlatform,
        requestPermission: async () => false,
        getDevicePushToken: async () => { throw new Error("Push support is missing from this build."); },
      },
    };
  }
  const platform: PushPlatform = Platform.OS === "android" ? "fcm" : "apns";
  return {
    driver: {
      available: true,
      unavailableReason: null,
      platform,
      async requestPermission() {
        if (Platform.OS === "android") {
          try {
            await module.setNotificationChannelAsync("dragent-push", {
              name: "Dragent push",
              importance: module.AndroidImportance.DEFAULT,
            });
          } catch {
            // Channel setup is best-effort; permission still decides.
          }
        }
        let permission = await module.getPermissionsAsync();
        if (!permission.granted) permission = await module.requestPermissionsAsync();
        return permission.granted || permission.ios?.status === module.IosAuthorizationStatus.PROVISIONAL;
      },
      async ensureAndroidChannel() {
        if (Platform.OS !== "android") return;
        await module.setNotificationChannelAsync("dragent-push", {
          name: "Dragent push",
          importance: module.AndroidImportance.DEFAULT,
        });
      },
      async getDevicePushToken() {
        const native = await module.getDevicePushTokenAsync();
        const token = typeof native.data === "string" ? native.data.trim() : "";
        if (!token) throw new Error("The device did not return a push token.");
        return token;
      },
      onTokenRotation(listener) {
        const subscription = module.addPushTokenListener(() => listener());
        return () => subscription.remove();
      },
    },
  };
}
