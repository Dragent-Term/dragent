import { useState, type ReactNode } from "react";
import { StyleSheet, View } from "react-native";
import { AppText, Control } from "../../components";
import { spacing } from "../../theme/tokens";

export interface ChatDisclosureProps {
  label: string;
  showLabel: string;
  hideLabel: string;
  defaultExpanded?: boolean;
  children: ReactNode;
}

/** Single collapsible section for verbose thoughts/tool output. */
export function ChatDisclosure({ label, showLabel, hideLabel, defaultExpanded = false, children }: ChatDisclosureProps) {
  const [expanded, setExpanded] = useState(defaultExpanded);
  return (
    <View style={styles.shell}>
      <Control
        accessibilityRole="button"
        accessibilityState={{ expanded }}
        accessibilityLabel={expanded ? `${label}: ${hideLabel}` : `${label}: ${showLabel}`}
        accessibilityHint={expanded ? "Collapses this section." : "Expands this section to read the full text."}
        onPress={() => setExpanded((value) => !value)}
        style={styles.toggle}
      >
        <AppText variant="caption" tone="accent" style={styles.toggleLabel}>
          {expanded ? hideLabel : showLabel}
        </AppText>
      </Control>
      {expanded ? children : null}
    </View>
  );
}

export function ChatClippedNote() {
  return (
    <AppText variant="caption" tone="textSubtle">
      Earlier conversation or long content was clipped to keep this view responsive.
    </AppText>
  );
}

const styles = StyleSheet.create({
  shell: { minWidth: 0, gap: spacing.xs },
  toggle: { alignSelf: "flex-start", minHeight: 44, minWidth: 44, justifyContent: "center", paddingVertical: spacing.xs },
  toggleLabel: { fontWeight: "600" },
});
