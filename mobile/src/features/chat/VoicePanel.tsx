/**
 * Voice mode's face, in place of the message box: an orb that follows your
 * voice while it listens, breathes while the agent works and glows while it
 * reads the reply; what it heard; and End. Tapping the orb sends now, skips
 * the reading, or tries again after a problem.
 */
import { useEffect, useRef } from "react";
import { Animated, Easing, StyleSheet, View } from "react-native";
import { AppText, Control, Icon } from "../../components";
import { useTheme } from "../../theme";
import type { VoicePhase } from "./useVoiceConversation";

function statusOf(phase: VoicePhase, name: string): string {
  switch (phase) {
    case "starting":
      return "Starting…";
    case "listening":
      return "Listening";
    case "sending":
      return "Sending…";
    case "waiting":
      return `${name} is working`;
    case "speaking":
      return `${name}`;
    case "error":
      return "Voice paused";
    default:
      return "";
  }
}

function hintOf(phase: VoicePhase): string {
  switch (phase) {
    case "listening":
      return "Pause when you're done, or tap to send";
    case "speaking":
      return "Tap to stop reading and talk";
    case "error":
      return "Tap to talk again";
    case "waiting":
      return "It reads the reply when it's ready";
    default:
      return "";
  }
}

export function VoicePanel({
  phase,
  heard,
  level,
  error,
  name,
  onNext,
  onEnd,
  bottom,
}: {
  phase: VoicePhase;
  heard: string;
  level: number;
  error: string | null;
  name: string;
  onNext: () => void;
  onEnd: () => void;
  /** Space below the panel (the keyboard / safe area). */
  bottom: number;
}) {
  const { theme } = useTheme();
  const voiceScale = useRef(new Animated.Value(1)).current;
  const breathe = useRef(new Animated.Value(0)).current;

  // Follows your voice while listening.
  useEffect(() => {
    if (phase !== "listening") return;
    Animated.timing(voiceScale, { toValue: 1 + Math.min(1, level) * 0.35, duration: 110, useNativeDriver: true }).start();
  }, [phase, level, voiceScale]);

  // Breathes while the agent works, pulses while it speaks.
  useEffect(() => {
    if (phase !== "waiting" && phase !== "speaking" && phase !== "sending" && phase !== "starting") {
      breathe.stopAnimation();
      breathe.setValue(0);
      return;
    }
    const fast = phase === "speaking";
    const loop = Animated.loop(
      Animated.sequence([
        Animated.timing(breathe, { toValue: 1, duration: fast ? 520 : 1100, easing: Easing.inOut(Easing.quad), useNativeDriver: true }),
        Animated.timing(breathe, { toValue: 0, duration: fast ? 520 : 1100, easing: Easing.inOut(Easing.quad), useNativeDriver: true }),
      ]),
    );
    loop.start();
    return () => loop.stop();
  }, [phase, breathe]);

  const tone =
    phase === "speaking" ? "#A78BFA" : phase === "error" ? theme.colors.danger : phase === "listening" ? theme.colors.accent : theme.colors.textMuted;
  const pulse = breathe.interpolate({ inputRange: [0, 1], outputRange: [1, phase === "speaking" ? 1.18 : 1.1] });
  const halo = breathe.interpolate({ inputRange: [0, 1], outputRange: [0.18, phase === "speaking" ? 0.45 : 0.3] });
  const scale = phase === "listening" ? voiceScale : pulse;

  return (
    <View style={[styles.dock, { paddingBottom: bottom + 8 }]} pointerEvents="box-none">
      <View style={[styles.card, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
        <View style={styles.head}>
          <View style={[styles.dot, { backgroundColor: tone }]} />
          <AppText style={styles.status} numberOfLines={1}>{statusOf(phase, name)}</AppText>
          <AppText variant="caption" tone="textSubtle">Voice</AppText>
        </View>

        <Control
          accessibilityRole="button"
          accessibilityLabel={hintOf(phase) || "Voice"}
          onPress={onNext}
          style={styles.orbHost}
        >
          <Animated.View style={[styles.halo, { backgroundColor: tone, opacity: phase === "listening" ? 0.22 : halo, transform: [{ scale }] }]} />
          <Animated.View style={[styles.orb, { backgroundColor: tone, transform: [{ scale }] }]}>
            <Icon name={phase === "speaking" ? "voice" : phase === "error" ? "refresh" : "mic"} size={26} tone="onAccent" />
          </Animated.View>
        </Control>

        <AppText style={styles.heard} numberOfLines={3}>
          {error ?? (heard || (phase === "listening" ? "Say something…" : " "))}
        </AppText>
        <AppText variant="caption" tone="textMuted" style={styles.hint}>{hintOf(phase)}</AppText>

        <View style={styles.actions}>
          <Control
            accessibilityRole="button"
            accessibilityLabel="End voice"
            onPress={onEnd}
            hitSlop={8}
            style={({ pressed }) => [styles.end, { backgroundColor: theme.colors.danger, opacity: pressed ? 0.8 : 1 }]}
          >
            <Icon name="close" size={20} tone="onAccent" />
          </Control>
        </View>
      </View>
    </View>
  );
}

const ORB = 76;

const styles = StyleSheet.create({
  dock: { position: "absolute", left: 0, right: 0, bottom: 0, paddingHorizontal: 12 },
  card: {
    borderRadius: 28,
    borderCurve: "continuous",
    borderWidth: StyleSheet.hairlineWidth,
    paddingHorizontal: 18,
    paddingTop: 14,
    paddingBottom: 16,
    alignItems: "center",
    gap: 6,
    shadowColor: "#000",
    shadowOpacity: 0.1,
    shadowRadius: 6,
    shadowOffset: { width: 0, height: 2 },
    elevation: 2,
  },
  head: { flexDirection: "row", alignItems: "center", gap: 8, alignSelf: "stretch" },
  dot: { width: 8, height: 8, borderRadius: 4 },
  status: { flex: 1, minWidth: 0, fontWeight: "600" },
  orbHost: { width: ORB * 1.9, height: ORB * 1.9, alignItems: "center", justifyContent: "center" },
  halo: { position: "absolute", width: ORB * 1.6, height: ORB * 1.6, borderRadius: ORB * 0.8 },
  orb: { width: ORB, height: ORB, borderRadius: ORB / 2, alignItems: "center", justifyContent: "center" },
  heard: { textAlign: "center", fontSize: 16, lineHeight: 22, minHeight: 22 },
  hint: { textAlign: "center" },
  actions: { flexDirection: "row", justifyContent: "center", paddingTop: 6 },
  end: { width: 52, height: 52, borderRadius: 26, alignItems: "center", justifyContent: "center" },
});
