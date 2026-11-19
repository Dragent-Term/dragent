import type { ReactNode } from "react";
import { KeyboardAvoidingView, Platform, StyleSheet, View, type StyleProp, type ViewStyle } from "react-native";
import { SafeAreaView, type Edges } from "react-native-safe-area-context";
import { useTheme } from "../theme";
import { spacing } from "../theme/tokens";
import { useResponsiveLayout } from "./useResponsiveLayout";
import { KeyboardFrame, KeyboardScrollView } from "../components/KeyboardAware";

export interface ScreenProps {
  children: ReactNode;
  style?: StyleProp<ViewStyle>;
  contentStyle?: StyleProp<ViewStyle>;
  maxWidth?: number;
  /** Optional override; the app shell owns all safe-area edges by default. */
  edges?: Edges;
  /** Height of a native header above this screen when keyboard avoidance needs it. */
  keyboardVerticalOffset?: number;
}

const noEdges: Edges = [];

function ScreenFrame({ children, style, edges = noEdges, keyboardVerticalOffset = 0 }: ScreenProps) {
  const { theme } = useTheme();
  return (
    <SafeAreaView edges={edges} style={[styles.fill, { backgroundColor: theme.colors.background }, style]}>
      <KeyboardAvoidingView
        style={styles.fill}
        behavior={Platform.OS === "ios" ? "padding" : undefined}
        keyboardVerticalOffset={keyboardVerticalOffset}
      >
        {children}
      </KeyboardAvoidingView>
    </SafeAreaView>
  );
}

/** Non-scrolling frame, suitable for a virtualized list or a full-height viewer. */
export function Screen({ children, contentStyle, maxWidth, ...props }: ScreenProps) {
  const layout = useResponsiveLayout();
  const contentWidth = Math.max(0, Math.min(maxWidth ?? layout.maxContentWidth, layout.maxContentWidth));
  return (
    <ScreenFrame {...props}>
      <View style={[styles.viewport, { paddingHorizontal: layout.gutter }]}>
        <View style={[styles.content, styles.fill, { maxWidth: contentWidth }, contentStyle]}>
          {children}
        </View>
      </View>
    </ScreenFrame>
  );
}

/** For forms and finite content. Use Screen + FlatList for unbounded lists. */
export function ScrollScreen({ children, contentStyle, maxWidth, style, edges = noEdges }: ScreenProps) {
  const { theme } = useTheme();
  const layout = useResponsiveLayout();
  const contentWidth = Math.max(0, Math.min(maxWidth ?? layout.maxContentWidth, layout.maxContentWidth));
  // The keyboard is measured against where this screen really is (under any
  // header), and the field being typed in is scrolled above it.
  return (
    <SafeAreaView edges={edges} style={[styles.fill, { backgroundColor: theme.colors.background }, style]}>
      <KeyboardFrame>
      <KeyboardScrollView
        style={styles.fill}
        contentContainerStyle={[styles.scrollContent, { paddingHorizontal: layout.gutter }]}
        keyboardShouldPersistTaps="handled"
        keyboardDismissMode="on-drag"
        contentInsetAdjustmentBehavior="never"
        automaticallyAdjustContentInsets={false}
        automaticallyAdjustKeyboardInsets={false}
      >
        <View style={[styles.content, { maxWidth: contentWidth }, contentStyle]}>
          {children}
        </View>
      </KeyboardScrollView>
      </KeyboardFrame>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  fill: { flex: 1 },
  viewport: { flex: 1, alignItems: "center", paddingVertical: spacing.lg },
  content: { width: "100%", minWidth: 0, gap: spacing.lg },
  scrollContent: { flexGrow: 1, alignItems: "center", paddingVertical: spacing.lg },
});
