import { useRef, useState, type ComponentRef } from "react";
import { ScrollView, StyleSheet, useWindowDimensions, View, type NativeScrollEvent, type NativeSyntheticEvent } from "react-native";
import { router } from "expo-router";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { AppText, Button, DragentMark, Icon, type IconName } from "../../components";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";

export const WELCOME_SEEN_KEY = "dragent.welcome.seen.v1";

interface Page {
  readonly title: string;
  readonly lead: string;
  readonly points: readonly { icon: IconName; text: string }[];
}

const PAGES: readonly Page[] = [
  {
    title: "Your agents, wherever you are",
    lead: "Dragent on your phone follows the agents working on your desktop.",
    points: [
      { icon: "agent", text: "See every agent and what it's doing, in chat or terminal." },
      { icon: "message", text: "Answer them, approve what they ask, and start new work." },
      { icon: "bell", text: "Get told the moment one needs you or finishes." },
    ],
  },
  {
    title: "Pair with your desktop",
    lead: "Your phone connects to the Dragent app on your computer.",
    points: [
      { icon: "monitor", text: "Open Dragent on your computer." },
      { icon: "settings", text: "Go to Settings → Mobile and choose Show QR code." },
      { icon: "scan", text: "Scan it here. That's it." },
    ],
  },
  {
    title: "Private by design",
    lead: "What passes between your phone and your desktop is end-to-end encrypted.",
    points: [
      { icon: "shield", text: "Our server only passes it along. It can't read it." },
      { icon: "monitor", text: "Your code and keys stay on your own machines." },
      { icon: "close", text: "Unpair a phone from either side at any time." },
    ],
  },
];

/** Shown once, before the first pairing: what the app does, how to pair, and that it's private. */
export function WelcomeScreen() {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const { width } = useWindowDimensions();
  const pager = useRef<ComponentRef<typeof ScrollView>>(null);
  const [page, setPage] = useState(0);
  const last = page === PAGES.length - 1;

  const finish = (scan: boolean) => {
    void AsyncStorage.setItem(WELCOME_SEEN_KEY, "1").catch(() => {});
    if (scan) router.replace("/pair-scan");
    else router.replace("/");
  };
  const goTo = (index: number) => {
    setPage(index);
    pager.current?.scrollTo({ x: index * width, animated: true });
  };
  const onScrollEnd = (event: NativeSyntheticEvent<NativeScrollEvent>) => {
    setPage(Math.round(event.nativeEvent.contentOffset.x / Math.max(1, width)));
  };

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background, paddingTop: insets.top, paddingBottom: insets.bottom + spacing.md }]}>
      <View style={styles.topBar}>
        {!last ? <Button label="Skip" size="sm" variant="ghost" onPress={() => goTo(PAGES.length - 1)} /> : <View />}
      </View>
      <ScrollView
        ref={pager}
        horizontal
        pagingEnabled
        showsHorizontalScrollIndicator={false}
        onMomentumScrollEnd={onScrollEnd}
        style={styles.pager}
      >
        {PAGES.map((entry, index) => (
          <View key={entry.title} style={[styles.page, { width }]}>
            <View style={[styles.hero, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
              {index === 0 ? <DragentMark size={56} /> : <Icon name={index === 1 ? "scan" : "shield"} size={40} tone="accent" />}
            </View>
            <AppText style={styles.title}>{entry.title}</AppText>
            <AppText tone="textMuted" style={styles.lead}>{entry.lead}</AppText>
            <View style={styles.points}>
              {entry.points.map((point, step) => (
                <View key={point.text} style={styles.point}>
                  <View style={[styles.pointIcon, { backgroundColor: theme.colors.surfaceRaised }]}>
                    {index === 1 ? (
                      <AppText style={[styles.step, { color: theme.colors.accent }]}>{step + 1}</AppText>
                    ) : (
                      <Icon name={point.icon} size={16} tone="text" />
                    )}
                  </View>
                  <AppText style={styles.pointText}>{point.text}</AppText>
                </View>
              ))}
            </View>
          </View>
        ))}
      </ScrollView>
      <View style={styles.dots} accessibilityLabel={`Page ${page + 1} of ${PAGES.length}`}>
        {PAGES.map((entry, index) => (
          <View key={entry.title} style={[styles.dot, { backgroundColor: index === page ? theme.colors.accent : theme.colors.border, width: index === page ? 18 : 6 }]} />
        ))}
      </View>
      <View style={styles.actions}>
        {last ? (
          <>
            <Button label="Scan the QR code" onPress={() => finish(true)} />
            <Button label="Not now" variant="ghost" onPress={() => finish(false)} />
          </>
        ) : (
          <Button label="Next" onPress={() => goTo(page + 1)} />
        )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1 },
  topBar: { flexDirection: "row", justifyContent: "flex-end", paddingHorizontal: spacing.md, minHeight: 44, alignItems: "center" },
  pager: { flex: 1 },
  page: { paddingHorizontal: spacing.xl, justifyContent: "center", gap: spacing.md },
  hero: { alignSelf: "center", width: 96, height: 96, borderRadius: 28, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth, alignItems: "center", justifyContent: "center", marginBottom: spacing.sm },
  title: { fontSize: 26, lineHeight: 32, fontWeight: "700", textAlign: "center", letterSpacing: -0.4 },
  lead: { fontSize: 16, lineHeight: 22, textAlign: "center" },
  points: { gap: spacing.md, marginTop: spacing.md },
  point: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  pointIcon: { width: 36, height: 36, borderRadius: radii.md, alignItems: "center", justifyContent: "center" },
  step: { fontWeight: "700", fontSize: 15 },
  pointText: { flex: 1, minWidth: 0, fontSize: 15, lineHeight: 21 },
  dots: { flexDirection: "row", justifyContent: "center", gap: 6, paddingVertical: spacing.md },
  dot: { height: 6, borderRadius: 3 },
  actions: { paddingHorizontal: spacing.lg, gap: spacing.xs },
});
