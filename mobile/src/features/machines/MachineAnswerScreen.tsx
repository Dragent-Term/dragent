import { useEffect, useRef, useState } from "react";
import { StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AppText, Button, DragentMark, Icon } from "../../components";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import { useMachine } from "../../machines/MachineContext";
import { machinePath } from "../../machines/routes";
import { answerAgent, isChoice, isPaneId, type AnswerChoice } from "../../notifications/answer";

type Phase = "connecting" | "sending" | "done" | "refused" | "failed";

/**
 * Where Allow or Deny on a notification lands: it asks the desktop to answer
 * the agent's permission prompt (the desktop checks it's still waiting),
 * once, and says what happened.
 */
export function MachineAnswerScreen({ deviceId, pane, choice }: { deviceId: string; pane: string; choice: string }) {
  const { theme } = useTheme();
  const machine = useMachine();
  const [phase, setPhase] = useState<Phase>("connecting");
  const [reason, setReason] = useState<string | null>(null);
  const sent = useRef(false);
  const valid = isPaneId(pane) && isChoice(choice);

  useEffect(() => {
    if (!valid || sent.current || !machine.rpc) return;
    sent.current = true;
    setPhase("sending");
    answerAgent(machine.rpc, pane, choice as AnswerChoice).then(
      (result) => {
        if (result.answered) setPhase("done");
        else {
          setReason(result.reason);
          setPhase("refused");
        }
      },
      () => setPhase("failed"),
    );
  }, [valid, machine.rpc, pane, choice]);

  // The desktop never came back: say so instead of waiting forever.
  useEffect(() => {
    if (phase !== "connecting") return;
    const timer = setTimeout(() => setPhase((now) => (now === "connecting" ? "failed" : now)), 15_000);
    return () => clearTimeout(timer);
  }, [phase]);

  const allow = choice === "allow";
  const title = !valid
    ? "This answer can't be sent"
    : phase === "connecting"
      ? "Connecting to your desktop…"
      : phase === "sending"
        ? allow ? "Allowing…" : "Denying…"
        : phase === "done"
          ? allow ? "Allowed" : "Denied"
          : phase === "refused"
            ? "Not answered"
            : "The desktop didn't answer";
  const detail = !valid
    ? "Open the agent and answer it there."
    : phase === "done"
      ? allow ? "The agent carries on." : "The agent was told no and waits for you."
      : phase === "refused"
        ? reason
        : phase === "failed"
          ? "Check your desktop is online, then open the agent to answer it."
          : null;
  const icon = phase === "done" ? (allow ? "check" : "close") : phase === "refused" || phase === "failed" || !valid ? "message" : null;

  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <View style={[styles.mark, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
        <DragentMark size={44} />
        {icon ? (
          <View style={[styles.badge, { backgroundColor: phase === "done" ? theme.colors.accent : theme.colors.surface, borderColor: theme.colors.border }]}>
            <Icon name={icon} size={14} tone={phase === "done" ? "onAccent" : "text"} />
          </View>
        ) : null}
      </View>
      <AppText style={styles.title} accessibilityLiveRegion="polite">{title}</AppText>
      {detail ? <AppText tone="textMuted" style={styles.detail}>{detail}</AppText> : null}
      <View style={styles.actions}>
        <Button label="Open the agent's desktop" onPress={() => router.replace(machinePath(deviceId))} />
        <Button label="Done" variant="ghost" onPress={() => router.replace("/")} />
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, alignItems: "center", justifyContent: "center", padding: spacing.xl, gap: spacing.md },
  mark: { width: 88, height: 88, borderRadius: radii.xl, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth, alignItems: "center", justifyContent: "center", marginBottom: spacing.sm },
  badge: { position: "absolute", right: -6, top: -6, width: 28, height: 28, borderRadius: 14, borderWidth: StyleSheet.hairlineWidth, alignItems: "center", justifyContent: "center" },
  title: { fontSize: 24, lineHeight: 30, fontWeight: "700", textAlign: "center" },
  detail: { textAlign: "center", fontSize: 15, lineHeight: 21 },
  actions: { alignSelf: "stretch", gap: spacing.xs, marginTop: spacing.lg },
});
