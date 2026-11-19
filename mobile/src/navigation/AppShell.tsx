import { useState, type ReactNode } from "react";
import { Image, StyleSheet, View } from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";
import { router, usePathname } from "expo-router";
import { AppText, Control, Icon, IconButton } from "../components";
import { LayoutViewportProvider, useResponsiveLayout } from "../layout";
import { useTheme } from "../theme";
import { useDevice } from "../pairing/DeviceContext";
import { useNotificationHistory } from "../notifications/history";

const destinations = [
  { href: "/", label: "Home", icon: "grid" },
  { href: "/settings", label: "Settings", icon: "settings" },
] as const;

function sectionFor(path: string): string {
  if (path === "/settings") return "/settings";
  return "/";
}

/** The Dragent mark and name: tapping them goes Home from anywhere. */
function Brand() {
  const path = usePathname();
  const goHome = () => {
    if (path === "/") return;
    if (router.canDismiss()) router.dismissTo("/");
    else router.replace("/");
  };
  return <Control accessibilityRole="link" accessibilityLabel="Dragent, go to Home" onPress={goHome}
    hitSlop={8} style={({ pressed }) => [styles.brand, pressed ? styles.brandPressed : null]}>
    <Image source={require("../../assets/favicon.png")} style={styles.mark} accessibilityIgnoresInvertColors />
    <AppText style={styles.wordmark}>Dragent</AppText>
  </Control>;
}

/** The bell: the alerts this phone received, with how many are unread. */
function AlertsButton() {
  const { theme } = useTheme();
  const { unread } = useNotificationHistory();
  return <View>
    <IconButton name="bell" label={unread > 0 ? `Notifications, ${unread} unread` : "Notifications"} onPress={() => router.navigate("/notifications")} />
    {unread > 0 ? <View pointerEvents="none" style={[styles.badge, { backgroundColor: theme.colors.accent }]}>
      <AppText style={[styles.badgeText, { color: theme.colors.background }]}>{unread > 9 ? "9+" : String(unread)}</AppText>
    </View> : null}
  </View>;
}

function NavigationItem({ item, selected }: {
  item: (typeof destinations)[number]; selected: boolean;
}) {
  const { theme } = useTheme();
  return <Control accessibilityRole="tab" accessibilityState={{ selected }}
    onPress={() => router.navigate(item.href)}
    style={({ pressed, hovered }) => [styles.sidebarLink,
      { backgroundColor: pressed || hovered || selected ? theme.colors.surfaceRaised : theme.colors.surface }]}>
    <Icon name={item.icon} size={18} tone={selected ? "text" : "textMuted"} />
    <AppText variant="body" tone={selected ? "text" : "textMuted"}
      style={[selected ? styles.activeLabel : undefined]}>{item.label}</AppText>
  </Control>;
}

