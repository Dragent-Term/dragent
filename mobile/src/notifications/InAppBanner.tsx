/**
 * The app's own notification banner, for a push that arrives while Dragent
 * is open: a card that slides down from the top, stays a few seconds (longer
 * while you touch it), goes on a swipe up, and opens the desktop it came
 * from on a tap. The system shows nothing then; the notification still goes
 * to the notification list.
 *
 * - iOS: the notification service extension has decrypted it already; the
 *   app is told not to present it and shows it here.
 * - Android: the push service hands a decrypted push to the app while it is
 *   in front (`onForegroundPush`) and files it quietly itself.
 *
 * Tapping a system notification (the app in the background) opens its
 * desktop too.
 */
import { markOpenedFromNotification } from "../navigation/awayHome";
import { useCallback, useEffect, useRef, useState } from "react";
import { Animated, AppState, PanResponder, Platform, StyleSheet, View } from "react-native";
import { requireOptionalNativeModule } from "expo";
import { router } from "expo-router";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { AppText, DragentMark } from "../components";
import { useTheme } from "../theme";
import { machineAnswerPath, machinePath } from "../machines/routes";
import { ANSWER_CATEGORY, isChoice, isPaneId } from "./answer";
import { loadNotificationHistory, recordNotification } from "./history";

interface Banner {
  readonly id: number;
  readonly title: string;
  readonly body: string;
  readonly desktop: string | null;
}

/** How long a banner stays when left alone. */
const SHOW_MS = 5000;
const HIDDEN_Y = -160;

function clean(value: unknown, max: number): string {
  return typeof value === "string" ? value.trim().slice(0, max) : "";
}

function desktopOf(value: unknown): string | null {
  return typeof value === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(value) ? value : null;
}

/** A system notification into the phone's alert history. */
function remember(
  notification: { date: number; request: { identifier: string; content: { title: string | null; body: string | null; data?: unknown } } },
  read: boolean,
) {
  const content = notification.request.content;
  const data = (content.data ?? null) as Record<string, unknown> | null;
  recordNotification(
    {
      id: notification.request.identifier,
      title: clean(content.title, 120) || "Dragent",
      body: clean(content.body, 400),
      desktop: desktopOf(data?.dragent_desktop),
      at: notification.date > 1e12 ? notification.date : notification.date * 1000,
    },
    read,
  );
}

/** Allow or Deny was pressed on an agent's notification: answer it. */
function answer(action: string, data: Record<string, unknown> | null): boolean {
  const desktop = desktopOf(data?.dragent_desktop);
  const pane = data?.dragent_pane;
  if (!desktop || !isPaneId(pane) || !isChoice(action)) return false;
  markOpenedFromNotification();
  router.push(machineAnswerPath(desktop, pane, action));
  return true;
}

/** Open the desktop a notification is about (else Home). */
function openDesktop(desktop: string | null) {
  markOpenedFromNotification();
  if (desktop) router.push(machinePath(desktop));
  else router.navigate("/");
}

export function InAppBanners() {
  const [banner, setBanner] = useState<Banner | null>(null);
  const next = useRef(0);

  const show = useCallback((title: unknown, body: unknown, desktop: unknown) => {
    const shownTitle = clean(title, 120) || "Dragent";
    const shownBody = clean(body, 400);
    recordNotification({ title: shownTitle, body: shownBody, desktop: desktopOf(desktop) });
    next.current += 1;
    setBanner({ id: next.current, title: shownTitle, body: shownBody, desktop: desktopOf(desktop) });
  }, []);

  useEffect(() => {
    if (Platform.OS !== "ios" && Platform.OS !== "android") return;
    let active = true;
    const subscriptions: { remove(): void }[] = [];

    // Android: the push service's hand-off while the app is in front.
    if (Platform.OS === "android") {
      const native = requireOptionalNativeModule<{
        addListener?: (event: string, listener: (event: Record<string, unknown>) => void) => { remove(): void };
      }>("DragentE2EE");
      const subscription = native?.addListener?.("onForegroundPush", (event) => show(event.title, event.body, event.desktop));
      if (subscription) subscriptions.push(subscription);
    }

    void loadNotificationHistory();
    void import("expo-notifications")
      .then((Notifications) => {
        if (!active) return;
        // Allow / Deny on an agent's permission request (iOS; Android's
        // buttons open the same answer screen by link).
        void Notifications.setNotificationCategoryAsync(ANSWER_CATEGORY, [
          { identifier: "allow", buttonTitle: "Allow", options: { opensAppToForeground: true } },
          { identifier: "deny", buttonTitle: "Deny", options: { opensAppToForeground: true, isDestructive: true } },
        ]).catch(() => {});
        // Into the history: what arrived while the app was closed (still in
        // the notification centre), now and each time the app comes back.
        const collect = () =>
          void Notifications.getPresentedNotificationsAsync()
            .then((shown) => shown.forEach((notification) => remember(notification, false)))
            .catch(() => {});
        collect();
        const appState = AppState.addEventListener("change", (state) => {
          if (state === "active") collect();
        });
        subscriptions.push(appState);
        // iOS: show ours, not the system's, while the app is open; keep it
        // in the notification list.
        Notifications.setNotificationHandler({
          handleNotification: async () => ({
            shouldShowBanner: false,
            shouldShowList: true,
            shouldPlaySound: false,
            shouldSetBadge: false,
          }),
        });
        if (Platform.OS === "ios") {
          subscriptions.push(
            Notifications.addNotificationReceivedListener((notification) => {
              const content = notification.request.content;
              show(content.title, content.body, (content.data as Record<string, unknown> | null)?.dragent_desktop);
            }),
          );
        }
        // A tap on a system notification: open its desktop.
        subscriptions.push(
          Notifications.addNotificationResponseReceivedListener((response) => {
            const data = response.notification.request.content.data as Record<string, unknown> | null;
            remember(response.notification, true);
            if (answer(response.actionIdentifier, data)) return;
            openDesktop(desktopOf(data?.dragent_desktop));
          }),
        );
        // Opened from a notification while the app wasn't running.
        void Notifications.getLastNotificationResponseAsync().then((response) => {
          if (!active || !response) return;
          const data = response.notification.request.content.data as Record<string, unknown> | null;
          remember(response.notification, true);
          if (!answer(response.actionIdentifier, data)) openDesktop(desktopOf(data?.dragent_desktop));
          void Notifications.clearLastNotificationResponseAsync?.();
        });
      })
      .catch(() => {});

    return () => {
      active = false;
      for (const subscription of subscriptions) subscription.remove();
    };
  }, [show]);

  if (!banner) return null;
  return (
    <BannerCard
      key={banner.id}
      banner={banner}
      onGone={() => setBanner((current) => (current?.id === banner.id ? null : current))}
    />
  );
}

