import { useState, type ComponentRef, type ReactNode, type Ref } from "react";
import { Platform, StyleSheet, TextInput, View, type StyleProp, type TextInputProps, type ViewStyle } from "react-native";
import { useTheme } from "../theme";
import { inputColorsFor } from "../theme/inputColors";
import { componentPaddingScale, radii, spacing, type } from "../theme/tokens";

export interface FieldInputProps extends TextInputProps {
  leading?: ReactNode;
  error?: string | undefined;
  containerStyle?: StyleProp<ViewStyle>;
  ref?: Ref<ComponentRef<typeof TextInput>>;
}

/** Shared field frame and native input; visible labels belong to FormField. */
export function FieldInput({
  leading, error, containerStyle, style, ref, onFocus, onBlur,
  editable = true, accessibilityState, ...props
}: FieldInputProps) {
  const { theme } = useTheme();
  const caretColors = inputColorsFor(theme.colors.accent, theme.colors.background, Platform.OS);
  const [focused, setFocused] = useState(false);
  return (
    <View style={[
      styles.frame,
      {
        backgroundColor: theme.colors.surface,
        borderColor: error ? theme.colors.danger : focused ? theme.colors.accent : theme.colors.border,
      },
      props.multiline ? styles.frameMultiline : null,
      !editable ? styles.disabled : null,
      containerStyle,
    ]}>
      {leading}
      <TextInput
        ref={ref}
        allowFontScaling
        placeholderTextColor={theme.colors.textSubtle}
        selectionColor={caretColors.selection}
        cursorColor={caretColors.caret}
        selectionHandleColor={caretColors.caret}
        underlineColorAndroid={theme.colors.surface}
        {...props}
        editable={editable}
        accessibilityState={{ ...accessibilityState, disabled: !editable }}
        onFocus={(event) => { setFocused(true); onFocus?.(event); }}
        onBlur={(event) => { setFocused(false); onBlur?.(event); }}
        style={[
          styles.input,
          { color: theme.colors.text },
          props.multiline ? styles.multiline : null,
          style,
        ]}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  frame: {
    minHeight: 40,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    borderWidth: 1,
    borderRadius: radii.md,
    borderCurve: "continuous",
    paddingHorizontal: spacing.md * componentPaddingScale,
    paddingVertical: 2,
  },
  input: {
    ...type.body,
    flex: 1,
    minWidth: 0,
    minHeight: 34,
    paddingVertical: 6 * componentPaddingScale,
    paddingHorizontal: 0,
    // The frame owns the visible focus treatment on native and web.
    outlineWidth: 0,
  },
  // One line to start, growing with the text like the chat box, then scrolling.
  multiline: { maxHeight: 160, textAlignVertical: "top", alignSelf: "stretch", paddingTop: 8, paddingBottom: 8 },
  frameMultiline: { alignItems: "flex-start" },
  disabled: { opacity: 0.4 },
});
