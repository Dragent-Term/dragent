import { useEffect } from "react";
import { Appearance } from "react-native";
import { router, Stack, usePathname } from "expo-router";
import { peekPendingPairLink } from "../src/pairing/pendingLink";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { ThemedStatusBar } from "../src/components/ThemedStatusBar";
import * as SplashScreen from "expo-splash-screen";
import { SafeAreaProvider } from "react-native-safe-area-context";
import { DeviceProvider, useDevice } from "../src/pairing/DeviceContext";
import { PushProvider, usePush } from "../src/notifications/PushProvider";
import { ThemeProvider, useTheme } from "../src/theme";
import { AppShell } from "../src/navigation/AppShell";
import { InAppBanners } from "../src/notifications/InAppBanner";
import { useAgentCard } from "../src/agentCard/useAgentCard";
import { useVersionCheck } from "../src/appVersion/useVersionNotice";
import { VersionNotices } from "../src/appVersion/VersionNotices";
import { useAwayGoesHome } from "../src/navigation/awayHome";
import { useAppStateEvents, usePushEvents } from "../src/diagnostics/useConnectionEvents";
import { loadConnectionLog } from "../src/diagnostics/connectionLog";
import { WELCOME_SEEN_KEY } from "../src/features/welcome/WelcomeScreen";

void SplashScreen.preventAutoHideAsync().catch(() => {});

function Navigation() {
  const { theme, isReady } = useTheme();
  // The live agent card (Live Activity / ongoing notification).
  useAgentCard();
  // "A new version is available": numbers and links only, never code.
  useVersionCheck();
  // Back after five minutes or more away: start at Home.
  useAwayGoesHome();
  // The connection log: the app's comings and goings, and push results.
  useAppStateEvents();
  const push = usePush();
  usePushEvents(push.status, push.reason ?? null);
  useEffect(() => {
    void loadConnectionLog();
  }, []);
  const { ready, status, device } = useDevice();
  useEffect(() => { if (isReady && ready) void SplashScreen.hideAsync(); }, [isReady, ready]);
  // First launch, nothing paired yet: the welcome guide, once.
  const paired = status === "paired" && device !== null;
  const pathname = usePathname();
  useEffect(() => {
    // Only from Home on a fresh launch: never in the middle of pairing (a
    // scanned or opened pairing link lands on its own screen).
    if (!isReady || !ready || paired || pathname !== "/" || peekPendingPairLink()) return;
    let cancelled = false;
    void AsyncStorage.getItem(WELCOME_SEEN_KEY)
      .then((seen) => {
        if (!cancelled && !seen) router.replace("/welcome");
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [isReady, ready, paired, pathname]);
  // The app's theme, not the phone's setting, decides light or dark: the
  // status bar (white over a dark theme, black over a light one), the
  // keyboard and system sheets all follow it.
  useEffect(() => { Appearance.setColorScheme(theme.mode); }, [theme.mode]);
  if (!isReady || !ready) return null;
  return <>
    <ThemedStatusBar />
    <AppShell>
      <Stack screenOptions={{ headerShown: false, contentStyle: { backgroundColor: theme.colors.background } }} />
    </AppShell>
    {/* A push while the app is open: the app's own banner, over everything. */}
    <InAppBanners />
    <VersionNotices />
  </>;
}

export default function RootLayout() {
  return <SafeAreaProvider>
    <ThemeProvider>
      <DeviceProvider><PushProvider><Navigation /></PushProvider></DeviceProvider>
    </ThemeProvider>
  </SafeAreaProvider>;
}