function BannerCard({ banner, onGone }: { banner: Banner; onGone: () => void }) {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const y = useRef(new Animated.Value(HIDDEN_Y)).current;
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const leaving = useRef(false);

  const leave = useCallback(() => {
    if (leaving.current) return;
    leaving.current = true;
    if (timer.current) clearTimeout(timer.current);
    Animated.timing(y, { toValue: HIDDEN_Y, duration: 220, useNativeDriver: true }).start(() => onGone());
  }, [y, onGone]);

  const wait = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(leave, SHOW_MS);
  }, [leave]);

  useEffect(() => {
    Animated.spring(y, { toValue: 0, useNativeDriver: true, damping: 18, stiffness: 180, mass: 0.9 }).start();
    wait();
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, [y, wait]);

  const pan = useRef(
    PanResponder.create({
      onMoveShouldSetPanResponder: (_, gesture) => Math.abs(gesture.dy) > 4,
      onPanResponderGrant: () => {
        if (timer.current) clearTimeout(timer.current);
      },
      // Up follows the finger; down only gives a little.
      onPanResponderMove: (_, gesture) => y.setValue(gesture.dy < 0 ? gesture.dy : gesture.dy * 0.15),
      onPanResponderRelease: (_, gesture) => {
        if (gesture.dy < -30 || gesture.vy < -0.6) {
          leave();
        } else {
          Animated.spring(y, { toValue: 0, useNativeDriver: true, damping: 18, stiffness: 220 }).start();
          wait();
        }
      },
    }),
  ).current;

  return (
    <View pointerEvents="box-none" style={[styles.host, { top: insets.top + 6 }]}>
      <Animated.View
        {...pan.panHandlers}
        style={[
          styles.card,
          {
            backgroundColor: theme.colors.surfaceRaised,
            borderColor: theme.colors.border,
            transform: [{ translateY: y }],
          },
        ]}
        onTouchStart={() => {
          if (timer.current) clearTimeout(timer.current);
        }}
        onTouchEnd={wait}
        accessible
        accessibilityRole="button"
        accessibilityLabel={`${banner.title}. ${banner.body}`}
        accessibilityHint="Opens the desktop it is about. Swipe up to dismiss."
        accessibilityLiveRegion="polite"
        onAccessibilityTap={() => {
          leave();
          openDesktop(banner.desktop);
        }}
      >
        <View
          style={styles.row}
          onStartShouldSetResponder={() => true}
          onResponderRelease={() => {
            leave();
            openDesktop(banner.desktop);
          }}
        >
          <View style={[styles.mark, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
            <DragentMark size={24} />
          </View>
          <View style={styles.text}>
            <View style={styles.head}>
              <AppText numberOfLines={1} style={styles.title}>{banner.title}</AppText>
              <AppText variant="caption" tone="textSubtle">now</AppText>
            </View>
            {banner.body ? (
              <AppText variant="caption" tone="textMuted" numberOfLines={2} style={styles.body}>
                {banner.body}
              </AppText>
            ) : null}
          </View>
        </View>
        <View style={[styles.grabber, { backgroundColor: theme.colors.border }]} />
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  host: { position: "absolute", left: 10, right: 10, zIndex: 1000, elevation: 1000 },
  card: {
    borderRadius: 20,
    borderCurve: "continuous",
    borderWidth: StyleSheet.hairlineWidth,
    paddingTop: 12,
    paddingBottom: 8,
    paddingHorizontal: 12,
    shadowColor: "#000",
    shadowOpacity: 0.1,
    shadowRadius: 6,
    shadowOffset: { width: 0, height: 2 },
    elevation: 2,
  },
  row: { flexDirection: "row", alignItems: "center", gap: 12 },
  mark: {
    width: 40,
    height: 40,
    borderRadius: 11,
    borderCurve: "continuous",
    borderWidth: StyleSheet.hairlineWidth,
    alignItems: "center",
    justifyContent: "center",
  },
  text: { flex: 1, minWidth: 0, gap: 1 },
  head: { flexDirection: "row", alignItems: "center", gap: 8 },
  title: { flex: 1, minWidth: 0, fontSize: 15, fontWeight: "600" },
  body: { fontSize: 13.5, lineHeight: 18 },
  grabber: { alignSelf: "center", width: 36, height: 4, borderRadius: 2, marginTop: 8 },
});
