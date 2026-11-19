import { useId } from "react";
import { StyleSheet, View } from "react-native";
import { spacing } from "../theme/tokens";
import { AppText } from "./AppText";
import { FieldInput, type FieldInputProps } from "./FieldInput";

export interface FormFieldProps extends Omit<FieldInputProps, "leading"> {
  label: string;
  hint?: string | undefined;
}

export function FormField({
  label, hint, error, containerStyle, accessibilityLabel = label,
  accessibilityHint, ...props
}: FormFieldProps) {
  const labelId = useId();
  const description = [error, hint].filter(Boolean).join(". ");
  return (
    <View style={[styles.shell, containerStyle]}>
      <AppText nativeID={labelId} variant="caption" style={styles.label}>{label}</AppText>
      <FieldInput
        accessibilityLabelledBy={labelId}
        {...props}
        accessibilityLabel={accessibilityLabel}
        accessibilityHint={accessibilityHint ?? (description || undefined)}
        error={error}
      />
      {hint ? <AppText variant="caption" tone="textMuted">{hint}</AppText> : null}
      {error ? (
        <AppText variant="caption" tone="danger" accessibilityRole="alert" accessibilityLiveRegion="polite">
          {error}
        </AppText>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  shell: { gap: spacing.xs },
  label: { fontWeight: "500" },
});
