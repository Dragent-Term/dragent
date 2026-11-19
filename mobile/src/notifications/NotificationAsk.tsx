/**
 * Asked once, after a desktop is paired: what notifications bring, before
 * the system's own prompt. "Turn on" shows the system prompt and registers
 * this phone; "Not now" leaves it to Settings → Notifications.
 */
import { useEffect, useRef } from "react";
import { Animated, Easing, Modal, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { AppText, Button, DragentMark, Icon, type IconName } from "../components";
import { useTheme } from "../theme";

const POINTS: { icon: IconName; title: string; text: string }[] = [
  { icon: "message", title: "When an agent needs you", text: "A question or a permission to approve, wherever you are." },
  { icon: "check", title: "When work is done", text: "Know the moment an agent finishes, with what it did." },
  { icon: "agent", title: "Live on your lock screen", text: "Agents at work and waiting, at a glance, as they change." },
  { icon: "shield", title: "Private", text: "Encrypted on your desktop, opened only on this phone." },
];

export function NotificationAsk({
  visible,
  pending,
  onAllow,
  onLater,
}: {
  visible: boolean;
  pending: boolean;
  onAllow: () => void;
  onLater: () => void;
}) {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const rise = useRef(new Animated.Value(0)).current;
  const ring = useRef(new Animated.Value(0)).current;

  useEffect(() => {
    if (!visible) return;
    rise.setValue(0);
    Animated.spring(rise, { toValue: 1, useNativeDriver: true, damping: 18, stiffness: 160 }).start();
    const loop = Animated.loop(
      Animated.sequence([
        Animated.timing(ring, { toValue: 1, duration: 1400, easing: Easing.out(Easing.quad), useNativeDriver: true }),
        Animated.timing(ring, { toValue: 0, duration: 0, useNativeDriver: true }),
      ]),
    );
    loop.start();
    return () => loop.stop();
  }, [visible, rise, ring]);

  const translateY = rise.interpolate({ inputRange: [0, 1], outputRange: [60, 0] });
  const ringScale = ring.interpolate({ inputRange: [0, 1], outputRange: [1, 1.7] });
  const ringOpacity = ring.interpolate({ inputRange: [0, 1], outputRange: [0.35, 0] });

  return (
    <Modal visible={visible} transparent animationType="fade" statusBarTranslucent onRequestClose={onLater}>
      <View style={[styles.scrim, { backgroundColor: "rgba(0,0,0,0.55)" }]}>
        <Animated.View
          style={[
            styles.sheet,
            {
              backgroundColor: theme.colors.surface,
              borderColor: theme.colors.border,
              paddingBottom: insets.bottom + 18,
              transform: [{ translateY }],
              opacity: rise,
            },
          ]}
        >
          <View style={[styles.grabber, { backgroundColor: theme.colors.border }]} />
          <View style={styles.hero}>
            <Animated.View style={[styles.ring, { borderColor: theme.colors.accent, opacity: ringOpacity, transform: [{ scale: ringScale }] }]} />
            <View style={[styles.markTile, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
              <DragentMark size={44} />
            </View>
            <View style={[styles.bell, { backgroundColor: theme.colors.accent }]}>
              <Icon name="message" size={14} tone="onAccent" />
            </View>
          </View>
          <AppText style={styles.title}>Stay in the loop</AppText>
          <AppText tone="textMuted" style={styles.lead}>
            Let Dragent tell you when your agents need you, even with the app closed.
          </AppText>
          <View style={styles.points}>
            {POINTS.map((point) => (
              <View key={point.title} style={styles.point}>
                <View style={[styles.pointIcon, { backgroundColor: theme.colors.surfaceRaised }]}>
                  <Icon name={point.icon} size={16} tone="text" />
                </View>
                <View style={styles.pointText}>
                  <AppText style={styles.pointTitle}>{point.title}</AppText>
                  <AppText variant="caption" tone="textMuted">{point.text}</AppText>
                </View>
              </View>
            ))}
          </View>
          <Button label="Turn on notifications" loading={pending} disabled={pending} onPress={onAllow} style={styles.allow} />
          <Button label="Not now" variant="ghost" disabled={pending} onPress={onLater} />
          <AppText variant="caption" tone="textSubtle" style={styles.foot}>You can change this anytime in Settings.</AppText>
        </Animated.View>
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  scrim: { flex: 1, justifyContent: "flex-end" },
  sheet: {
    borderTopLeftRadius: 28,
    borderTopRightRadius: 28,
    borderCurve: "continuous",
    borderWidth: StyleSheet.hairlineWidth,
    paddingHorizontal: 22,
    paddingTop: 10,
    gap: 10,
    alignItems: "stretch",
  },
  grabber: { alignSelf: "center", width: 38, height: 5, borderRadius: 3, marginBottom: 8 },
  hero: { alignSelf: "center", width: 96, height: 96, alignItems: "center", justifyContent: "center", marginBottom: 4 },
  ring: { position: "absolute", width: 76, height: 76, borderRadius: 24, borderWidth: 2 },
  markTile: { width: 76, height: 76, borderRadius: 22, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth, alignItems: "center", justifyContent: "center" },
  bell: { position: "absolute", top: 4, right: 4, width: 26, height: 26, borderRadius: 13, alignItems: "center", justifyContent: "center" },
  title: { fontSize: 24, lineHeight: 30, fontWeight: "700", textAlign: "center", letterSpacing: -0.4 },
  lead: { textAlign: "center", fontSize: 15, lineHeight: 21, marginBottom: 4 },
  points: { gap: 14, paddingVertical: 8 },
  point: { flexDirection: "row", alignItems: "center", gap: 12 },
  pointIcon: { width: 36, height: 36, borderRadius: 10, alignItems: "center", justifyContent: "center" },
  pointText: { flex: 1, minWidth: 0, gap: 1 },
  pointTitle: { fontWeight: "600" },
  allow: { marginTop: 6 },
  foot: { textAlign: "center" },
});
