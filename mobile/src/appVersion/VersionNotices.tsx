/**
 * The two ways a new version is announced: a card over the bottom of the app
 * ("A new version is available" → Update / Later), and, when this build can
 * no longer work, a full screen that asks for the update first.
 */
import { useEffect, useRef, useState } from "react";
import { Animated, Modal, Platform, StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { AppText, Button, DragentMark, Icon } from "../components";
import { useTheme } from "../theme";
import { dismissNewVersion, openUpdate, useVersionState } from "./useVersionNotice";

const WHERE = Platform.OS === "ios" ? "TestFlight" : "the download page";

export function VersionNotices() {
  const version = useVersionState();
  if (version.notice.kind === "required") return <UpdateRequired url={version.notice.release.url} notes={version.notice.release.notes} />;
  if (version.notice.kind === "available" && !version.dismissed) return <UpdateCard url={version.notice.release.url} notes={version.notice.release.notes} />;
  return null;
}

function UpdateCard({ url, notes }: { url: string; notes: string | null }) {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const rise = useRef(new Animated.Value(0)).current;
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    Animated.spring(rise, { toValue: 1, useNativeDriver: true, damping: 18, stiffness: 170 }).start();
  }, [rise]);
  const translateY = rise.interpolate({ inputRange: [0, 1], outputRange: [120, 0] });
  return (
    <View pointerEvents="box-none" style={[styles.host, { bottom: insets.bottom + 12 }]}>
      <Animated.View
        accessibilityRole="alert"
        style={[styles.card, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border, transform: [{ translateY }], opacity: rise }]}
      >
        <View style={[styles.mark, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
          <DragentMark size={22} />
        </View>
        <View style={styles.text}>
          <AppText style={styles.title}>A new version is available</AppText>
          <AppText variant="caption" tone="textMuted" numberOfLines={2}>
            {failed ? `Couldn't open ${WHERE}. Try again from Settings.` : (notes ?? `Get it from ${WHERE}.`)}
          </AppText>
        </View>
        <Button label="Later" size="sm" variant="ghost" onPress={dismissNewVersion} />
        <Button label="Update" size="sm" onPress={() => void openUpdate(url).then((ok) => setFailed(!ok))} />
      </Animated.View>
    </View>
  );
}

function UpdateRequired({ url, notes }: { url: string; notes: string | null }) {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const [failed, setFailed] = useState(false);
  return (
    <Modal visible transparent={false} animationType="fade" statusBarTranslucent onRequestClose={() => {}}>
      <View style={[styles.required, { backgroundColor: theme.colors.background, paddingTop: insets.top + 24, paddingBottom: insets.bottom + 24 }]}>
        <View style={styles.requiredBody}>
          <View style={[styles.bigMark, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
            <DragentMark size={48} />
            <View style={[styles.badge, { backgroundColor: theme.colors.accent }]}>
              <Icon name="refresh" size={14} tone="onAccent" />
            </View>
          </View>
          <AppText style={styles.requiredTitle}>Update Dragent to continue</AppText>
          <AppText tone="textMuted" style={styles.requiredText}>
            This version can no longer talk to your desktop. The new one is ready in {WHERE}.
          </AppText>
          {notes ? <AppText variant="caption" tone="textSubtle" style={styles.requiredText}>{notes}</AppText> : null}
        </View>
        <Button label="Update now" onPress={() => void openUpdate(url).then((ok) => setFailed(!ok))} />
        {failed ? <AppText variant="caption" tone="danger" style={styles.requiredText}>{`Couldn't open ${WHERE}. Open it yourself and update Dragent.`}</AppText> : null}
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  host: { position: "absolute", left: 12, right: 12, zIndex: 900, elevation: 900 },
  card: {
    flexDirection: "row",
    alignItems: "center",
    gap: 10,
    borderRadius: 20,
    borderCurve: "continuous",
    borderWidth: StyleSheet.hairlineWidth,
    paddingVertical: 10,
    paddingLeft: 10,
    paddingRight: 8,
    shadowColor: "#000",
    shadowOpacity: 0.1,
    shadowRadius: 6,
    shadowOffset: { width: 0, height: 2 },
    elevation: 2,
  },
  mark: { width: 36, height: 36, borderRadius: 10, borderWidth: StyleSheet.hairlineWidth, alignItems: "center", justifyContent: "center" },
  text: { flex: 1, minWidth: 0 },
  title: { fontWeight: "600" },
  required: { flex: 1, paddingHorizontal: 24, gap: 12 },
  requiredBody: { flex: 1, alignItems: "center", justifyContent: "center", gap: 12 },
  bigMark: { width: 88, height: 88, borderRadius: 24, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth, alignItems: "center", justifyContent: "center", marginBottom: 8 },
  badge: { position: "absolute", top: -6, right: -6, width: 28, height: 28, borderRadius: 14, alignItems: "center", justifyContent: "center" },
  requiredTitle: { fontSize: 24, lineHeight: 30, fontWeight: "700", textAlign: "center", letterSpacing: -0.4 },
  requiredText: { textAlign: "center" },
});
