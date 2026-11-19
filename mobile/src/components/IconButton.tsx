import { StyleSheet, type PressableProps, type StyleProp, type ViewStyle } from "react-native";
import { useTheme } from "../theme";
import { radii } from "../theme/tokens";
import { Control } from "./Control";
import { Icon, type IconName, type IconTone } from "./Icon";

export interface IconButtonProps extends Omit<PressableProps, "children" | "style"> {
  name: IconName;
  label: string;
  selected?: boolean;
  tone?: IconTone;
  size?: "md" | "sm";
  style?: StyleProp<ViewStyle>;
}

export function IconButton({
  name, label, selected = false, tone, size = "md", disabled = false, accessibilityState, style, ...props
}: IconButtonProps) {
  const { theme } = useTheme();
  return (
    <Control
      {...props}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ ...accessibilityState, selected, disabled }}
      style={({ hovered, pressed }) => [
        size === "sm" ? styles.shellSm : styles.shell,
        {
          backgroundColor: selected ? theme.colors.selection
            : hovered || pressed ? theme.colors.surfaceRaised : undefined,
        },
        pressed ? styles.pressed : null,
        disabled ? styles.disabled : null,
        style,
      ]}
    >
      <Icon name={name} size={size === "sm" ? 14 : 18} tone={tone ?? (selected ? "text" : "textMuted")} />
    </Control>
  );
}

const styles = StyleSheet.create({
  shell: {
    minHeight: 44,
    minWidth: 44,
    borderRadius: radii.md,
    borderCurve: "continuous",
    alignItems: "center",
    justifyContent: "center",
  },
  shellSm: {
    minHeight: 32,
    minWidth: 32,
    borderRadius: radii.md,
    borderCurve: "continuous",
    alignItems: "center",
    justifyContent: "center",
  },
  pressed: { opacity: 0.75 },
  disabled: { opacity: 0.4 },
});
