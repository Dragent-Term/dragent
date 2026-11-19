import { StyleSheet, View } from "react-native";
import { AppText, Button } from "../../components";
import { spacing } from "../../theme/tokens";
import type { SecurityScreenState } from "./securityStates";

/**
 * Every phone-side §9 screen (docs/design/e2ee.md §9): the security key
 * changed, update the app, pair again, pair this desktop, can't connect,
 * and the pin-store reset. Transient notices (interrupted) render inline
 * through the same descriptor. No silent fallback anywhere: each state
 * names what happened and the one safe next step.
 */
export function SecurityScreen({
  state,
  desktopName,
  onPairAgain,
  onRemoveDesktop,
  onRetry,
}: {
  state: SecurityScreenState;
  desktopName?: string | undefined;
  onPairAgain: () => void;
  onRemoveDesktop?: (() => void) | undefined;
  onRetry?: (() => void) | undefined;
}) {
  const title =
    state.kind === "key-changed" && desktopName
      ? `This desktop's security key changed (${desktopName}).`
      : state.title;
  return (
    <View style={styles.shell} accessibilityRole="alert">
      <View style={styles.content}>
        <AppText accessibilityRole="header" style={styles.title}>
          {title}
        </AppText>
        {state.body ? (
          <AppText tone="textMuted" style={styles.centered}>
            {state.body}
          </AppText>
        ) : null}
        <View style={styles.actions}>
          {state.actions.includes("pair-again") ? (
            <Button label="Pair again" onPress={onPairAgain} />
          ) : null}
          {state.actions.includes("remove-desktop") && onRemoveDesktop ? (
            <Button label="Remove desktop" variant="ghost" onPress={onRemoveDesktop} />
          ) : null}
          {state.actions.includes("retry") && onRetry ? (
            <Button label="Retry" variant="ghost" onPress={onRetry} />
          ) : null}
        </View>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  shell: { alignItems: "center", paddingVertical: spacing.xxl, paddingHorizontal: spacing.lg },
  content: { width: "100%", maxWidth: 360, alignItems: "center", gap: spacing.sm },
  title: { textAlign: "center", fontWeight: "600", maxWidth: "100%" },
  centered: { textAlign: "center", maxWidth: "100%" },
  actions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm, justifyContent: "center", paddingTop: spacing.xs },
});