/** Navigation adapts to the available window; every feature shares this frame. */
export function AppShell({ children }: { children: ReactNode }) {
  const { theme } = useTheme();
  const device = useDevice();
  const path = usePathname();
  const section = sectionFor(path);
  const windowLayout = useResponsiveLayout();
  const wide = windowLayout.size === "expanded";
  const [contentWidth, setContentWidth] = useState(windowLayout.maxContentWidth);
  const isDetail = !destinations.some((item) => item.href === path);
  const activeLabel = path.startsWith("/preview")
    ? "Preview"
    : path === "/notifications"
      ? "Notifications"
      : path === "/connection-log"
        ? "Connection log"
      : destinations.find((item) => item.href === section)?.label ?? "Home";
  const back = () => router.canGoBack() ? router.back() : router.replace("/");
  const pairingLabel = device.device
    ? device.device.deviceName
    : device.ready ? "Not paired" : "Loading…";

  // The desktop companion owns its compact navigation and tab canvas. The
  // pairing shell must not consume the terminal's reading/input area.
  // The welcome guide is full screen.
  if (path === "/welcome") return <>{children}</>;
  if (path.startsWith("/preview") || path.startsWith("/machine")) return (
    <SafeAreaView edges={["top", "bottom", "left", "right"]}
      style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <View style={styles.content} onLayout={(event) => setContentWidth(event.nativeEvent.layout.width)}>
        <LayoutViewportProvider width={contentWidth}>{children}</LayoutViewportProvider>
      </View>
    </SafeAreaView>
  );

  return <SafeAreaView edges={["top", "bottom", "left", "right"]}
    style={[styles.frame, { backgroundColor: theme.colors.background }]}>
    <View style={styles.horizontal}>
      {wide ? <View style={[styles.sidebar, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
        <View style={styles.sidebarBrand}><Brand /></View>
        <AppText variant="micro" tone="textSubtle" style={styles.navCaption}>WORKSPACE</AppText>
        <View style={styles.sidebarLinks}>
          {destinations.map((item) => <NavigationItem key={item.href} item={item} selected={section === item.href} />)}
        </View>
        <View style={styles.sidebarSpacer} />
        <Control accessibilityRole="button" accessibilityLabel="Open settings" onPress={() => router.navigate("/settings")}
          style={[styles.profile, { borderColor: theme.colors.border }]}>
          <View style={[styles.avatar, { backgroundColor: theme.colors.surfaceRaised }]}><Icon name="settings" size={18} /></View>
          <View style={styles.profileCopy}>
            <AppText variant="caption" style={styles.activeLabel} numberOfLines={1}>{pairingLabel}</AppText>
            <AppText variant="caption" tone="textSubtle" numberOfLines={1}>
              {device.device ? "Paired phone" : "Pair with your desktop"}
            </AppText>
          </View>
          <Icon name="chevron-right" size={14} />
        </Control>
      </View> : null}
      <View style={styles.main} onLayout={(event) => setContentWidth(event.nativeEvent.layout.width)}>
        <View style={[styles.topbar, { borderColor: theme.colors.border, backgroundColor: theme.colors.background }]}>
          {wide ? <View style={styles.breadcrumb}>
            {isDetail ? <IconButton name="arrow-left" label="Back" onPress={back} /> : null}
            <AppText variant="caption" tone="textSubtle">Workspace</AppText>
            <Icon name="chevron-right" size={12} />
            <AppText variant="caption">{activeLabel}</AppText>
          </View> : isDetail ? <View style={styles.backLabel}>
            <IconButton name="arrow-left" label="Back" onPress={back} />
            <AppText variant="body" style={styles.activeLabel}>{activeLabel}</AppText>
          </View> : <Brand />}
          <View style={styles.topbarActions}>
            {wide ? <AppText variant="caption" tone="textSubtle">Desktop companion</AppText> : null}
            <AlertsButton />
            <IconButton name="settings" label="Settings" onPress={() => router.navigate("/settings")} />
          </View>
        </View>
        <LayoutViewportProvider width={contentWidth}>
          <View style={styles.content}>{children}</View>
        </LayoutViewportProvider>
      </View>
    </View>
</SafeAreaView>;
}

const styles = StyleSheet.create({
  frame: { flex: 1 }, horizontal: { flex: 1, flexDirection: "row" },
  sidebar: { width: 216, borderRightWidth: 1, paddingHorizontal: 14 },
  sidebarBrand: { paddingHorizontal: 10, paddingTop: 25, paddingBottom: 38 },
  brand: { flexDirection: "row", gap: 9, alignItems: "center", borderRadius: 8 },
  brandPressed: { opacity: 0.6 },
  badge: { position: "absolute", top: 4, right: 2, minWidth: 16, height: 16, borderRadius: 8, paddingHorizontal: 4, alignItems: "center", justifyContent: "center" },
  badgeText: { fontSize: 10, lineHeight: 12, fontWeight: "700" },
  mark: { width: 28, height: 28 },
  wordmark: { fontSize: 19, fontWeight: "700", letterSpacing: -0.6 },
  navCaption: { paddingHorizontal: 12, marginBottom: 12, letterSpacing: 1 },
  sidebarLinks: { gap: 4 }, sidebarLink: { minHeight: 44, paddingHorizontal: 12, paddingVertical: 12, gap: 11, flexDirection: "row", alignItems: "center", borderRadius: 7 },
  activeLabel: { fontWeight: "600" }, sidebarSpacer: { flex: 1 },
  profile: { flexDirection: "row", alignItems: "center", gap: 10, paddingVertical: 20, paddingHorizontal: 4, borderTopWidth: 1 },
  avatar: { width: 32, height: 32, borderRadius: 7, alignItems: "center", justifyContent: "center" },
  profileCopy: { flex: 1, minWidth: 0, gap: 2 },
  main: { flex: 1, minWidth: 0 }, topbar: { minHeight: 58, paddingHorizontal: 20, flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 12, borderBottomWidth: 1 },
  breadcrumb: { flexDirection: "row", gap: 10, alignItems: "center", minWidth: 0, flex: 1 },
  backLabel: { flexDirection: "row", alignItems: "center", gap: 6, flex: 1 },
  topbarActions: { flexDirection: "row", gap: 16, alignItems: "center" }, content: { flex: 1 },
});
