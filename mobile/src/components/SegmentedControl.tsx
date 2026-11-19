import { useCallback, useRef, type ComponentRef } from "react";
import { Platform, ScrollView, StyleSheet, View } from "react-native";
import { useTheme } from "../theme";
import { componentPaddingScale, spacing } from "../theme/tokens";
import { AppText } from "./AppText";
import { Control } from "./Control";
import { useChoiceKeyboard } from "./useChoiceKeyboard";

export interface SegmentedOption<T extends string> {
  id: T;
  label: string;
  hint?: string;
}

export interface SegmentedControlProps<T extends string> {
  label: string;
  options: readonly SegmentedOption<T>[];
  value: T;
  onChange: (id: T) => void;
  layout?: "wrap" | "scroll";
  /** Compact options for dense popover contexts. Defaults to the roomy 48pt minimum. */
  size?: "md" | "sm";
}

/**
 * One quiet radio group for choosing between a few modes (session view,
 * terminal entry). Selected options expose checked state for screen readers.
 * `md` meets the 48pt touch minimum; `sm` fits dense popover contexts.
 */
export function SegmentedControl<T extends string>({
  label,
  options,
  value,
  onChange,
  layout = "wrap",
  size = "md",
}: SegmentedControlProps<T>) {
  const { theme } = useTheme();
  const group = useRef<ComponentRef<typeof View>>(null);
  const selectedIndex = Math.max(0, options.findIndex((option) => option.id === value));
  const activate = useCallback((index: number) => {
    const option = options[index];
    if (option) onChange(option.id);
  }, [options, onChange]);
  useChoiceKeyboard(group, "radio", options.length, activate);
  const choices = (
    <View ref={group} style={[styles.group, layout === "scroll" ? styles.scrollGroup : null]} accessibilityRole="radiogroup" accessibilityLabel={label}>
      {options.map((option, index) => {
        const selected = value === option.id;
        return (
          <Control
            key={option.id}
            accessibilityRole="radio"
            accessibilityState={{ checked: selected }}
            {...(Platform.OS === "web" ? { tabIndex: index === selectedIndex ? 0 as const : -1 as const } : {})}
            accessibilityLabel={option.hint ?? option.label}
            onPress={() => onChange(option.id)}
            style={({ pressed }) => [
              styles.option,
              size === "sm" ? styles.optionSm : null,
              layout === "scroll" ? styles.scrollOption : null,
              { borderColor: selected ? theme.colors.accent : theme.colors.border },
              selected ? { backgroundColor: theme.colors.surfaceRaised } : null,
              pressed ? styles.pressed : null,
            ]}
          >
            <AppText variant="caption" tone={selected ? "text" : "textMuted"} style={styles.label}>
              {option.label}
            </AppText>
          </Control>
        );
      })}
    </View>
  );
  return layout === "scroll" ? (
    <ScrollView horizontal style={styles.scroll} showsHorizontalScrollIndicator={false} keyboardShouldPersistTaps="handled">
      {choices}
    </ScrollView>
  ) : choices;
}

const styles = StyleSheet.create({
  group: { flexDirection: "row", flexWrap: "wrap", gap: spacing.xs },
  scroll: { flexGrow: 0, flexShrink: 0 },
  scrollGroup: { flexWrap: "nowrap" },
  scrollOption: { flexGrow: 0, flexShrink: 0, flexBasis: "auto", minWidth: 84 },
  option: {
    flexGrow: 1,
    flexShrink: 1,
    flexBasis: 120,
    minWidth: 0,
    minHeight: 48,
    alignItems: "center",
    justifyContent: "center",
    borderWidth: 1,
    borderRadius: 10,
    borderCurve: "continuous",
    paddingHorizontal: spacing.md * componentPaddingScale,
  },
  optionSm: { minHeight: 32, paddingVertical: 2, flexBasis: 96 },
  pressed: { opacity: 0.75 },
  label: { fontWeight: "600" },
});
