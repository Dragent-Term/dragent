import { Image, StyleSheet, View } from "react-native";
import { useTheme } from "../theme";
import { AppText } from "./AppText";

export interface AvatarProps {
  name: string;
  size?: number;
  uri?: string | null | undefined;
}

/** Round user avatar: photo when a URI is set, otherwise initials circle.
 * Mirrors desktop share_sheet avatar: accent circle with single uppercase initial. */
export function Avatar({ name, size = 24, uri }: AvatarProps) {
  const { theme } = useTheme();
  const trimmed = name.trim();
  // Handle "First Last" or emails: take first alphanumeric char.
  const match = trimmed.match(/[A-Za-z0-9]/);
  const initial = match ? match[0]!.toUpperCase() : "?";
  const radius = size / 2;

  if (uri) {
    return (
      <View
        accessible={false}
        aria-hidden
        accessibilityElementsHidden
        importantForAccessibility="no-hide-descendants"
        style={[styles.shell, { width: size, height: size, borderRadius: radius, backgroundColor: theme.colors.accent }]}
      >
        <Image
          source={{ uri }}
          accessibilityIgnoresInvertColors
          style={{ width: size, height: size, borderRadius: radius }}
        />
      </View>
    );
  }

  return (
    <View
      accessible={false}
      aria-hidden
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
      style={[styles.shell, { width: size, height: size, borderRadius: radius, backgroundColor: theme.colors.accent }]}
    >
      <AppText style={[styles.initial, { color: theme.colors.onAccent, fontSize: Math.max(10, Math.round(size * 0.45)) }]}>
        {initial}
      </AppText>
    </View>
  );
}

export function avatarInitial(name: string): string {
  const match = name.trim().match(/[A-Za-z0-9]/);
  return match ? match[0]!.toUpperCase() : "?";
}

const styles = StyleSheet.create({
  shell: { flexShrink: 0, alignItems: "center", justifyContent: "center", overflow: "hidden" },
  initial: { fontWeight: "600", lineHeight: undefined },
});
