import { StyleSheet, View } from "react-native";
import { AppText } from "../../components";
import { SecurityScreen } from "./SecurityScreen";
import { SECURITY_SCREEN_KINDS, UPDATE_APP_MESSAGE, securityStateFor, type SecurityScreenKind } from "./securityStates";

const SAMPLE_DESKTOP = "Studio";

function sampleInput(kind: SecurityScreenKind): Parameters<typeof securityStateFor>[0] {
  switch (kind) {
    case "store-lost":
      return { secure: { state: "lost", problem: null } };
    case "update-app":
      return { socketError: UPDATE_APP_MESSAGE };
    case "update-desktop":
      return { desktopLegacy: true, desktopName: SAMPLE_DESKTOP };
    case "pair-again":
      return { secure: { state: "retrying", problem: "refused" }, desktopName: SAMPLE_DESKTOP };
    case "pair-desktop":
      return { secure: { state: "unpaired", problem: null } };
    case "cannot-connect":
      return { secure: { state: "retrying", problem: "handshake" }, desktopName: SAMPLE_DESKTOP };
    case "key-changed":
      return {
        secure: { state: "retrying", problem: "handshake" },
        wasOpen: true,
        desktopName: SAMPLE_DESKTOP,
      };
    case "interrupted":
      return { secure: { state: "retrying", problem: "interrupted" }, desktopName: SAMPLE_DESKTOP };
  }
}

/**
 * Every phone-side §9 screen (docs/design/e2ee.md §9) with the transport
 * state that triggers it. Mounted by the preview harness (one line);
 * each entry also names its trigger for reviewers without a running relay.
 */
export function SecurityPreview({ onAction }: { onAction: (kind: SecurityScreenKind) => void }) {
  return (
    <View style={styles.list}>
      {SECURITY_SCREEN_KINDS.map((kind) => {
        const state = securityStateFor(sampleInput(kind));
        if (!state) return null;
        return (
          <View key={kind} style={styles.entry}>
            <AppText variant="caption" tone="textMuted">
              {kind}
            </AppText>
            <SecurityScreen
              state={state}
              desktopName={SAMPLE_DESKTOP}
              onPairAgain={() => onAction(kind)}
              onRemoveDesktop={() => onAction(kind)}
              onRetry={() => onAction(kind)}
            />
          </View>
        );
      })}
    </View>
  );
}

const styles = StyleSheet.create({
  list: { gap: 24 },
  entry: { gap: 4 },
});
